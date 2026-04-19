//! Case-conversion gremlin string functions: tolower / toupper (+ local variants).

use super::super::super::eval_function_with_vals;
use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

pub(crate) fn tolower(vals: Vec<Value>) -> Result<Value, GqlError> {
    match vals.first() {
        Some(Value::Str(s)) => Ok(Value::Str(s.to_lowercase().into())),
        Some(Value::Null) => Ok(Value::Null),
        Some(Value::List(_)) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "String".into(),
            actual: "List".into(),
        })),
        _ => Ok(Value::Null),
    }
}

pub(crate) fn toupper(vals: Vec<Value>) -> Result<Value, GqlError> {
    match vals.first() {
        Some(Value::Str(s)) => Ok(Value::Str(s.to_uppercase().into())),
        Some(Value::Null) => Ok(Value::Null),
        Some(Value::List(_)) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "String".into(),
            actual: "List".into(),
        })),
        _ => Ok(Value::Null),
    }
}

pub(crate) fn tolower_local(vals: Vec<Value>) -> Result<Value, GqlError> {
    match vals.first() {
        Some(Value::List(items)) => {
            let results: Vec<Value> = items
                .iter()
                .map(|v| match v {
                    Value::Str(s) => Value::Str(s.to_lowercase().into()),
                    Value::Null => Value::Null,
                    other => other.clone(),
                })
                .collect();
            Ok(Value::List(results))
        }
        _ => eval_function_with_vals("gremlin_tolower", vals),
    }
}

pub(crate) fn toupper_local(vals: Vec<Value>) -> Result<Value, GqlError> {
    match vals.first() {
        Some(Value::List(items)) => {
            let results: Vec<Value> = items
                .iter()
                .map(|v| match v {
                    Value::Str(s) => Value::Str(s.to_uppercase().into()),
                    Value::Null => Value::Null,
                    other => other.clone(),
                })
                .collect();
            Ok(Value::List(results))
        }
        _ => eval_function_with_vals("gremlin_toupper", vals),
    }
}
