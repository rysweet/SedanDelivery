//! `list[start..end]` slice evaluation.

use crate::ast::Expr;
use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

use super::super::{eval_expr, Row};

pub(in crate::executor::filter) fn eval_slice<'r>(
    list_expr: &Expr,
    start_expr: &Option<Box<Expr>>,
    end_expr: &Option<Box<Expr>>,
    row: &Row<'r>,
) -> Result<Value, GqlError> {
    let list = eval_expr(list_expr, row)?;
    match list {
        Value::Null => Ok(Value::Null),
        Value::List(items) => {
            let len = items.len() as i64;
            // Cypher: null slice bounds → null result
            let start = match start_expr {
                Some(e) => match eval_expr(e, row)? {
                    Value::Int(i) => {
                        // Negative indices count from end
                        let idx = if i < 0 { (len + i).max(0) } else { i.min(len) };
                        idx as usize
                    }
                    Value::Null => return Ok(Value::Null),
                    other => {
                        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: "Integer".to_string(),
                            actual: other.type_name().to_string(),
                        }))
                    }
                },
                None => 0,
            };
            let end = match end_expr {
                Some(e) => match eval_expr(e, row)? {
                    Value::Int(i) => {
                        let idx = if i < 0 { (len + i).max(0) } else { i.min(len) };
                        idx as usize
                    }
                    Value::Null => return Ok(Value::Null),
                    other => {
                        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: "Integer".to_string(),
                            actual: other.type_name().to_string(),
                        }))
                    }
                },
                None => len as usize,
            };
            // Invalid range (start > end) → empty list
            if start >= end {
                Ok(Value::List(vec![]))
            } else {
                Ok(Value::List(items[start..end.min(items.len())].to_vec()))
            }
        }
        other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "List".to_string(),
            actual: other.type_name().to_string(),
        })),
    }
}
