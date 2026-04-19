//! ORDER BY rewriting in the presence of aggregation / WITH projection.

use crate::ast::{Expr, Identifier, ReturnItem};

use super::aggregation::is_agg_name;
use super::column_name::{expr_to_col_name, return_item_name};

/// Rewrite an ORDER BY expression in a WITH clause that uses aggregation.
/// Maps pre-projection expressions (like `a.name`) to their post-aggregation aliases
/// (like `name` if the WITH has `a.name AS name`). Also handles aggregate functions.
pub(crate) fn rewrite_with_order_expr(expr: &Expr, with_items: &[ReturnItem]) -> Expr {
    // First check if the expression exactly matches a WITH item's expression.
    for ri in with_items {
        if *expr == ri.expr {
            let alias = ri
                .alias
                .clone()
                .map(|a| a.0)
                .unwrap_or_else(|| return_item_name(ri));
            return Expr::Variable(Identifier::new(alias));
        }
    }
    // For compound expressions, try to rewrite sub-expressions.
    // E.g., `a.name + 'C'` where `a.name AS name` -> `name + 'C'`.
    match expr {
        Expr::BinaryOp { op, left, right } => {
            let new_left = rewrite_with_order_expr(left, with_items);
            let new_right = rewrite_with_order_expr(right, with_items);
            Expr::BinaryOp {
                op: *op,
                left: Box::new(new_left),
                right: Box::new(new_right),
            }
        }
        Expr::FunctionCall {
            name,
            args: _,
            distinct: _,
        } => {
            if is_agg_name(name.as_str()) {
                // Map aggregate functions to their output aliases.
                for ri in with_items {
                    if let Expr::FunctionCall { name: rn, .. } = &ri.expr {
                        if name == rn {
                            let alias = ri
                                .alias
                                .clone()
                                .map(|a| a.0)
                                .unwrap_or_else(|| return_item_name(ri));
                            return Expr::Variable(Identifier::new(alias));
                        }
                    }
                }
            }
            expr.clone()
        }
        _ => expr.clone(),
    }
}

/// Rewrite an ORDER BY expression when aggregation is active.
/// If the expression is an aggregate function call that matches a RETURN item,
/// replace it with a variable reference to the output alias.
pub(crate) fn rewrite_agg_order_expr(expr: &Expr, return_items: &[ReturnItem]) -> Expr {
    // After aggregation, ORDER BY expressions must reference output column names.
    // Try to find the matching RETURN item and rewrite to Variable(output_column_name).
    let expr_col = expr_to_col_name(expr);
    for ri in return_items {
        let ri_col = expr_to_col_name(&ri.expr);
        let ri_alias = return_item_name(ri);
        // Match if the ORDER BY expression equals the return item expression,
        // or if it matches the alias.
        if expr_col == ri_col || expr_col == ri_alias {
            return Expr::Variable(Identifier::new(ri_alias));
        }
    }
    // For compound expressions (e.g. `$age + avg(person.age) - 1000`), recursively
    // rewrite sub-expressions that match return items, replacing agg calls with aliases.
    match expr {
        Expr::BinaryOp { op, left, right } => Expr::BinaryOp {
            op: *op,
            left: Box::new(rewrite_agg_order_expr(left, return_items)),
            right: Box::new(rewrite_agg_order_expr(right, return_items)),
        },
        Expr::UnaryOp { op, expr: inner } => Expr::UnaryOp {
            op: *op,
            expr: Box::new(rewrite_agg_order_expr(inner, return_items)),
        },
        _ => expr.clone(),
    }
}
