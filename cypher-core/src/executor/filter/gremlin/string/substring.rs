//! Substring gremlin string functions: substring (+ local variant).

use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

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
