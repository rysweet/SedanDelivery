//! Helpers for detecting whether expressions reference aliases that are
//! still pending creation in the current `CREATE` pattern. These checks
//! drive the flush boundaries between batched node/edge creates so that
//! later items can correctly read earlier-bound aliases.

use std::collections::HashSet;

use crate::ast::Expr;

pub(super) fn expr_references_any_alias(expr: &Expr, aliases: &HashSet<String>) -> bool {
    match expr {
        Expr::Variable(id) => aliases.contains(id.0.as_str()),
        Expr::Property(obj, key) => {
            let _ = key;
            expr_references_any_alias(obj, aliases)
        }
        Expr::BinaryOp { left, right, .. } => {
            expr_references_any_alias(left, aliases) || expr_references_any_alias(right, aliases)
        }
        Expr::UnaryOp { expr, .. }
        | Expr::IsNull(expr)
        | Expr::IsNotNull(expr)
        | Expr::HasLabel(expr, _) => expr_references_any_alias(expr, aliases),
        Expr::FunctionCall { args, .. } | Expr::List(args) => args
            .iter()
            .any(|arg| expr_references_any_alias(arg, aliases)),
        Expr::MapLiteral(entries) => entries
            .iter()
            .any(|(_, value)| expr_references_any_alias(value, aliases)),
        Expr::Case {
            subject,
            alternatives,
            default,
        } => {
            subject
                .as_deref()
                .is_some_and(|expr| expr_references_any_alias(expr, aliases))
                || alternatives.iter().any(|(when, then)| {
                    expr_references_any_alias(when, aliases)
                        || expr_references_any_alias(then, aliases)
                })
                || default
                    .as_deref()
                    .is_some_and(|expr| expr_references_any_alias(expr, aliases))
        }
        Expr::Subscript(list, index) => {
            expr_references_any_alias(list, aliases) || expr_references_any_alias(index, aliases)
        }
        Expr::Slice(list, start, end) => {
            expr_references_any_alias(list, aliases)
                || start
                    .as_deref()
                    .is_some_and(|expr| expr_references_any_alias(expr, aliases))
                || end
                    .as_deref()
                    .is_some_and(|expr| expr_references_any_alias(expr, aliases))
        }
        Expr::ListComp {
            variable,
            list,
            filter,
            map,
        } => {
            if expr_references_any_alias(list, aliases) {
                return true;
            }
            let scoped_aliases: HashSet<String> = aliases
                .iter()
                .filter(|alias| alias.as_str() != variable.0.as_str())
                .cloned()
                .collect();
            filter
                .as_deref()
                .is_some_and(|expr| expr_references_any_alias(expr, &scoped_aliases))
                || expr_references_any_alias(map, &scoped_aliases)
        }
        Expr::Reduce {
            accumulator,
            initial,
            variable,
            list,
            expression,
        } => {
            if expr_references_any_alias(initial, aliases)
                || expr_references_any_alias(list, aliases)
            {
                return true;
            }
            let scoped_aliases: HashSet<String> = aliases
                .iter()
                .filter(|alias| {
                    alias.as_str() != accumulator.0.as_str()
                        && alias.as_str() != variable.0.as_str()
                })
                .cloned()
                .collect();
            expr_references_any_alias(expression, &scoped_aliases)
        }
        Expr::ExistsSubquery {
            where_clause,
            full_query,
            ..
        } => {
            where_clause
                .as_deref()
                .is_some_and(|expr| expr_references_any_alias(expr, aliases))
                || full_query.is_some()
        }
        Expr::PatternExpr(_) => !aliases.is_empty(),
        Expr::Integer(_)
        | Expr::Float(_)
        | Expr::Str(_)
        | Expr::Bool(_)
        | Expr::Null
        | Expr::Parameter(_) => false,
    }
}

pub(super) fn prop_exprs_reference_any_alias(
    prop_exprs: &[(String, Expr)],
    aliases: &HashSet<String>,
) -> bool {
    prop_exprs
        .iter()
        .any(|(_, expr)| expr_references_any_alias(expr, aliases))
}
