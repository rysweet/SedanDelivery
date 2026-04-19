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
    use crate::types::value::{Crs, GeoPoint};
    use crate::types::CypherDuration;
    #[test]
    fn date_props() {
        let d = Value::Date(jiff::civil::Date::new(2024, 3, 15).unwrap());
        assert_eq!(
            temporal_property_access(&d, "year").unwrap(),
            Value::Int(2024)
        );
        assert_eq!(temporal_property_access(&d, "day").unwrap(), Value::Int(15));
    }
    #[test]
    fn date_quarter() {
        assert_eq!(
            temporal_property_access(
                &Value::Date(jiff::civil::Date::new(2024, 4, 15).unwrap()),
                "quarter"
            )
            .unwrap(),
            Value::Int(2)
        );
    }
    #[test]
    fn date_unknown() {
        assert_eq!(
            temporal_property_access(
                &Value::Date(jiff::civil::Date::new(2024, 1, 1).unwrap()),
                "nope"
            )
            .unwrap(),
            Value::Null
        );
    }
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
    fn dur_props() {
        let d = Value::Duration(CypherDuration {
            months: 27,
            days: 17,
            nanos: 3_723_456_789_012,
        });
        assert_eq!(
            temporal_property_access(&d, "years").unwrap(),
            Value::Int(2)
        );
        assert_eq!(
            temporal_property_access(&d, "weeks").unwrap(),
            Value::Int(2)
        );
    }
    #[test]
    fn point_props() {
        let p = Value::Point(GeoPoint {
            x: 1.0,
            y: 2.0,
            z: None,
            crs: Crs::Cartesian,
        });
        assert_eq!(
            temporal_property_access(&p, "x").unwrap(),
            Value::Float(1.0)
        );
        assert_eq!(
            temporal_property_access(&p, "srid").unwrap(),
            Value::Int(7203)
        );
    }
    #[test]
    fn non_temporal() {
        assert_eq!(
            temporal_property_access(&Value::Int(42), "year").unwrap(),
            Value::Null
        );
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

    fn date(y: i16, m: i8, d: i8) -> Value {
        Value::Date(jiff::civil::Date::new(y, m, d).unwrap())
    }
    fn ldt(y: i16, mo: i8, d: i8, h: i8, mi: i8, s: i8, ns: i32) -> Value {
        Value::LocalDateTime(jiff::civil::DateTime::new(y, mo, d, h, mi, s, ns).unwrap())
    }
    fn dt_utc(y: i16, mo: i8, d: i8, h: i8, mi: i8, s: i8) -> Value {
        let z = jiff::civil::DateTime::new(y, mo, d, h, mi, s, 0)
            .unwrap()
            .to_zoned(jiff::tz::TimeZone::UTC)
            .unwrap();
        Value::DateTime(Box::new(z))
    }

    #[test]
    fn date_all_keys() {
        let d = date(2024, 4, 15);
        for k in [
            "year",
            "month",
            "day",
            "week",
            "weekYear",
            "dayOfWeek",
            "weekDay",
            "dayOfYear",
            "ordinalDay",
            "quarter",
            "dayOfQuarter",
        ] {
            assert!(matches!(
                temporal_property_access(&d, k).unwrap(),
                Value::Int(_)
            ));
        }
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
    fn localdatetime_all_keys() {
        let l = ldt(2024, 4, 15, 10, 30, 45, 123_456_789);
        for k in [
            "year",
            "month",
            "day",
            "hour",
            "minute",
            "second",
            "millisecond",
            "microsecond",
            "nanosecond",
            "week",
            "weekYear",
            "dayOfWeek",
            "weekDay",
            "dayOfYear",
            "ordinalDay",
            "quarter",
            "dayOfQuarter",
        ] {
            assert!(matches!(
                temporal_property_access(&l, k).unwrap(),
                Value::Int(_)
            ));
        }
        assert_eq!(temporal_property_access(&l, "nope").unwrap(), Value::Null);
    }

    #[test]
    fn datetime_all_keys() {
        let z = dt_utc(2024, 4, 15, 10, 30, 45);
        for k in [
            "year",
            "month",
            "day",
            "hour",
            "minute",
            "second",
            "millisecond",
            "microsecond",
            "nanosecond",
            "week",
            "weekYear",
            "dayOfWeek",
            "weekDay",
            "dayOfYear",
            "ordinalDay",
            "quarter",
            "dayOfQuarter",
            "offsetMinutes",
            "offsetSeconds",
            "epochMillis",
            "epochSeconds",
        ] {
            assert!(matches!(
                temporal_property_access(&z, k).unwrap(),
                Value::Int(_)
            ));
        }
        for k in ["timezone", "offset"] {
            assert!(matches!(
                temporal_property_access(&z, k).unwrap(),
                Value::Str(_)
            ));
        }
        assert_eq!(temporal_property_access(&z, "nope").unwrap(), Value::Null);
    }

    #[test]
    fn datetime_with_named_iana_timezone() {
        let z: jiff::Zoned = "2024-01-01T00:00:00+00:00[Europe/London]".parse().unwrap();
        let v = Value::DateTime(Box::new(z));
        if let Value::Str(s) = temporal_property_access(&v, "timezone").unwrap() {
            assert_eq!(&*s, "Europe/London");
        } else {
            panic!()
        }
    }

    #[test]
    fn datetime_with_factory_timezone_returns_offset() {
        let z: jiff::Zoned = "1970-01-01T00:00:00-00:00[Factory]".parse().unwrap();
        let v = Value::DateTime(Box::new(z));
        if let Value::Str(s) = temporal_property_access(&v, "timezone").unwrap() {
            assert!(s.starts_with('+') || s.starts_with('-'));
        } else {
            panic!()
        }
    }

    #[test]
    fn duration_all_keys() {
        let d = Value::Duration(CypherDuration {
            months: 27,
            days: 17,
            nanos: 3_723_456_789_012,
        });
        for k in [
            "years",
            "months",
            "quarters",
            "quartersOfYear",
            "monthsOfYear",
            "monthsOfQuarter",
            "weeks",
            "days",
            "daysOfWeek",
            "hours",
            "minutes",
            "minutesOfHour",
            "seconds",
            "secondsOfMinute",
            "milliseconds",
            "millisecondsOfSecond",
            "microseconds",
            "microsecondsOfSecond",
            "nanoseconds",
            "nanosecondsOfSecond",
        ] {
            assert!(matches!(
                temporal_property_access(&d, k).unwrap(),
                Value::Int(_)
            ));
        }
        assert_eq!(temporal_property_access(&d, "nope").unwrap(), Value::Null);
    }

    #[test]
    fn duration_nanos_overflow_errs() {
        let d = Value::Duration(CypherDuration {
            months: 0,
            days: 0,
            nanos: i128::MAX,
        });
        assert!(temporal_property_access(&d, "nanoseconds").is_err());
    }

    #[test]
    fn point_all_keys_wgs84_3d() {
        let p = Value::Point(GeoPoint {
            x: 10.0,
            y: 20.0,
            z: Some(100.0),
            crs: Crs::Wgs843d,
        });
        for k in ["x", "y", "z", "longitude", "latitude", "height"] {
            let _ = temporal_property_access(&p, k).unwrap();
        }
        assert!(matches!(
            temporal_property_access(&p, "srid").unwrap(),
            Value::Int(_)
        ));
        if let Value::Str(s) = temporal_property_access(&p, "crs").unwrap() {
            assert_eq!(&*s, "wgs-84-3d");
        } else {
            panic!()
        }
        assert_eq!(temporal_property_access(&p, "nope").unwrap(), Value::Null);
    }

    #[test]
    fn point_all_keys_cartesian2d_no_z() {
        let p = Value::Point(GeoPoint {
            x: 1.0,
            y: 2.0,
            z: None,
            crs: Crs::Cartesian,
        });
        assert_eq!(temporal_property_access(&p, "z").unwrap(), Value::Null);
        if let Value::Str(s) = temporal_property_access(&p, "crs").unwrap() {
            assert_eq!(&*s, "cartesian");
        } else {
            panic!()
        }
        // longitude/latitude don't apply to cartesian
        assert_eq!(
            temporal_property_access(&p, "longitude").unwrap(),
            Value::Null
        );
    }

    #[test]
    fn point_crs_variants() {
        for (crs, want_srid, want_name) in [
            (Crs::Wgs84, 4326, "wgs-84"),
            (Crs::Wgs843d, 4979, "wgs-84-3d"),
            (Crs::Cartesian, 7203, "cartesian"),
            (Crs::Cartesian3d, 9157, "cartesian-3d"),
        ] {
            let p = Value::Point(GeoPoint {
                x: 0.0,
                y: 0.0,
                z: None,
                crs,
            });
            assert_eq!(
                temporal_property_access(&p, "srid").unwrap(),
                Value::Int(want_srid)
            );
            if let Value::Str(s) = temporal_property_access(&p, "crs").unwrap() {
                assert_eq!(&*s, want_name);
            } else {
                panic!()
            }
        }
    }
}
