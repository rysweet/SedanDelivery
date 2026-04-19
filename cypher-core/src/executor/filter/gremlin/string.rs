//! Gremlin string functions extracted from filter::eval_function_with_vals.
//!
//! Behavior preserved verbatim from the original `filter` module.

#![allow(clippy::needless_pass_by_value)]
#![allow(unused_imports)]

use indexmap::IndexMap;

use super::super::eval_function_with_vals;
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

pub(crate) fn substring(vals: Vec<Value>) -> Result<Value, GqlError> {
    match vals.first() {
        Some(Value::Str(s)) => {
            let chars: Vec<char> = s.chars().collect();
            let len = chars.len() as i64;
            let raw_start = match vals.get(1) {
                Some(Value::Int(n)) => *n,
                _ => 0,
            };
            // Resolve negative start: negative means from end
            let start = if raw_start < 0 {
                (len + raw_start).max(0) as usize
            } else {
                (raw_start as usize).min(chars.len())
            };
            if start >= chars.len() {
                return Ok(Value::Str("".into()));
            }
            let result = match vals.get(2) {
                Some(Value::Int(raw_end)) => {
                    // Gremlin: second arg is END INDEX (not length)
                    let re = *raw_end;
                    if re < 0 {
                        // Negative end: end = len + raw_end
                        let end = ((len + re) as usize).min(chars.len());
                        if end <= start {
                            String::new()
                        } else {
                            chars[start..end].iter().collect::<String>()
                        }
                    } else {
                        let end = (re as usize).min(chars.len());
                        if end <= start {
                            String::new()
                        } else {
                            chars[start..end].iter().collect::<String>()
                        }
                    }
                }
                _ => chars[start..].iter().collect::<String>(),
            };
            Ok(Value::Str(result.into()))
        }
        Some(Value::Null) => Ok(Value::Null),
        Some(Value::List(_)) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "The substring() step can only take string as argument".into(),
            actual: "list".into(),
        })),
        _ => Ok(Value::Null),
    }
}

pub(crate) fn substring_local(vals: Vec<Value>) -> Result<Value, GqlError> {
    {
        // Apply substring to each element in a list
        let list = match vals.first() {
            Some(Value::List(items)) => items,
            _ => return Ok(Value::Null),
        };
        let start = match vals.get(1) {
            Some(Value::Int(n)) => *n,
            _ => 0,
        };
        let end = match vals.get(2) {
            Some(Value::Int(n)) => Some(*n),
            _ => None,
        };
        let results: Vec<Value> = list
            .iter()
            .map(|v| {
                if let Value::Str(s) = v {
                    let chars: Vec<char> = s.chars().collect();
                    let len = chars.len() as i64;
                    let s_idx = if start < 0 {
                        (len + start).max(0) as usize
                    } else {
                        (start as usize).min(chars.len())
                    };
                    let result = match end {
                        Some(e) => {
                            let e_idx = if e < 0 {
                                ((len + e) as usize).min(chars.len())
                            } else {
                                (e as usize).min(chars.len())
                            };
                            if e_idx <= s_idx {
                                String::new()
                            } else {
                                chars[s_idx..e_idx].iter().collect()
                            }
                        }
                        None => chars[s_idx..].iter().collect(),
                    };
                    Value::Str(result.into())
                } else {
                    v.clone()
                }
            })
            .collect();
        Ok(Value::List(results))
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

pub(crate) fn trim_local(vals: Vec<Value>) -> Result<Value, GqlError> {
    match vals.first() {
        Some(Value::List(items)) => {
            // Validate all elements are String or Null
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
