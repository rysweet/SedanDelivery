//! `datetime.fromEpoch` and `datetime.fromEpochMillis` constructors.

use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

use super::super::super::super::builtins::type_error;
use super::super::super::temporal_parse_error;

pub(crate) fn builtin_datetime_fromepoch(vals: Vec<Value>) -> Result<Value, GqlError> {
    if vals.is_empty() || vals.len() > 2 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "datetime.fromEpoch(seconds[, nanoseconds])".to_string(),
            actual: format!("{} arguments", vals.len()),
        }));
    }
    let secs = match &vals[0] {
        Value::Int(n) => *n,
        other => return type_error("datetime.fromEpoch", other),
    };
    let nanos = if vals.len() == 2 {
        match &vals[1] {
            Value::Int(n) => *n,
            _ => 0,
        }
    } else {
        0
    };
    let ts = jiff::Timestamp::new(secs, nanos as i32)
        .map_err(|e| temporal_parse_error("datetime.fromEpoch", e))?;
    let zoned = ts.to_zoned(jiff::tz::TimeZone::UTC);
    Ok(Value::DateTime(Box::new(zoned)))
}

pub(crate) fn builtin_datetime_fromepochmillis(vals: Vec<Value>) -> Result<Value, GqlError> {
    if vals.len() != 1 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "datetime.fromEpochMillis(millis)".to_string(),
            actual: format!("{} arguments", vals.len()),
        }));
    }
    let millis = match &vals[0] {
        Value::Int(n) => *n,
        other => return type_error("datetime.fromEpochMillis", other),
    };
    let ts = jiff::Timestamp::from_millisecond(millis)
        .map_err(|e| temporal_parse_error("datetime.fromEpochMillis", e))?;
    let zoned = ts.to_zoned(jiff::tz::TimeZone::UTC);
    Ok(Value::DateTime(Box::new(zoned)))
}
