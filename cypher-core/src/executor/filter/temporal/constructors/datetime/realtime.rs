//! `datetime()` and `localdatetime()` zero-arg "now" entry points used as
//! built-in functions, including delegation to the full constructor when
//! arguments are supplied.

use crate::error::GqlError;
use crate::types::Value;

use super::super::super::super::get_query_now;
use super::datetime_constructor::eval_datetime_constructor;
use super::localdatetime_constructor::eval_localdatetime_constructor;

pub(crate) fn builtin_temporal_realtime_datetime(vals: Vec<Value>) -> Result<Value, GqlError> {
    if vals.first().map(|v| v.is_null()).unwrap_or(false) {
        return Ok(Value::Null);
    }
    if !vals.is_empty() {
        return eval_datetime_constructor(&vals);
    }
    Ok(Value::DateTime(Box::new(get_query_now())))
}

pub(crate) fn builtin_temporal_realtime_localdatetime(vals: Vec<Value>) -> Result<Value, GqlError> {
    if vals.first().map(|v| v.is_null()).unwrap_or(false) {
        return Ok(Value::Null);
    }
    if !vals.is_empty() {
        return eval_localdatetime_constructor(&vals);
    }
    let now = get_query_now();
    Ok(Value::LocalDateTime(jiff::civil::DateTime::from(now)))
}
