//! `list[i]` and `map[key]` subscript evaluation.

use crate::ast::Expr;
use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

use super::super::{eval_expr, Row};

pub(in crate::executor::filter) fn eval_subscript<'r>(
    list_expr: &Expr,
    index_expr: &Expr,
    row: &Row<'r>,
) -> Result<Value, GqlError> {
    let list = eval_expr(list_expr, row)?;
    let idx = eval_expr(index_expr, row)?;
    match (list, idx) {
        (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
        (Value::List(items), Value::Int(i)) => {
            let len = items.len();
            // Cypher: negative indices count from the end.
            // Out-of-bounds returns null in Cypher.
            let actual = if i < 0 {
                let offset = (-i) as usize;
                if offset > len {
                    return Ok(Value::Null);
                }
                len - offset
            } else {
                i as usize
            };
            if actual >= len {
                return Ok(Value::Null);
            }
            Ok(items[actual].clone())
        }
        // Map[String] — dynamic property access: map['key']
        (Value::Map(m), Value::Str(key)) => Ok(m.get(key.as_ref()).cloned().unwrap_or(Value::Null)),
        (list, idx) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "List[Integer] or Map[String]".to_string(),
            actual: format!("{}[{}]", list.type_name(), idx.type_name()),
        })),
    }
}
