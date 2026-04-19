//! Whitespace trim gremlin string functions: trim / ltrim / rtrim (+ local variants).

use super::super::super::eval_function_with_vals;
use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

pub(crate) fn trim(vals: Vec<Value>) -> Result<Value, GqlError> {
    match vals.first() {
        Some(Value::Str(s)) => Ok(Value::Str(s.trim().into())),
        Some(Value::Null) => Ok(Value::Null),
        Some(Value::List(_)) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "String".into(),
            actual: "List".into(),
        })),
        _ => Ok(Value::Null),
    }
}

pub(crate) fn ltrim(vals: Vec<Value>) -> Result<Value, GqlError> {
    match vals.first() {
        Some(Value::Str(s)) => Ok(Value::Str(s.trim_start().into())),
        Some(Value::Null) => Ok(Value::Null),
        Some(Value::List(_)) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "String".into(),
            actual: "List".into(),
        })),
        _ => Ok(Value::Null),
    }
}

pub(crate) fn rtrim(vals: Vec<Value>) -> Result<Value, GqlError> {
    match vals.first() {
        Some(Value::Str(s)) => Ok(Value::Str(s.trim_end().into())),
        Some(Value::Null) => Ok(Value::Null),
        Some(Value::List(_)) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "String".into(),
            actual: "List".into(),
        })),
        _ => Ok(Value::Null),
    }
}

pub(crate) fn trim_local(vals: Vec<Value>) -> Result<Value, GqlError> {
    match vals.first() {
        Some(Value::List(items)) => {
            for v in items {
                if !matches!(v, Value::Str(_) | Value::Null) {
                    return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: "The trim(local) step can only take string or list of strings"
                            .into(),
                        actual: v.type_name().into(),
                    }));
                }
            }
            let results: Vec<Value> = items
                .iter()
                .map(|v| match v {
                    Value::Str(s) => Value::Str(s.trim().into()),
                    _ => Value::Null,
                })
                .collect();
            Ok(Value::List(results))
        }
        _ => eval_function_with_vals("gremlin_trim", vals),
    }
}

pub(crate) fn ltrim_local(vals: Vec<Value>) -> Result<Value, GqlError> {
    match vals.first() {
        Some(Value::List(items)) => {
            for v in items {
                if !matches!(v, Value::Str(_) | Value::Null) {
                    return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: "The lTrim(local) step can only take string or list of strings"
                            .into(),
                        actual: v.type_name().into(),
                    }));
                }
            }
            let results: Vec<Value> = items
                .iter()
                .map(|v| match v {
                    Value::Str(s) => Value::Str(s.trim_start().into()),
                    _ => Value::Null,
                })
                .collect();
            Ok(Value::List(results))
        }
        _ => eval_function_with_vals("gremlin_ltrim", vals),
    }
}

pub(crate) fn rtrim_local(vals: Vec<Value>) -> Result<Value, GqlError> {
    match vals.first() {
        Some(Value::List(items)) => {
            for v in items {
                if !matches!(v, Value::Str(_) | Value::Null) {
                    return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: "The rTrim(local) step can only take string or list of strings"
                            .into(),
                        actual: v.type_name().into(),
                    }));
                }
            }
            let results: Vec<Value> = items
                .iter()
                .map(|v| match v {
                    Value::Str(s) => Value::Str(s.trim_end().into()),
                    _ => Value::Null,
                })
                .collect();
            Ok(Value::List(results))
        }
        _ => eval_function_with_vals("gremlin_rtrim", vals),
    }
}
