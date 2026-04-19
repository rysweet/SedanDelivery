//! Function argument-kind validation and aggregation determinism checks.

use crate::ast::Expr;
use crate::error::GqlError;

use crate::planner::validator::{plan_error_semantic, ValidationContext, VarKind};

use super::aggregation::is_nondeterministic_function;

impl ValidationContext {
    /// Validate that type-restricted functions receive the correct argument kinds.
    pub(super) fn validate_function_arg_kinds(
        &self,
        name: &str,
        args: &[Expr],
    ) -> Result<(), GqlError> {
        let lower = name.to_ascii_lowercase();
        match lower.as_str() {
            "type" => {
                // type(r) requires a Relationship argument
                if let Some(Expr::Variable(ref id)) = args.first() {
                    if let Some(&kind) = self.vars.get(&id.0) {
                        if kind == VarKind::Node {
                            return Err(plan_error_semantic(
                                "type() requires a relationship argument, got a node",
                            ));
                        }
                    }
                }
            }
            "length" => {
                // length(p) requires a Path argument — not a node or relationship
                if let Some(Expr::Variable(ref id)) = args.first() {
                    if let Some(&kind) = self.vars.get(&id.0) {
                        if kind == VarKind::Node || kind == VarKind::Relationship {
                            return Err(plan_error_semantic(
                                "length() requires a path argument, not a node or relationship",
                            ));
                        }
                    }
                }
            }
            "size" => {
                // size(p) where p is a path variable is invalid (InvalidArgumentType)
                if let Some(Expr::Variable(ref id)) = args.first() {
                    if let Some(&kind) = self.vars.get(&id.0) {
                        if kind == VarKind::Path {
                            return Err(plan_error_semantic(
                                "size() requires a list or string argument, not a path",
                            ));
                        }
                    }
                }
                // size(pattern) where argument is a PatternExpr is UnexpectedSyntax
                if let Some(arg) = args.first() {
                    if matches!(arg, Expr::PatternExpr(_)) {
                        return Err(plan_error_semantic(
                            "size() does not accept a pattern expression",
                        ));
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Check that aggregation functions don't contain non-deterministic (non-constant) functions.
    pub(super) fn check_no_nondeterministic_in_agg(
        &self,
        _agg_name: &str,
        args: &[Expr],
    ) -> Result<(), GqlError> {
        for arg in args {
            if self.contains_nondeterministic(arg) {
                return Err(plan_error_semantic(
                    "non-deterministic functions (like rand()) are not allowed inside aggregation",
                ));
            }
        }
        Ok(())
    }

    fn contains_nondeterministic(&self, expr: &Expr) -> bool {
        match expr {
            Expr::FunctionCall { name, args, .. } => {
                if is_nondeterministic_function(name.as_str()) {
                    return true;
                }
                args.iter().any(|a| self.contains_nondeterministic(a))
            }
            Expr::BinaryOp { left, right, .. } => {
                self.contains_nondeterministic(left) || self.contains_nondeterministic(right)
            }
            Expr::UnaryOp { expr, .. } => self.contains_nondeterministic(expr),
            Expr::Property(inner, _) => self.contains_nondeterministic(inner),
            Expr::List(items) => items.iter().any(|i| self.contains_nondeterministic(i)),
            _ => false,
        }
    }
}
