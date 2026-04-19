//! Map override application and value conversion helpers for truncation.

use crate::error::GqlError;
use crate::types::Value;

use super::super::temporal_parse_error;

/// Apply map overrides for date fields: day, dayOfWeek.
pub(super) fn apply_date_overrides(
    d: jiff::civil::Date,
    overrides: &indexmap::IndexMap<Box<str>, Value>,
) -> Result<jiff::civil::Date, GqlError> {
    let mut result = d;
    if let Some(Value::Int(day)) = overrides.get("day") {
        result = jiff::civil::Date::new(result.year(), result.month(), *day as i8)
            .map_err(|e| temporal_parse_error("truncate", e))?;
    }
    if let Some(Value::Int(dow)) = overrides.get("dayOfWeek") {
        // Current day-of-week and desired day-of-week (1=Monday)
        let current_dow = result.weekday().to_monday_one_offset() as i64;
        let offset = *dow - current_dow;
        result = result
            .checked_add(jiff::Span::new().days(offset))
            .map_err(|e| temporal_parse_error("truncate", e))?;
    }
    Ok(result)
}

/// Apply map overrides for time fields: hour, minute, second, millisecond, microsecond, nanosecond.
pub(super) fn apply_time_overrides(
    t: jiff::civil::Time,
    overrides: &indexmap::IndexMap<Box<str>, Value>,
) -> Result<jiff::civil::Time, GqlError> {
    let hour = if let Some(Value::Int(v)) = overrides.get("hour") {
        *v as i8
    } else {
        t.hour()
    };
    let minute = if let Some(Value::Int(v)) = overrides.get("minute") {
        *v as i8
    } else {
        t.minute()
    };
    let second = if let Some(Value::Int(v)) = overrides.get("second") {
        *v as i8
    } else {
        t.second()
    };
    // Sub-second components are hierarchical: millisecond > microsecond > nanosecond
    let base_ns = t.subsec_nanosecond();
    let base_ms = base_ns / 1_000_000;
    let base_us = (base_ns % 1_000_000) / 1_000;
    let base_ns_only = base_ns % 1_000;

    let ms = if let Some(Value::Int(v)) = overrides.get("millisecond") {
        *v as i32
    } else {
        base_ms
    };
    let us = if let Some(Value::Int(v)) = overrides.get("microsecond") {
        *v as i32
    } else if overrides.contains_key("millisecond") {
        0
    } else {
        base_us
    };
    let ns_only = if let Some(Value::Int(v)) = overrides.get("nanosecond") {
        *v as i32
    } else if overrides.contains_key("microsecond") || overrides.contains_key("millisecond") {
        0
    } else {
        base_ns_only
    };
    let nanosecond = ms * 1_000_000 + us * 1_000 + ns_only;
    jiff::civil::Time::new(hour, minute, second, nanosecond)
        .map_err(|e| temporal_parse_error("truncate", e))
}

/// Convert a jiff::Zoned (at epoch date) to Value::Time.
pub(super) fn zoned_to_time_value(z: &jiff::Zoned) -> Value {
    let dt = jiff::civil::DateTime::from(z);
    let t = dt.time();
    Value::Time(t, z.offset().seconds())
}
