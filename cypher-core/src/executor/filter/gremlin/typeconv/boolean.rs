//! `asBool` conversion.

use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

pub(crate) fn asbool(vals: Vec<Value>) -> Result<Value, GqlError> {
    match vals.first() {
        Some(Value::Bool(b)) => Ok(Value::Bool(*b)),
        Some(Value::Int(n)) => Ok(Value::Bool(*n != 0)),
        Some(Value::Float(f)) => {
            if f.is_nan() {
                Ok(Value::Bool(false))
            } else {
                Ok(Value::Bool(*f != 0.0))
            }
        }
        Some(Value::Str(s)) => match s.to_lowercase().as_str() {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "boolean-compatible value".into(),
                actual: format!("'{}'", s),
            })),
        },
        Some(Value::Null) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "boolean-compatible value".into(),
            actual: "null".into(),
        })),
        Some(Value::List(_)) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "boolean-compatible value".into(),
            actual: "list".into(),
        })),
        _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "boolean-compatible value".into(),
            actual: "unknown".into(),
        })),
    }
}
