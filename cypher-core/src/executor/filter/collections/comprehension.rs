//! List comprehension (`[x IN list WHERE pred | map]`) and `reduce(...)`.

use std::collections::HashMap;

use crate::ast::{Expr, Identifier};
use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

use super::super::{eval_expr, Row};

pub(in crate::executor::filter) fn eval_list_comp<'r>(
    variable: &Identifier,
    list: &Expr,
    filter: &Option<Box<Expr>>,
    map: &Expr,
    row: &Row<'r>,
) -> Result<Value, GqlError> {
    let list_val = eval_expr(list, row)?;
    match list_val {
        Value::Null => Ok(Value::Null),
        Value::List(items) => {
            let var_name: &str = variable.0.as_str();
            let mut result = Vec::with_capacity(items.len());
            for item in &items {
                // Build an extended row with the loop variable bound.
                let mut extended: HashMap<&str, &Value> =
                    row.iter().map(|(&k, v)| (k, *v)).collect();
                extended.insert(var_name, item);

                // Evaluate filter if present; skip element on false/null.
                if let Some(pred) = filter {
                    match eval_expr(pred, &extended)? {
                        Value::Bool(true) => {}
                        Value::Bool(false) | Value::Null => continue,
                        other => {
                            return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                                expected: "Boolean".to_string(),
                                actual: other.type_name().to_string(),
                            }))
                        }
                    }
                }

                // Map the surviving element.
                result.push(eval_expr(map, &extended)?);
            }
            Ok(Value::List(result))
        }
        other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "List".to_string(),
            actual: other.type_name().to_string(),
        })),
    }
}

/// Evaluate `reduce(accumulator = initial, variable IN list | expression)`.
///
/// 1. Evaluates `initial` to get the starting accumulator value.
/// 2. Evaluates `list` — must be a Value::List (or Null → return Null).
/// 3. For each element, binds both `accumulator` and `variable` in an extended
///    row and evaluates `expression`. The result becomes the new accumulator.
/// 4. Returns the final accumulator value.
pub(in crate::executor::filter) fn eval_reduce<'r>(
    accumulator: &Identifier,
    initial: &Expr,
    variable: &Identifier,
    list: &Expr,
    expression: &Expr,
    row: &Row<'r>,
) -> Result<Value, GqlError> {
    let init_val = eval_expr(initial, row)?;
    let list_val = eval_expr(list, row)?;
    match list_val {
        Value::Null => Ok(Value::Null),
        Value::List(items) => {
            let acc_name: &str = accumulator.0.as_str();
            let var_name: &str = variable.0.as_str();
            let mut acc = init_val;
            for item in &items {
                let mut extended: HashMap<&str, &Value> =
                    row.iter().map(|(&k, v)| (k, *v)).collect();
                extended.insert(acc_name, &acc);
                extended.insert(var_name, item);
                acc = eval_expr(expression, &extended)?;
            }
            Ok(acc)
        }
        other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "List".to_string(),
            actual: other.type_name().to_string(),
        })),
    }
}
