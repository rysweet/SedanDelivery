//! Derive output column names from expressions and RETURN items.

use crate::ast::{Expr, ReturnItem};

/// Return the output column name for a RETURN item.
pub(crate) fn return_item_name(item: &ReturnItem) -> String {
    if let Some(alias) = &item.alias {
        return alias.0.clone();
    }
    expr_to_col_name(&item.expr)
}

fn binop_col_prec(op: &crate::ast::BinaryOp) -> i32 {
    use crate::ast::BinaryOp;
    match op {
        BinaryOp::Pow => 6,
        BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod => 5,
        BinaryOp::Add | BinaryOp::Sub | BinaryOp::Concat => 4,
        _ => 3,
    }
}

fn binop_col_nonassoc(op: &crate::ast::BinaryOp) -> bool {
    use crate::ast::BinaryOp;
    matches!(op, BinaryOp::Sub | BinaryOp::Div | BinaryOp::Mod)
}

/// Derive a column name from an expression (mirrors executor's `expr_to_column_name`).
pub(crate) fn expr_to_col_name(expr: &Expr) -> String {
    match expr {
        Expr::Variable(id) => id.0.clone(),
        Expr::Property(obj, key) => {
            let obj_str = match obj.as_ref() {
                Expr::Subscript(_, _) => format!("({})", expr_to_col_name(obj)),
                _ => expr_to_col_name(obj),
            };
            format!("{}.{}", obj_str, key)
        }
        Expr::FunctionCall {
            name,
            args,
            distinct,
            ..
        } => {
            let arg_strs: Vec<String> = args.iter().map(expr_to_col_name).collect();
            if *distinct {
                format!("{}(DISTINCT {})", name, arg_strs.join(", "))
            } else {
                format!("{}({})", name, arg_strs.join(", "))
            }
        }
        Expr::Integer(n) => n.to_string(),
        Expr::Float(f) => f.to_string(),
        Expr::Str(s) => format!("'{s}'"),
        Expr::BinaryOp { op, left, right } => {
            let op_str = match op {
                crate::ast::BinaryOp::Add => "+",
                crate::ast::BinaryOp::Sub => "-",
                crate::ast::BinaryOp::Mul => "*",
                crate::ast::BinaryOp::Div => "/",
                crate::ast::BinaryOp::Mod => "%",
                crate::ast::BinaryOp::Eq => "=",
                crate::ast::BinaryOp::Neq => "<>",
                crate::ast::BinaryOp::Lt => "<",
                crate::ast::BinaryOp::Lte => "<=",
                crate::ast::BinaryOp::Gt => ">",
                crate::ast::BinaryOp::Gte => ">=",
                crate::ast::BinaryOp::And => "AND",
                crate::ast::BinaryOp::Or => "OR",
                crate::ast::BinaryOp::Xor => "XOR",
                crate::ast::BinaryOp::In => "IN",
                crate::ast::BinaryOp::Contains => "CONTAINS",
                crate::ast::BinaryOp::StartsWith => "STARTS WITH",
                crate::ast::BinaryOp::EndsWith => "ENDS WITH",
                crate::ast::BinaryOp::Pow => "^",
                crate::ast::BinaryOp::RegexMatch => "=~",
                crate::ast::BinaryOp::Concat => "++",
            };
            let parent_prec = binop_col_prec(op);
            let left_str = match left.as_ref() {
                Expr::BinaryOp { op: child_op, .. } if binop_col_prec(child_op) < parent_prec => {
                    format!("({})", expr_to_col_name(left))
                }
                _ => expr_to_col_name(left),
            };
            let right_str = match right.as_ref() {
                Expr::BinaryOp { op: child_op, .. }
                    if binop_col_prec(child_op) < parent_prec
                        || (binop_col_prec(child_op) == parent_prec && binop_col_nonassoc(op)) =>
                {
                    format!("({})", expr_to_col_name(right))
                }
                _ => expr_to_col_name(right),
            };
            format!("{} {} {}", left_str, op_str, right_str)
        }
        Expr::Bool(b) => b.to_string(),
        Expr::Null => "null".to_string(),
        Expr::HasLabel(e, label) => format!("{}:{}", expr_to_col_name(e), label.0),
        Expr::UnaryOp { op, expr: e } => match op {
            crate::ast::UnaryOp::Not => format!("NOT {}", expr_to_col_name(e)),
            crate::ast::UnaryOp::Neg => format!("-{}", expr_to_col_name(e)),
        },
        Expr::IsNull(e) => format!("{} IS NULL", expr_to_col_name(e)),
        Expr::IsNotNull(e) => format!("{} IS NOT NULL", expr_to_col_name(e)),
        Expr::MapLiteral(pairs) => {
            let pairs_str: Vec<String> = pairs
                .iter()
                .map(|(k, v)| format!("{}: {}", k, expr_to_col_name_quoted(v)))
                .collect();
            format!("{{{}}}", pairs_str.join(", "))
        }
        Expr::List(items) => {
            let items_str: Vec<String> = items.iter().map(expr_to_col_name).collect();
            format!("[{}]", items_str.join(", "))
        }
        Expr::Subscript(list, idx) => {
            format!("{}[{}]", expr_to_col_name(list), expr_to_col_name(idx))
        }
        Expr::Parameter(name) => format!("${name}"),
        Expr::ListComp {
            variable,
            list,
            filter,
            map,
        } => {
            let list_str = expr_to_col_name(list);
            let var_str = &variable.0;
            if let Some(f) = filter {
                let filter_str = expr_to_col_name(f);
                if **map != Expr::Variable(variable.clone()) {
                    format!(
                        "[{} IN {} WHERE {} | {}]",
                        var_str,
                        list_str,
                        filter_str,
                        expr_to_col_name(map)
                    )
                } else {
                    format!("[{} IN {} WHERE {}]", var_str, list_str, filter_str)
                }
            } else {
                format!("[{} IN {}]", var_str, list_str)
            }
        }
        _ => format!("{:?}", expr),
    }
}

/// Like `expr_to_col_name` but quotes string literals (for map literal values).
fn expr_to_col_name_quoted(expr: &Expr) -> String {
    match expr {
        Expr::Str(s) => format!("'{s}'"),
        _ => expr_to_col_name(expr),
    }
}
