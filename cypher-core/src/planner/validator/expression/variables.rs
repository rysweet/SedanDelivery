//! Variable-reference collection across expressions.

use std::collections::HashSet;

use crate::ast::Expr;

/// Collect all variable names referenced in an expression.
pub(in crate::planner::validator) fn collect_variable_refs(
    expr: &Expr,
    vars: &mut HashSet<String>,
) {
    match expr {
        Expr::Variable(ref id) if id.0 != "*" => {
            vars.insert(id.0.clone());
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
