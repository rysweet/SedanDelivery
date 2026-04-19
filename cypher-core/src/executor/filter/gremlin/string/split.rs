//! Split gremlin string functions: split (+ local variant).

use super::super::super::eval_function_with_vals;
use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

pub(crate) fn split(vals: Vec<Value>) -> Result<Value, GqlError> {
    match (vals.first(), vals.get(1)) {
        (Some(Value::Str(s)), Some(Value::Str(sep))) => {
            let parts: Vec<Value> = if sep.is_empty() {
                // Empty separator: split into individual characters
                s.chars()
                    .map(|c| Value::Str(c.to_string().into()))
                    .collect()
            } else if sep.as_ref() == "null" {
                // null separator: split by whitespace
                s.split_whitespace().map(|p| Value::Str(p.into())).collect()
            } else {
                s.split(sep.as_ref())
                    .map(|p| Value::Str(p.into()))
                    .collect()
            };
            Ok(Value::List(parts))
        }
        (Some(Value::Null), _) => Ok(Value::Null),
        (Some(Value::List(_)), _) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "String".into(),
            actual: "List".into(),
        })),
        _ => Ok(Value::Null),
    }
}

pub(crate) fn split_local(vals: Vec<Value>) -> Result<Value, GqlError> {
    match (vals.first(), vals.get(1)) {
        (Some(Value::List(items)), Some(Value::Str(sep))) => {
            let results: Vec<Value> = items
                .iter()
                .map(|v| match v {
                    Value::Str(s) => {
                        let parts: Vec<Value> = if sep.is_empty() {
                            s.chars()
                                .map(|c| Value::Str(c.to_string().into()))
                                .collect()
                        } else {
                            s.split(sep.as_ref())
                                .map(|p| Value::Str(p.into()))
                                .collect()
                        };
                        Value::List(parts)
                    }
                    Value::Null => Value::Null,
                    other => other.clone(),
                })
                .collect();
            Ok(Value::List(results))
        }
        _ => eval_function_with_vals("gremlin_split", vals),
    }
}
