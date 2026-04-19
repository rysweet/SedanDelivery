//! List membership operator: `IN`.

use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

pub(super) fn in_op(lv: Value, rv: Value) -> Result<Value, GqlError> {
    match rv {
        Value::Null => Ok(Value::Null),
        Value::List(items) => {
            if lv.is_null() {
                // null IN [] → false; null IN [any non-empty list] → null
                // Because null == anything is null, so membership is always unknown
                // for non-empty lists.
                if items.is_empty() {
                    Ok(Value::Bool(false))
                } else {
                    Ok(Value::Null)
                }
            } else {
                // x IN list: true if any element equals x; null if null present and not found.
                let mut found_null = false;
                for item in &items {
                    match lv.cypher_eq(item) {
                        Some(true) => return Ok(Value::Bool(true)),
                        None => found_null = true,
                        Some(false) => {}
                    }
                }
                if found_null {
                    Ok(Value::Null)
                } else {
                    Ok(Value::Bool(false))
                }
            }
        }
        other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "List".to_string(),
            actual: other.type_name().to_string(),
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_utils::{bin, lit_i, lit_null, run};
    use crate::ast::{BinaryOp, Expr};
    use crate::types::Value;

    #[test]
    fn in_basic_true() {
        let l = Expr::List(vec![lit_i(1), lit_i(2), lit_i(3)]);
        assert_eq!(
            run(&bin(BinaryOp::In, lit_i(2), l)).unwrap(),
            Value::Bool(true)
        );
    }
    #[test]
    fn in_basic_false() {
        let l = Expr::List(vec![lit_i(1), lit_i(2)]);
        assert_eq!(
            run(&bin(BinaryOp::In, lit_i(99), l)).unwrap(),
            Value::Bool(false)
        );
    }
    #[test]
    fn in_with_null_in_list() {
        let l = Expr::List(vec![lit_i(1), lit_null()]);
        assert_eq!(run(&bin(BinaryOp::In, lit_i(99), l)).unwrap(), Value::Null);
    }
    #[test]
    fn in_null_value() {
        let l_empty = Expr::List(vec![]);
        assert_eq!(
            run(&bin(BinaryOp::In, lit_null(), l_empty)).unwrap(),
            Value::Bool(false)
        );
        let l_nonempty = Expr::List(vec![lit_i(1)]);
        assert_eq!(
            run(&bin(BinaryOp::In, lit_null(), l_nonempty)).unwrap(),
            Value::Null
        );
    }
    #[test]
    fn in_rhs_null() {
        assert_eq!(
            run(&bin(BinaryOp::In, lit_i(1), lit_null())).unwrap(),
            Value::Null
        );
    }
    #[test]
    fn in_rhs_not_list_errs() {
        assert!(run(&bin(BinaryOp::In, lit_i(1), lit_i(1))).is_err());
    }
}
