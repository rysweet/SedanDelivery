//! Helpers for extracting date/time/timezone components from temporal values.

use crate::types::Value;

/// Extract a date (year, month, day) from any temporal value.
pub(super) fn extract_date(val: &Value) -> Option<jiff::civil::Date> {
    match val {
        Value::Date(d) => Some(*d),
        Value::LocalDateTime(dt) => Some(dt.date()),
        Value::DateTime(zdt) => Some(jiff::civil::Date::from(&**zdt)),
        _ => None,
    }
}

/// Extract a time (hour, minute, second, nanosecond) from any temporal value.
pub(super) fn extract_time(val: &Value) -> Option<jiff::civil::Time> {
    match val {
        Value::LocalTime(t) => Some(*t),
        Value::Time(t, _) => Some(*t),
        Value::LocalDateTime(dt) => Some(dt.time()),
        Value::DateTime(zdt) => Some(jiff::civil::Time::from(&**zdt)),
        _ => None,
    }
}

/// Extract timezone from a temporal value, if available.
pub(super) fn extract_tz(val: &Value) -> Option<jiff::tz::TimeZone> {
    match val {
        Value::Time(_, offset) => {
            let off = jiff::tz::Offset::from_seconds(*offset).unwrap_or(jiff::tz::Offset::UTC);
            Some(jiff::tz::TimeZone::fixed(off))
        }
        Value::DateTime(zdt) => Some(zdt.time_zone().clone()),
        _ => None,
    }
}
