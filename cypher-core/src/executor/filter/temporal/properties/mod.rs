//! Temporal/spatial property access (`.year`, `.month`, `.day`, ...) and the
//! `time_to_utc_nanos` helper.
//!
//! The dispatcher [`temporal_property_access`] routes by `Value` variant to the
//! corresponding submodule:
//!
//! - [`date`] — `Value::Date`
//! - [`time`] — `Value::LocalTime` and `Value::Time` (also exports
//!   [`time::time_to_utc_nanos`])
//! - [`local_datetime`] — `Value::LocalDateTime`
//! - [`datetime`] — `Value::DateTime` (zoned)
//! - [`duration`] — `Value::Duration`
//! - [`point`] — `Value::Point` (spatial)

use crate::error::GqlError;
use crate::types::Value;

mod date;
mod datetime;
mod duration;
mod local_datetime;
mod point;
mod time;

pub(crate) use self::time::time_to_utc_nanos;

pub fn temporal_property_access(val: &Value, key: &str) -> Result<Value, GqlError> {
    match val {
        Value::Date(d) => date::date_property(d, key),
        Value::LocalTime(t) => time::local_time_property(t, key),
        Value::Time(t, offset_secs) => time::time_property(t, *offset_secs, key),
        Value::LocalDateTime(dt) => local_datetime::local_datetime_property(dt, key),
        Value::DateTime(zdt) => datetime::datetime_property(zdt.as_ref(), key),
        Value::Duration(d) => duration::duration_property(d, key),
        Value::Point(p) => point::point_property(p, key),
        _ => Ok(Value::Null),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_temporal_value_returns_null() {
        assert_eq!(
            temporal_property_access(&Value::Int(42), "year").unwrap(),
            Value::Null
        );
    }
}
