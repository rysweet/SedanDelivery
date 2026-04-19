//! Size / reversal gremlin string functions: length, reverse, count_local, length_local.

use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

pub(crate) fn length(vals: Vec<Value>) -> Result<Value, GqlError> {
    match vals.first() {
        Some(Value::Str(s)) => Ok(Value::Int(s.len() as i64)),
        Some(Value::Null) => Ok(Value::Null),
        Some(other) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "The length() step can only take string as argument".into(),
            actual: other.type_name().into(),
        })),
        _ => Ok(Value::Int(0)),
    }
}

pub(crate) fn reverse(vals: Vec<Value>) -> Result<Value, GqlError> {
    match vals.first() {
        Some(Value::Str(s)) => Ok(Value::Str(s.chars().rev().collect::<String>().into())),
        Some(Value::List(l)) => {
            let mut rev = l.clone();
            rev.reverse();
            Ok(Value::List(rev))
        }
        Some(Value::Null) => Ok(Value::Null),
        // Non-string, non-list types: return the value unchanged (Gremlin semantics)
        Some(other) => Ok(other.clone()),
        None => Ok(Value::Null),
    }
}

pub(crate) fn count_local(vals: Vec<Value>) -> Result<Value, GqlError> {
    match vals.first() {
        Some(Value::List(l)) => {
            // count(local) on a list returns the number of elements.
            Ok(Value::Int(l.len() as i64))
        }
        Some(Value::Map(m)) => Ok(Value::Int(m.len() as i64)),
        Some(Value::Str(s)) => Ok(Value::Int(s.len() as i64)),
        Some(Value::Null) => Ok(Value::Null),
        _ => Ok(Value::Int(0)),
    }
}

pub(crate) fn length_local(vals: Vec<Value>) -> Result<Value, GqlError> {
    match vals.first() {
        Some(Value::List(l)) => {
            // length(local) on a list returns per-element string lengths.
            let lengths: Vec<Value> = l
                .iter()
                .map(|v| match v {
                    Value::Str(s) => Value::Int(s.len() as i64),
                    Value::List(inner) => Value::Int(inner.len() as i64),
                    Value::Map(m) => Value::Int(m.len() as i64),
                    Value::Null => Value::Null,
                    _ => Value::Int(0),
                })
                .collect();
            Ok(Value::List(lengths))
        }
        Some(Value::Map(m)) => Ok(Value::Int(m.len() as i64)),
        Some(Value::Str(s)) => Ok(Value::Int(s.len() as i64)),
        Some(Value::Null) => Ok(Value::Null),
        _ => Ok(Value::Int(0)),
    }
}
