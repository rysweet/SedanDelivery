//! General expression validation entry point on `ValidationContext`.

use crate::ast::{BinaryOp, Expr, UnaryOp};
use crate::error::GqlError;

use crate::planner::validator::{op_name, plan_error_semantic, ValidationContext};

use super::aggregation::{contains_aggregation, is_aggregation_function};
use super::functions::{validate_function_arity, validate_known_function};
use super::literals::{
    is_non_boolean_literal, is_non_numeric_literal, is_non_string_non_null_literal,
};

impl ValidationContext {
    /// Validate expressions for compile-time type errors.
    /// This catches: `NOT <non-boolean-literal>`, `<non-boolean> AND/OR <expr>`.
    pub(in crate::planner::validator) fn validate_expr_general(
        &self,
        expr: &Expr,
    ) -> Result<(), GqlError> {
        match expr {
            // Check for floating-point overflow (e.g. 1.34E999 -> Infinity)
            Expr::Float(f) => {
                if f.is_infinite() {
                    return Err(plan_error_semantic(
                        "floating-point literal is too large (overflow to infinity)",
                    ));
                }
                return Ok(());
            }
            Expr::UnaryOp {
                op: UnaryOp::Not,
                expr: inner,
            } => {
                self.validate_expr_general(inner)?;
                // NOT on a non-boolean literal is a compile-time error
                if is_non_boolean_literal(inner) {
                    return Err(plan_error_semantic(
                        "NOT requires a boolean argument, got a non-boolean literal",
                    ));
                }
            }
            Expr::UnaryOp {
                op: UnaryOp::Neg,
                expr: inner,
            } => {
                self.validate_expr_general(inner)?;
                // -<non-numeric literal> is a type error
                if is_non_numeric_literal(inner) {
                    return Err(plan_error_semantic(
                        "unary minus requires a numeric argument",
                    ));
                }
            }
            Expr::BinaryOp { op, left, right } => {
                self.validate_expr_general(left)?;
                self.validate_expr_general(right)?;

                match op {
                    BinaryOp::And | BinaryOp::Or | BinaryOp::Xor
                        // Boolean operators on non-boolean literals
                        if (is_non_boolean_literal(left) || is_non_boolean_literal(right)) => {
                            return Err(plan_error_semantic(&format!(
                                "{} requires boolean arguments, got a non-boolean literal",
                                op_name(*op)
                            )));
                        }
                    BinaryOp::Add
                    | BinaryOp::Sub
                    | BinaryOp::Mul
                    | BinaryOp::Div
                    | BinaryOp::Mod
                    | BinaryOp::Pow => {
                        // Arithmetic on booleans is caught at runtime
                    }
                    BinaryOp::StartsWith
                    | BinaryOp::EndsWith
                    | BinaryOp::Contains
                    | BinaryOp::RegexMatch
                        // String operations on non-string literals
                        if (is_non_string_non_null_literal(left)
                            || is_non_string_non_null_literal(right))
                        => {
                            return Err(plan_error_semantic(&format!(
                                "{} requires string arguments",
                                op_name(*op)
                            )));
                        }
                    _ => {}
                }
            }
            Expr::FunctionCall { name, args, .. } => {
                // Reject direct user calls to __patternComp.
                // This is an internal function emitted only by the parser for pattern
                // comprehensions like `[(n:Person) | n.name]`. Parser-generated calls
                // always have an Expr::PatternExpr as their first argument; user-typed
                // calls do not, so we can detect and reject them here.
                if name.to_ascii_lowercase().as_str() == "__patterncomp" {
                    if !args
                        .first()
                        .is_some_and(|a| matches!(a, Expr::PatternExpr(_)))
                    {
                        return Err(plan_error_semantic(
                            "function '__patternComp' is an internal function and cannot be called directly",
                        ));
                    }
                    // Parser-generated — skip args[0] (PatternExpr introduces its own scope)
                    // but validate the map expression (args[1]) and optional WHERE (args[2]).
                    for arg in args.iter().skip(1) {
                        self.validate_expr_general(arg)?;
                    }
                    return Ok(());
                }
                for arg in args {
                    self.validate_expr_general(arg)?;
                }
                // Validate function is known
                validate_known_function(name.as_str())?;
                // Validate function argument counts for known functions at compile time
                validate_function_arity(name.as_str(), args)?;
                // Validate argument kinds for type-restricted functions
                self.validate_function_arg_kinds(name.as_str(), args)?;
                // Check for non-constant (non-deterministic) functions inside aggregation
                if is_aggregation_function(name.as_str()) {
                    self.check_no_nondeterministic_in_agg(name.as_str(), args)?;
                }
            }
            Expr::Property(inner, _) => {
                self.validate_expr_general(inner)?;
            }
            Expr::IsNull(inner) | Expr::IsNotNull(inner) => {
                self.validate_expr_general(inner)?;
            }
            Expr::Subscript(a, b) => {
                self.validate_expr_general(a)?;
                self.validate_expr_general(b)?;
            }
            Expr::Slice(a, b, c) => {
                self.validate_expr_general(a)?;
                if let Some(b) = b {
                    self.validate_expr_general(b)?;
                }
                if let Some(c) = c {
                    self.validate_expr_general(c)?;
                }
            }
            Expr::List(items) => {
                for item in items {
                    self.validate_expr_general(item)?;
                }
            }
            Expr::MapLiteral(pairs) => {
                for (_, v) in pairs {
                    self.validate_expr_general(v)?;
                }
            }
            Expr::Case {
                subject,
                alternatives,
                default,
            } => {
                if let Some(s) = subject {
                    self.validate_expr_general(s)?;
                }
                for (w, t) in alternatives {
                    self.validate_expr_general(w)?;
                    self.validate_expr_general(t)?;
                }
                if let Some(d) = default {
                    self.validate_expr_general(d)?;
                }
            }
            Expr::ListComp {
                list, filter, map, ..
            } => {
                self.validate_expr_general(list)?;
                if let Some(f) = filter {
                    self.validate_expr_general(f)?;
                }
                // Aggregation functions are not allowed inside list comprehension body
                if contains_aggregation(map) {
                    return Err(plan_error_semantic(
                        "aggregation functions are not allowed inside list comprehension",
                    ));
                }
                if let Some(f) = filter {
                    if contains_aggregation(f) {
                        return Err(plan_error_semantic(
                            "aggregation functions are not allowed inside list comprehension filter",
                        ));
                    }
                }
                self.validate_expr_general(map)?;
            }
            Expr::Reduce {
                initial,
                list,
                expression,
                ..
            } => {
                self.validate_expr_general(initial)?;
                self.validate_expr_general(list)?;
                if contains_aggregation(expression) {
                    return Err(plan_error_semantic(
                        "aggregation functions are not allowed inside reduce expression",
                    ));
                }
                self.validate_expr_general(expression)?;
            }
            _ => {}
        }
        Ok(())
    }
}
