//! String + size/length builtins: size, length, toUpper/toLower, trim/ltrim/rtrim,
//! left/right/substring, replace, split.

use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

use super::super::max_list_size;
use super::helpers::{require_arity, type_error};

pub(in crate::executor::filter) fn builtin_size(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("size", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Str(s) => Ok(Value::Int(s.chars().count() as i64)),
        Value::List(l) => Ok(Value::Int(l.len() as i64)),
        other => type_error("size", other),
    }
}

pub(in crate::executor::filter) fn builtin_length(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("length", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Str(s) => Ok(Value::Int(s.chars().count() as i64)),
        Value::List(items) => {
            let results: Result<Vec<Value>, _> = items
                .iter()
                .map(|v| builtin_length(vec![v.clone()]))
                .collect();
            Ok(Value::List(results?))
        }
        other => type_error("length", other),
    }
}

pub(in crate::executor::filter) fn builtin_toupper(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("toUpper", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Str(s) => Ok(Value::Str(s.to_uppercase().into_boxed_str())),
        Value::List(items) => {
            let results: Result<Vec<Value>, _> = items
                .iter()
                .map(|v| builtin_toupper(vec![v.clone()]))
                .collect();
            Ok(Value::List(results?))
        }
        other => type_error("toUpper", other),
    }
}

pub(in crate::executor::filter) fn builtin_tolower(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("toLower", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Str(s) => Ok(Value::Str(s.to_lowercase().into_boxed_str())),
        Value::List(items) => {
            let results: Result<Vec<Value>, _> = items
                .iter()
                .map(|v| builtin_tolower(vec![v.clone()]))
                .collect();
            Ok(Value::List(results?))
        }
        other => type_error("toLower", other),
    }
}

pub(in crate::executor::filter) fn builtin_trim(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("trim", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Str(s) => Ok(Value::Str(s.trim().to_string().into_boxed_str())),
        Value::List(items) => {
            let results: Result<Vec<Value>, _> = items
                .iter()
                .map(|v| builtin_trim(vec![v.clone()]))
                .collect();
            Ok(Value::List(results?))
        }
        other => type_error("trim", other),
    }
}

pub(in crate::executor::filter) fn builtin_ltrim(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("ltrim", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Str(s) => Ok(Value::Str(s.trim_start().to_string().into_boxed_str())),
        Value::List(items) => {
            let results: Result<Vec<Value>, _> = items
                .iter()
                .map(|v| builtin_ltrim(vec![v.clone()]))
                .collect();
            Ok(Value::List(results?))
        }
        other => type_error("ltrim", other),
    }
}

pub(in crate::executor::filter) fn builtin_rtrim(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("rtrim", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Str(s) => Ok(Value::Str(s.trim_end().to_string().into_boxed_str())),
        Value::List(items) => {
            let results: Result<Vec<Value>, _> = items
                .iter()
                .map(|v| builtin_rtrim(vec![v.clone()]))
                .collect();
            Ok(Value::List(results?))
        }
        other => type_error("rtrim", other),
    }
}

pub(in crate::executor::filter) fn builtin_left(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("left", 2, &vals)?;
    match (&vals[0], &vals[1]) {
        (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
        (Value::Str(s), Value::Int(n)) => {
            let n = (*n).max(0) as usize;
            Ok(Value::Str(
                s.chars().take(n).collect::<String>().into_boxed_str(),
            ))
        }
        _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "String, Integer".to_string(),
            actual: format!("{}, {}", vals[0].type_name(), vals[1].type_name()),
        })),
    }
}

pub(in crate::executor::filter) fn builtin_right(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("right", 2, &vals)?;
    match (&vals[0], &vals[1]) {
        (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
        (Value::Str(s), Value::Int(n)) => {
            let n = (*n).max(0) as usize;
            let chars: Vec<char> = s.chars().collect();
            let start = chars.len().saturating_sub(n);
            Ok(Value::Str(
                chars[start..].iter().collect::<String>().into_boxed_str(),
            ))
        }
        _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "String, Integer".to_string(),
            actual: format!("{}, {}", vals[0].type_name(), vals[1].type_name()),
        })),
    }
}

pub(in crate::executor::filter) fn builtin_substring(vals: Vec<Value>) -> Result<Value, GqlError> {
    if vals.len() < 2 || vals.len() > 3 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "substring(String, Integer[, Integer])".to_string(),
            actual: format!("{} arguments", vals.len()),
        }));
    }
    match (&vals[0], &vals[1]) {
        (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
        (Value::Str(s), Value::Int(start)) => {
            let chars: Vec<char> = s.chars().collect();
            let len = chars.len();
            let start = (*start).max(0) as usize;
            let end = if vals.len() == 3 {
                match &vals[2] {
                    Value::Int(n) => (start + (*n).max(0) as usize).min(len),
                    Value::Null => len,
                    other => return type_error("substring", other),
                }
            } else {
                len
            };
            Ok(Value::Str(
                chars[start.min(len)..end.min(len)]
                    .iter()
                    .collect::<String>()
                    .into_boxed_str(),
            ))
        }
        _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "String, Integer".to_string(),
            actual: format!("{}, {}", vals[0].type_name(), vals[1].type_name()),
        })),
    }
}

pub(in crate::executor::filter) fn builtin_replace(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("replace", 3, &vals)?;
    match (&vals[0], &vals[1], &vals[2]) {
        (Value::Null, _, _) | (_, Value::Null, _) | (_, _, Value::Null) => Ok(Value::Null),
        (Value::Str(s), Value::Str(from), Value::Str(to)) => Ok(Value::Str(
            s.replace(from.as_ref(), to.as_ref()).into_boxed_str(),
        )),
        (Value::List(items), from, to) => {
            let results: Result<Vec<Value>, _> = items
                .iter()
                .map(|v| builtin_replace(vec![v.clone(), from.clone(), to.clone()]))
                .collect();
            Ok(Value::List(results?))
        }
        _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "String, String, String".to_string(),
            actual: format!(
                "{}, {}, {}",
                vals[0].type_name(),
                vals[1].type_name(),
                vals[2].type_name()
            ),
        })),
    }
}

pub(in crate::executor::filter) fn builtin_split(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("split", 2, &vals)?;
    match (&vals[0], &vals[1]) {
        (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
        (Value::Str(s), Value::Str(delim)) => {
            let parts: Vec<Value> = s
                .split(delim.as_ref())
                .take(max_list_size())
                .map(|p| Value::Str(p.to_string().into_boxed_str()))
                .collect();
            Ok(Value::List(parts))
        }
        (Value::List(items), delim) => {
            let results: Result<Vec<Value>, _> = items
                .iter()
                .map(|v| builtin_split(vec![v.clone(), delim.clone()]))
                .collect();
            Ok(Value::List(results?))
        }
        _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "String, String".to_string(),
            actual: format!("{}, {}", vals[0].type_name(), vals[1].type_name()),
        })),
    }
}
