//! Literal-expression conversion to runtime `Value`.

use crate::ast::Expr;
use crate::types::Value;

/// Convert a literal expression to a Value for type checking.
pub(crate) fn expr_to_literal_value(expr: &Expr) -> Option<Value> {
    match expr {
        Expr::Integer(n) => Some(Value::Int(*n)),
        Expr::Float(f) => Some(Value::Float(*f)),
        Expr::Str(s) => Some(Value::Str(s.clone())),
        Expr::Bool(b) => Some(Value::Bool(*b)),
        Expr::Null => Some(Value::Null),
        _ => None, // Non-literal expressions can't be type-checked at plan time
    }
}
