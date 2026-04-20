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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn literal_int() {
        assert_eq!(
            expr_to_literal_value(&Expr::Integer(42)),
            Some(Value::Int(42))
        );
    }
    #[test]
    fn literal_float() {
        assert_eq!(
            expr_to_literal_value(&Expr::Float(1.5)),
            Some(Value::Float(1.5))
        );
    }
    #[test]
    fn literal_str() {
        assert_eq!(
            expr_to_literal_value(&Expr::Str("x".into())),
            Some(Value::Str("x".into()))
        );
    }
    #[test]
    fn literal_bool() {
        assert_eq!(
            expr_to_literal_value(&Expr::Bool(true)),
            Some(Value::Bool(true))
        );
    }
    #[test]
    fn literal_null() {
        assert_eq!(expr_to_literal_value(&Expr::Null), Some(Value::Null));
    }
    #[test]
    fn non_literal_returns_none() {
        let e = Expr::Variable(crate::ast::Identifier::new("x"));
        assert_eq!(expr_to_literal_value(&e), None);
    }
}
