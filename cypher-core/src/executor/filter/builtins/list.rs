//! List builtins: head, last, tail, reverse, range.

use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

use super::super::max_list_size;
use super::helpers::{require_arity, type_error};

pub(in crate::executor::filter) fn builtin_head(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("head", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::List(l) => Ok(l.first().cloned().unwrap_or(Value::Null)),
        other => type_error("head", other),
    }
}

pub(in crate::executor::filter) fn builtin_last(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("last", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::List(l) => Ok(l.last().cloned().unwrap_or(Value::Null)),
        other => type_error("last", other),
    }
}

pub(in crate::executor::filter) fn builtin_tail(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("tail", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::List(l) => Ok(Value::List(l[1.min(l.len())..].to_vec())),
        other => type_error("tail", other),
    }
}

pub(in crate::executor::filter) fn builtin_reverse(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("reverse", 1, &vals)?;
    // safe: require_arity(1) above guarantees at least one element
    match vals.into_iter().next().expect("arity checked") {
        Value::Null => Ok(Value::Null),
        Value::List(mut l) => {
            l.reverse();
            Ok(Value::List(l))
        }
        Value::Str(s) => Ok(Value::Str(
            s.chars().rev().collect::<String>().into_boxed_str(),
        )),
        other => type_error("reverse", &other),
    }
}

pub(in crate::executor::filter) fn builtin_range(vals: Vec<Value>) -> Result<Value, GqlError> {
    if vals.len() < 2 || vals.len() > 3 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "range(Integer, Integer[, Integer])".to_string(),
            actual: format!("{} arguments", vals.len()),
        }));
    }
    match (&vals[0], &vals[1]) {
        (Value::Int(start), Value::Int(end)) => {
            let step = if vals.len() == 3 {
                match &vals[2] {
                    Value::Int(s) => *s,
                    other => return type_error("range", other),
                }
            } else {
                1
            };
            if step == 0 {
                return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "non-zero step for range()".to_string(),
                    actual: "0".to_string(),
                }));
            }
            let list_limit = max_list_size();
            let mut result = Vec::new();
            let mut i = *start;
            if step > 0 {
                while i <= *end {
                    if result.len() >= list_limit {
                        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: format!("range() result <= {list_limit} elements"),
                            actual: format!("would exceed {list_limit} elements"),
                        }));
                    }
                    result.push(Value::Int(i));
                    i = i.checked_add(step).ok_or_else(|| {
                        GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: "range() step addition within i64 bounds".to_string(),
                            actual: "integer overflow".to_string(),
                        })
                    })?;
                }
            } else {
                while i >= *end {
                    if result.len() >= list_limit {
                        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: format!("range() result <= {list_limit} elements"),
                            actual: format!("would exceed {list_limit} elements"),
                        }));
                    }
                    result.push(Value::Int(i));
                    i = i.checked_add(step).ok_or_else(|| {
                        GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: "range() step addition within i64 bounds".to_string(),
                            actual: "integer overflow".to_string(),
                        })
                    })?;
                }
            }
            Ok(Value::List(result))
        }
        (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
        _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "Integer, Integer".to_string(),
            actual: format!("{}, {}", vals[0].type_name(), vals[1].type_name()),
        })),
    }
}
