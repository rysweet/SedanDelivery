//! Aggregation detection and nondeterministic-function helpers.

use crate::ast::{BinaryOp, Expr};

use super::literals::is_simple_grouping_key;

pub(in crate::planner::validator) fn contains_aggregation(expr: &Expr) -> bool {
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
pub(in crate::planner::validator) fn has_ambiguous_aggregation(expr: &Expr) -> bool {
    match expr {
        Expr::BinaryOp { left, right, op } => {
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

pub(in crate::planner::validator) fn has_nested_aggregation(expr: &Expr, inside_agg: bool) -> bool {
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

pub(in crate::planner::validator) fn is_nondeterministic_function(name: &str) -> bool {
    matches!(name.to_ascii_lowercase().as_str(), "rand")
}

pub(in crate::planner::validator) fn is_aggregation_function(name: &str) -> bool {
    // Delegate to the authoritative implementation in the parent planner module.
    crate::planner::is_aggregate_function(name)
}
