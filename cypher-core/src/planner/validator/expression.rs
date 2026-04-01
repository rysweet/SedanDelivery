//! Expression validation, type checking, aggregation helpers, and function validation.

use std::collections::HashSet;

use crate::ast::{BinaryOp, Expr, UnaryOp};
use crate::error::GqlError;

use super::{plan_error_semantic, ValidationContext, VarKind};

// ── Literal classification helpers ──────────────────────────────────────────

/// Returns true if expr contains no variable references (pure literal/arithmetic).
pub(super) fn is_non_variable_expr(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Integer(_) | Expr::Float(_) | Expr::Str(_) | Expr::Bool(_) | Expr::Null
    )
}

/// Returns true if expr is a non-boolean literal (includes all literal types except Bool and Null).
fn is_non_boolean_literal(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Integer(_) | Expr::Float(_) | Expr::Str(_) | Expr::List(_) | Expr::MapLiteral(_)
    )
}

fn is_non_numeric_literal(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Str(_) | Expr::Bool(_) | Expr::List(_) | Expr::MapLiteral(_)
    )
}

fn is_non_string_non_null_literal(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Integer(_) | Expr::Float(_) | Expr::Bool(_) | Expr::List(_) | Expr::MapLiteral(_)
    )
}

// ── Aggregation helpers ─────────────────────────────────────────────────────

pub(super) fn contains_aggregation(expr: &Expr) -> bool {
    match expr {
        Expr::FunctionCall { name, args, .. } => {
            if is_aggregation_function(name.as_str()) {
                return true;
            }
            args.iter().any(contains_aggregation)
        }
        Expr::BinaryOp { left, right, .. } => {
            contains_aggregation(left) || contains_aggregation(right)
        }
        Expr::UnaryOp { expr, .. } => contains_aggregation(expr),
        Expr::Property(inner, _) => contains_aggregation(inner),
        Expr::IsNull(inner) | Expr::IsNotNull(inner) => contains_aggregation(inner),
        Expr::Subscript(a, b) => contains_aggregation(a) || contains_aggregation(b),
        Expr::Slice(a, b, c) => {
            contains_aggregation(a)
                || b.as_ref().is_some_and(|e| contains_aggregation(e))
                || c.as_ref().is_some_and(|e| contains_aggregation(e))
        }
        Expr::Case {
            subject,
            alternatives,
            default,
        } => {
            subject.as_ref().is_some_and(|s| contains_aggregation(s))
                || alternatives
                    .iter()
                    .any(|(w, t)| contains_aggregation(w) || contains_aggregation(t))
                || default.as_ref().is_some_and(|d| contains_aggregation(d))
        }
        Expr::List(items) => items.iter().any(contains_aggregation),
        Expr::MapLiteral(pairs) => pairs.iter().any(|(_, v)| contains_aggregation(v)),
        Expr::ListComp {
            list, filter, map, ..
        } => {
            contains_aggregation(list)
                || filter.as_ref().is_some_and(|f| contains_aggregation(f))
                || contains_aggregation(map)
        }
        Expr::Reduce {
            initial,
            list,
            expression,
            ..
        } => {
            contains_aggregation(initial)
                || contains_aggregation(list)
                || contains_aggregation(expression)
        }
        _ => false,
    }
}

/// Returns true if an expression has ambiguous aggregation — e.g. `a.x + count(*)`
/// where an arithmetic operator at the top level mixes aggregated and non-aggregated children,
/// AND those non-aggregated children are themselves complex expressions (not simple variables
/// or property accesses, which would be valid grouping keys).
pub(super) fn has_ambiguous_aggregation(expr: &Expr) -> bool {
    match expr {
        Expr::BinaryOp { left, right, op } => {
            // Only check arithmetic operators
            if !matches!(
                op,
                BinaryOp::Add
                    | BinaryOp::Sub
                    | BinaryOp::Mul
                    | BinaryOp::Div
                    | BinaryOp::Mod
                    | BinaryOp::Pow
                    | BinaryOp::Concat
            ) {
                return false;
            }
            let left_has = contains_aggregation(left);
            let right_has = contains_aggregation(right);
            if left_has && !right_has && !is_simple_grouping_key(right) {
                return true;
            }
            if right_has && !left_has && !is_simple_grouping_key(left) {
                return true;
            }
            false
        }
        _ => false,
    }
}

/// Returns true if expr is a simple grouping key (variable, property, literal, or list/map).
fn is_simple_grouping_key(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Variable(_)
            | Expr::Property(_, _)
            | Expr::Integer(_)
            | Expr::Float(_)
            | Expr::Str(_)
            | Expr::Bool(_)
            | Expr::Null
            | Expr::Parameter(_)
            | Expr::List(_)
            | Expr::MapLiteral(_)
    )
}

pub(super) fn has_nested_aggregation(expr: &Expr, inside_agg: bool) -> bool {
    match expr {
        Expr::FunctionCall { name, args, .. } => {
            if is_aggregation_function(name.as_str()) {
                if inside_agg {
                    return true;
                }
                return args.iter().any(|a| has_nested_aggregation(a, true));
            }
            args.iter().any(|a| has_nested_aggregation(a, inside_agg))
        }
        Expr::BinaryOp { left, right, .. } => {
            has_nested_aggregation(left, inside_agg) || has_nested_aggregation(right, inside_agg)
        }
        Expr::UnaryOp { expr, .. } => has_nested_aggregation(expr, inside_agg),
        Expr::Property(inner, _) => has_nested_aggregation(inner, inside_agg),
        Expr::IsNull(inner) | Expr::IsNotNull(inner) => has_nested_aggregation(inner, inside_agg),
        _ => false,
    }
}

/// Collect all variable names referenced in an expression.
pub(super) fn collect_variable_refs(expr: &Expr, vars: &mut HashSet<String>) {
    match expr {
        Expr::Variable(ref id) => {
            if id.0 != "*" {
                vars.insert(id.0.clone());
            }
        }
        Expr::Property(inner, _) => collect_variable_refs(inner, vars),
        Expr::BinaryOp { left, right, .. } => {
            collect_variable_refs(left, vars);
            collect_variable_refs(right, vars);
        }
        Expr::UnaryOp { expr, .. } => collect_variable_refs(expr, vars),
        Expr::FunctionCall { args, .. } => {
            for arg in args {
                collect_variable_refs(arg, vars);
            }
        }
        Expr::IsNull(inner) | Expr::IsNotNull(inner) | Expr::HasLabel(inner, _) => {
            collect_variable_refs(inner, vars);
        }
        Expr::List(items) => {
            for item in items {
                collect_variable_refs(item, vars);
            }
        }
        Expr::MapLiteral(pairs) => {
            for (_, v) in pairs {
                collect_variable_refs(v, vars);
            }
        }
        Expr::Subscript(a, b) => {
            collect_variable_refs(a, vars);
            collect_variable_refs(b, vars);
        }
        _ => {}
    }
}

pub(super) fn is_nondeterministic_function(name: &str) -> bool {
    matches!(name.to_ascii_lowercase().as_str(), "rand")
}

pub(super) fn is_aggregation_function(name: &str) -> bool {
    // Delegate to the authoritative implementation in the parent planner module.
    crate::planner::is_aggregate_function(name)
}

// ── Function name validation ────────────────────────────────────────────────

pub(super) fn validate_known_function(name: &str) -> Result<(), GqlError> {
    let lower = name.to_ascii_lowercase();
    let known = matches!(
        lower.as_str(),
        "abs"
            | "ceil"
            | "floor"
            | "round"
            | "sqrt"
            | "sign"
            | "tostring"
            | "tointeger"
            | "toint"
            | "tofloat"
            | "toboolean"
            | "size"
            | "length"
            | "toupper"
            | "tolower"
            | "trim"
            | "ltrim"
            | "rtrim"
            | "left"
            | "right"
            | "split"
            | "replace"
            | "substring"
            | "head"
            | "last"
            | "tail"
            | "reverse"
            | "coalesce"
            | "isnull"
            | "range"
            | "type"
            | "keys"
            | "labels"
            | "nodes"
            | "relationships"
            | "id"
            | "properties"
            | "startnode"
            | "endnode"
            | "exists"
            | "rand"
            | "timestamp"
            | "pi"
            | "e"
            | "log"
            | "log10"
            | "exp"
            | "sin"
            | "cos"
            | "tan"
            | "asin"
            | "acos"
            | "atan"
            | "atan2"
            | "degrees"
            | "radians"
            | "haversin"
            | "count"
            | "sum"
            | "avg"
            | "min"
            | "max"
            | "collect"
            | "stdev"
            | "stdevp"
            | "percentilecont"
            | "percentiledisc"
            | "single"
            | "any"
            | "all"
            | "none"
            | "reduce"
            | "extract"
            | "filter"
            | "tobooleanlist"
            | "tointegerlist"
            | "tofloatlist"
            | "tostringlist"
            | "tobooleanornull"
            | "tointegerornull"
            | "tofloatornull"
            | "tostringornull"
            | "char_length"
            | "character_length"
            // Temporal functions
            | "date"
            | "datetime"
            | "localdatetime"
            | "time"
            | "localtime"
            | "duration"
            // Temporal namespaced functions
            | "date.truncate"
            | "datetime.truncate"
            | "localdatetime.truncate"
            | "localtime.truncate"
            | "time.truncate"
            | "date.realtime"
            | "date.statement"
            | "date.transaction"
            | "datetime.realtime"
            | "datetime.statement"
            | "datetime.transaction"
            | "datetime.fromepoch"
            | "datetime.fromepochmillis"
            | "localdatetime.realtime"
            | "localdatetime.statement"
            | "localdatetime.transaction"
            | "localtime.realtime"
            | "localtime.statement"
            | "localtime.transaction"
            | "time.realtime"
            | "time.statement"
            | "time.transaction"
            | "duration.between"
            | "duration.inmonths"
            | "duration.indays"
            | "duration.inseconds"
            // Spatial functions
            | "point"
            | "distance"
    );
    if !known {
        return Err(plan_error_semantic(&format!("unknown function '{name}'")));
    }
    Ok(())
}

// ── Function arity validation ───────────────────────────────────────────────

pub(super) fn validate_function_arity(name: &str, args: &[Expr]) -> Result<(), GqlError> {
    let n = args.len();
    let lower_name = name.to_ascii_lowercase();
    let expected = match lower_name.as_str() {
        // Functions that take exactly 0 arguments
        "rand" | "timestamp" | "pi" | "e" => Some((0, 0)),
        // Functions that take exactly 1 argument
        "abs" | "ceil" | "floor" | "round" | "sqrt" | "sign" | "tostring" | "tointeger"
        | "toint" | "tofloat" | "toboolean" | "size" | "length" | "toupper" | "tolower"
        | "trim" | "ltrim" | "rtrim" | "head" | "last" | "tail" | "reverse" | "isnull" | "type"
        | "keys" | "labels" | "nodes" | "relationships" | "id" | "properties" | "startnode"
        | "endnode" | "exists" | "log" | "log10" | "exp" | "sin" | "cos" | "tan" | "asin"
        | "acos" | "atan" | "degrees" | "radians" | "haversin" => Some((1, 1)),
        // Functions that take exactly 2 arguments
        "left" | "right" | "split" | "atan2" => Some((2, 2)),
        // Functions that take exactly 3 arguments
        "replace" => Some((3, 3)),
        // Functions that take 2-3 arguments
        "substring" => Some((2, 3)),
        // Functions that take variable arguments
        "range" => Some((2, 3)),
        "coalesce" => {
            if n == 0 {
                return Err(plan_error_semantic(
                    "coalesce() requires at least 1 argument",
                ));
            }
            None
        }
        // Temporal functions: 0 or 1 argument
        "date" | "datetime" | "localdatetime" | "time" | "localtime" | "duration" => Some((0, 1)),
        // Spatial functions
        "point" => Some((1, 1)),
        "distance" => Some((2, 2)),
        // Aggregation functions: 1 arg (or 0 for count(*))
        "count" => Some((0, 1)),
        "sum" | "avg" | "collect" | "stdev" | "stdevp" | "min" | "max" => Some((1, 1)),
        "percentilecont" | "percentiledisc" => Some((2, 2)),
        _ => None, // Unknown functions are handled elsewhere
    };
    if let Some((min, max)) = expected {
        if n < min || n > max {
            return Err(plan_error_semantic(&format!(
                "function {name}() expects {}{} argument(s), got {n}",
                min,
                if min != max {
                    format!("-{max}")
                } else {
                    String::new()
                }
            )));
        }
    }
    Ok(())
}

// ── General expression validation (impl on ValidationContext) ────────────────

impl ValidationContext {
    /// Validate expressions for compile-time type errors.
    /// This catches: `NOT <non-boolean-literal>`, `<non-boolean> AND/OR <expr>`.
    pub(super) fn validate_expr_general(&self, expr: &Expr) -> Result<(), GqlError> {
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
                    BinaryOp::And | BinaryOp::Or | BinaryOp::Xor => {
                        // Boolean operators on non-boolean literals
                        if is_non_boolean_literal(left) || is_non_boolean_literal(right) {
                            return Err(plan_error_semantic(&format!(
                                "{} requires boolean arguments, got a non-boolean literal",
                                super::op_name(*op)
                            )));
                        }
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
                    | BinaryOp::RegexMatch => {
                        // String operations on non-string literals
                        if is_non_string_non_null_literal(left)
                            || is_non_string_non_null_literal(right)
                        {
                            return Err(plan_error_semantic(&format!(
                                "{} requires string arguments",
                                super::op_name(*op)
                            )));
                        }
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

    // ── Function argument type validation ──────────────────────────────────

    /// Validate that type-restricted functions receive the correct argument kinds.
    fn validate_function_arg_kinds(&self, name: &str, args: &[Expr]) -> Result<(), GqlError> {
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
    fn check_no_nondeterministic_in_agg(
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
