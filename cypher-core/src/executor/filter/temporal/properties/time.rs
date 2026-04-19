//! Property access for `Value::LocalTime` and `Value::Time`, plus the
//! `time_to_utc_nanos` helper shared with the arithmetic module.

use crate::error::GqlError;
use crate::types::Value;

pub(super) fn local_time_property(t: &jiff::civil::Time, key: &str) -> Result<Value, GqlError> {
    match key {
        "hour" => Ok(Value::Int(t.hour() as i64)),
        "minute" => Ok(Value::Int(t.minute() as i64)),
        "second" => Ok(Value::Int(t.second() as i64)),
        "millisecond" => Ok(Value::Int((t.subsec_nanosecond() / 1_000_000) as i64)),
        "microsecond" => Ok(Value::Int((t.subsec_nanosecond() / 1_000) as i64)),
        "nanosecond" => Ok(Value::Int(t.subsec_nanosecond() as i64)),
        _ => Ok(Value::Null),
    }
}

pub(super) fn time_property(
    t: &jiff::civil::Time,
    offset_secs: i32,
    key: &str,
) -> Result<Value, GqlError> {
    match key {
        "hour" => Ok(Value::Int(t.hour() as i64)),
        "minute" => Ok(Value::Int(t.minute() as i64)),
        "second" => Ok(Value::Int(t.second() as i64)),
        "millisecond" => Ok(Value::Int((t.subsec_nanosecond() / 1_000_000) as i64)),
        "microsecond" => Ok(Value::Int((t.subsec_nanosecond() / 1_000) as i64)),
        "nanosecond" => Ok(Value::Int(t.subsec_nanosecond() as i64)),
        "offset" => Ok(Value::Str(
            crate::types::value::format_offset_str(offset_secs).into_boxed_str(),
        )),
        "offsetMinutes" => Ok(Value::Int((offset_secs / 60) as i64)),
        "offsetSeconds" => Ok(Value::Int(offset_secs as i64)),
        "timezone" => Ok(Value::Str(
            crate::types::value::format_offset_str(offset_secs).into_boxed_str(),
        )),
        _ => Ok(Value::Null),
    }
}

/// Convert a zoned time (civil time + offset in seconds) to UTC nanoseconds-from-midnight.
pub(crate) fn time_to_utc_nanos(t: &jiff::civil::Time, offset_secs: i32) -> i64 {
    let local_nanos = t.hour() as i64 * 3_600_000_000_000
        + t.minute() as i64 * 60_000_000_000
        + t.second() as i64 * 1_000_000_000
        + t.subsec_nanosecond() as i64;
    local_nanos - offset_secs as i64 * 1_000_000_000
}
