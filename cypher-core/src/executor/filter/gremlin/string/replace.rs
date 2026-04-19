//! Replace gremlin string functions: replace (+ local variant).

use super::super::super::eval_function_with_vals;
use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

pub(crate) fn replace(vals: Vec<Value>) -> Result<Value, GqlError> {
    match (vals.first(), vals.get(1), vals.get(2)) {
        (Some(Value::Str(s)), Some(Value::Str(old)), Some(Value::Str(new))) => {
            Ok(Value::Str(s.replace(old.as_ref(), new.as_ref()).into()))
        }
        (Some(Value::Null), _, _) => Ok(Value::Null),
        (Some(Value::List(_)), _, _) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "String".into(),
            actual: "List".into(),
        })),
        _ => Ok(Value::Null),
    }
}

pub(crate) fn replace_local(vals: Vec<Value>) -> Result<Value, GqlError> {
    match (vals.first(), vals.get(1), vals.get(2)) {
        (Some(Value::List(items)), Some(Value::Str(old)), Some(Value::Str(new))) => {
            let results: Vec<Value> = items
                .iter()
                .map(|v| match v {
                    Value::Str(s) => Value::Str(s.replace(old.as_ref(), new.as_ref()).into()),
                    Value::Null => Value::Null,
                    other => other.clone(),
                })
                .collect();
            Ok(Value::List(results))
        }
        _ => eval_function_with_vals("gremlin_replace", vals),
    }
}
