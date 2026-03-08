//! Expression-to-column-name conversion for projections and aggregations.

use crate::ast::Expr;

/// Returns the precedence level of a binary operator for column-name parenthesization.
/// Higher value = higher precedence (binds tighter).
fn binop_col_prec(op: &crate::ast::BinaryOp) -> i32 {
    use crate::ast::BinaryOp;
    match op {
        BinaryOp::Pow => 6,
        BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod => 5,
        BinaryOp::Add | BinaryOp::Sub | BinaryOp::Concat => 4,
        _ => 3,
    }
}

/// True for operators where `a op (b op c) != (a op b) op c`,
/// so the right child needs parentheses when it has the same precedence.
fn binop_col_nonassoc(op: &crate::ast::BinaryOp) -> bool {
    use crate::ast::BinaryOp;
    matches!(op, BinaryOp::Sub | BinaryOp::Div | BinaryOp::Mod)
}

/// Derive a column name string from an expression.
/// `n.name` -> `"n.name"`, `n` -> `"n"`, literal -> `"literal"`.
pub(crate) fn expr_to_column_name(expr: &Expr) -> String {
    match expr {
        Expr::Variable(id) => id.0.clone(),
        Expr::Property(obj, key) => {
            // Subscript expressions as property objects are wrapped in parens
            // to match the query text: `(list[1]).key` -> column `(list[1]).key`.
            let obj_str = match obj.as_ref() {
                Expr::Subscript(_, _) => format!("({})", expr_to_column_name(obj)),
                _ => expr_to_column_name(obj),
            };
            format!("{}.{}", obj_str, key)
        }
        Expr::Integer(n) => n.to_string(),
        Expr::Float(f) => f.to_string(),
        Expr::Str(s) => format!("'{s}'"),
        Expr::Bool(b) => b.to_string(),
        Expr::Null => "null".to_string(),
        Expr::FunctionCall {
            name,
            args,
            distinct,
        } => {
            let arg_strs: Vec<String> = args.iter().map(expr_to_column_name).collect();
            if *distinct {
                format!("{}(DISTINCT {})", name, arg_strs.join(", "))
            } else {
                format!("{}({})", name, arg_strs.join(", "))
            }
        }
        Expr::UnaryOp {
            op: crate::ast::UnaryOp::Neg,
            expr: inner,
        } => format!("-{}", expr_to_column_name(inner)),
        Expr::UnaryOp {
            op: crate::ast::UnaryOp::Not,
            expr: inner,
        } => format!("NOT {}", expr_to_column_name(inner)),
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
            // Left child: wrap if strictly lower precedence
            let left_str = match left.as_ref() {
                Expr::BinaryOp { op: child_op, .. } if binop_col_prec(child_op) < parent_prec => {
                    format!("({})", expr_to_column_name(left))
                }
                _ => expr_to_column_name(left),
            };
            // Right child: wrap if lower precedence, or same prec for non-associative ops
            let right_str = match right.as_ref() {
                Expr::BinaryOp { op: child_op, .. }
                    if binop_col_prec(child_op) < parent_prec
                        || (binop_col_prec(child_op) == parent_prec && binop_col_nonassoc(op)) =>
                {
                    format!("({})", expr_to_column_name(right))
                }
                _ => expr_to_column_name(right),
            };
            format!("{} {} {}", left_str, op_str, right_str)
        }
        Expr::ListComp {
            variable,
            list,
            filter,
            map,
        } => {
            let list_str = expr_to_column_name(list);
            let var_str = &variable.0;
            if let Some(f) = filter {
                let filter_str = expr_to_column_name(f);
                if **map != Expr::Variable(variable.clone()) {
                    format!(
                        "[{} IN {} WHERE {} | {}]",
                        var_str,
                        list_str,
                        filter_str,
                        expr_to_column_name(map)
                    )
                } else {
                    format!("[{} IN {} WHERE {}]", var_str, list_str, filter_str)
                }
            } else {
                format!("[{} IN {}]", var_str, list_str)
            }
        }
        // These must match expr_to_col_name in planner.rs exactly.
        Expr::IsNull(e) => format!("{} IS NULL", expr_to_column_name(e)),
        Expr::IsNotNull(e) => format!("{} IS NOT NULL", expr_to_column_name(e)),
        Expr::HasLabel(e, label) => format!("{}:{}", expr_to_column_name(e), label.0),
        Expr::Parameter(name) => format!("${name}"),
        Expr::List(items) => {
            let items_str: Vec<String> = items.iter().map(expr_to_column_name).collect();
            format!("[{}]", items_str.join(", "))
        }
        Expr::Subscript(list, idx) => {
            format!(
                "{}[{}]",
                expr_to_column_name(list),
                expr_to_column_name(idx)
            )
        }
        _ => format!("{:?}", expr),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Identifier;

    #[test]
    fn expr_to_col_variable() {
        assert_eq!(
            expr_to_column_name(&Expr::Variable(Identifier::new("n"))),
            "n"
        );
    }

    #[test]
    fn expr_to_col_property() {
        let expr = Expr::Property(
            Box::new(Expr::Variable(Identifier::new("n"))),
            "name".into(),
        );
        assert_eq!(expr_to_column_name(&expr), "n.name");
    }

    #[test]
    fn expr_to_col_function() {
        let expr = Expr::FunctionCall {
            name: "count".into(),
            args: vec![Expr::Variable(Identifier::new("n"))],
            distinct: false,
        };
        assert_eq!(expr_to_column_name(&expr), "count(n)");
    }
}
