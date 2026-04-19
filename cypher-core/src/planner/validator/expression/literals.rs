//! Literal-classification helpers used by expression validation.

use crate::ast::Expr;

/// Returns true if expr contains no variable references (pure literal/arithmetic).
pub(in crate::planner::validator) fn is_non_variable_expr(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Integer(_) | Expr::Float(_) | Expr::Str(_) | Expr::Bool(_) | Expr::Null
    )
}

/// Returns true if expr is a non-boolean literal (includes all literal types except Bool and Null).
pub(super) fn is_non_boolean_literal(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Integer(_) | Expr::Float(_) | Expr::Str(_) | Expr::List(_) | Expr::MapLiteral(_)
    )
}

pub(super) fn is_non_numeric_literal(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Str(_) | Expr::Bool(_) | Expr::List(_) | Expr::MapLiteral(_)
    )
}

pub(super) fn is_non_string_non_null_literal(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Integer(_) | Expr::Float(_) | Expr::Bool(_) | Expr::List(_) | Expr::MapLiteral(_)
    )
}

/// Returns true if expr is a simple grouping key (variable, property, literal, or list/map).
pub(super) fn is_simple_grouping_key(expr: &Expr) -> bool {
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
