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

#[cfg(test)]
mod tests {
    use super::super::temporal_property_access;
    use super::time_to_utc_nanos;
    use crate::types::Value;

    #[test]
    fn localtime_props() {
        let t = Value::LocalTime(jiff::civil::Time::new(10, 30, 45, 123_456_789).unwrap());
        assert_eq!(
            temporal_property_access(&t, "hour").unwrap(),
            Value::Int(10)
        );
        assert_eq!(
            temporal_property_access(&t, "nanosecond").unwrap(),
            Value::Int(123456789)
        );
    }

    #[test]
    fn time_offset() {
        let t = Value::Time(jiff::civil::Time::new(10, 30, 0, 0).unwrap(), 3600);
        assert_eq!(
            temporal_property_access(&t, "offset").unwrap(),
            Value::Str("+01:00".into())
        );
    }

    #[test]
    fn localtime_all_keys() {
        let t = Value::LocalTime(jiff::civil::Time::new(10, 30, 45, 123_456_789).unwrap());
        for k in [
            "hour",
            "minute",
            "second",
            "millisecond",
            "microsecond",
            "nanosecond",
        ] {
            assert!(matches!(
                temporal_property_access(&t, k).unwrap(),
                Value::Int(_)
            ));
        }
        assert_eq!(
            temporal_property_access(&t, "unknown").unwrap(),
            Value::Null
        );
    }

    #[test]
    fn time_all_keys() {
        let t = Value::Time(jiff::civil::Time::new(10, 30, 0, 0).unwrap(), 3600);
        for k in [
            "hour",
            "minute",
            "second",
            "millisecond",
            "microsecond",
            "nanosecond",
            "offset",
            "offsetMinutes",
            "offsetSeconds",
            "timezone",
        ] {
            let _ = temporal_property_access(&t, k).unwrap();
        }
        assert_eq!(temporal_property_access(&t, "nope").unwrap(), Value::Null);
    }

    #[test]
    fn utc_nanos_no_offset() {
        assert_eq!(
            time_to_utc_nanos(&jiff::civil::Time::new(0, 0, 0, 0).unwrap(), 0),
            0
        );
    }

    #[test]
    fn utc_nanos_positive() {
        assert_eq!(
            time_to_utc_nanos(&jiff::civil::Time::new(12, 0, 0, 0).unwrap(), 3600),
            11 * 3_600_000_000_000
        );
    }

    #[test]
    fn utc_nanos_negative() {
        assert_eq!(
            time_to_utc_nanos(&jiff::civil::Time::new(12, 0, 0, 0).unwrap(), -5 * 3600),
            17 * 3_600_000_000_000
        );
    }
}
