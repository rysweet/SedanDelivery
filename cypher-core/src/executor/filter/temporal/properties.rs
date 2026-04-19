//! Temporal property access (.year, .month, .day, etc.) and time-to-UTC-nanos conversion.

use crate::error::{ExecutionError, GqlError};
use crate::types::value::Crs;
use crate::types::Value;

use super::temporal_parse_error;

pub fn temporal_property_access(val: &Value, key: &str) -> Result<Value, GqlError> {
    match val {
        Value::Date(d) => match key {
            "year" => Ok(Value::Int(d.year() as i64)),
            "month" => Ok(Value::Int(d.month() as i64)),
            "day" => Ok(Value::Int(d.day() as i64)),
            "week" => {
                let iso = d.iso_week_date();
                Ok(Value::Int(iso.week() as i64))
            }
            "weekYear" => {
                let iso = d.iso_week_date();
                Ok(Value::Int(iso.year() as i64))
            }
            "dayOfWeek" | "weekDay" => Ok(Value::Int(d.weekday().to_monday_one_offset() as i64)),
            "dayOfYear" => Ok(Value::Int(d.day_of_year() as i64)),
            "quarter" => Ok(Value::Int(((d.month() as i64 - 1) / 3) + 1)),
            "ordinalDay" => Ok(Value::Int(d.day_of_year() as i64)),
            "dayOfQuarter" => {
                let q0 = ((d.month() as i64 - 1) / 3) as i32; // 0-indexed quarter
                let q_start = jiff::civil::Date::new(d.year(), (q0 * 3 + 1) as i8, 1)
                    .map_err(|e| temporal_parse_error("date.dayOfQuarter", e))?;
                let diff = q_start
                    .until(jiff::civil::DateDifference::new(*d).largest(jiff::Unit::Day))
                    .map_err(|e| temporal_parse_error("date.dayOfQuarter", e))?;
                Ok(Value::Int(diff.get_days() as i64 + 1))
            }
            _ => Ok(Value::Null),
        },
        Value::LocalTime(t) => match key {
            "hour" => Ok(Value::Int(t.hour() as i64)),
            "minute" => Ok(Value::Int(t.minute() as i64)),
            "second" => Ok(Value::Int(t.second() as i64)),
            "millisecond" => Ok(Value::Int((t.subsec_nanosecond() / 1_000_000) as i64)),
            "microsecond" => Ok(Value::Int((t.subsec_nanosecond() / 1_000) as i64)),
            "nanosecond" => Ok(Value::Int(t.subsec_nanosecond() as i64)),
            _ => Ok(Value::Null),
        },
        Value::Time(t, offset_secs) => match key {
            "hour" => Ok(Value::Int(t.hour() as i64)),
            "minute" => Ok(Value::Int(t.minute() as i64)),
            "second" => Ok(Value::Int(t.second() as i64)),
            "millisecond" => Ok(Value::Int((t.subsec_nanosecond() / 1_000_000) as i64)),
            "microsecond" => Ok(Value::Int((t.subsec_nanosecond() / 1_000) as i64)),
            "nanosecond" => Ok(Value::Int(t.subsec_nanosecond() as i64)),
            "offset" => Ok(Value::Str(
                crate::types::value::format_offset_str(*offset_secs).into_boxed_str(),
            )),
            "offsetMinutes" => Ok(Value::Int((*offset_secs / 60) as i64)),
            "offsetSeconds" => Ok(Value::Int(*offset_secs as i64)),
            "timezone" => Ok(Value::Str(
                crate::types::value::format_offset_str(*offset_secs).into_boxed_str(),
            )),
            _ => Ok(Value::Null),
        },
        Value::LocalDateTime(dt) => match key {
            "year" => Ok(Value::Int(dt.year() as i64)),
            "month" => Ok(Value::Int(dt.month() as i64)),
            "day" => Ok(Value::Int(dt.day() as i64)),
            "hour" => Ok(Value::Int(dt.hour() as i64)),
            "minute" => Ok(Value::Int(dt.minute() as i64)),
            "second" => Ok(Value::Int(dt.second() as i64)),
            "millisecond" => Ok(Value::Int((dt.subsec_nanosecond() / 1_000_000) as i64)),
            "microsecond" => Ok(Value::Int((dt.subsec_nanosecond() / 1_000) as i64)),
            "nanosecond" => Ok(Value::Int(dt.subsec_nanosecond() as i64)),
            "week" => {
                let d = dt.date();
                let iso = d.iso_week_date();
                Ok(Value::Int(iso.week() as i64))
            }
            "weekYear" => {
                let iso = dt.date().iso_week_date();
                Ok(Value::Int(iso.year() as i64))
            }
            "dayOfWeek" | "weekDay" => {
                Ok(Value::Int(dt.date().weekday().to_monday_one_offset() as i64))
            }
            "dayOfYear" | "ordinalDay" => Ok(Value::Int(dt.date().day_of_year() as i64)),
            "quarter" => Ok(Value::Int(((dt.month() as i64 - 1) / 3) + 1)),
            "dayOfQuarter" => {
                let d = dt.date();
                let q0 = ((d.month() as i64 - 1) / 3) as i32;
                let q_start = jiff::civil::Date::new(d.year(), (q0 * 3 + 1) as i8, 1)
                    .map_err(|e| temporal_parse_error("localdatetime.dayOfQuarter", e))?;
                let diff = q_start
                    .until(jiff::civil::DateDifference::new(d).largest(jiff::Unit::Day))
                    .map_err(|e| temporal_parse_error("localdatetime.dayOfQuarter", e))?;
                Ok(Value::Int(diff.get_days() as i64 + 1))
            }
            _ => Ok(Value::Null),
        },
        Value::DateTime(zdt) => match key {
            "year" => Ok(Value::Int(zdt.year() as i64)),
            "month" => Ok(Value::Int(zdt.month() as i64)),
            "day" => Ok(Value::Int(zdt.day() as i64)),
            "hour" => Ok(Value::Int(zdt.hour() as i64)),
            "minute" => Ok(Value::Int(zdt.minute() as i64)),
            "second" => Ok(Value::Int(zdt.second() as i64)),
            "millisecond" => Ok(Value::Int((zdt.subsec_nanosecond() / 1_000_000) as i64)),
            "microsecond" => Ok(Value::Int((zdt.subsec_nanosecond() / 1_000) as i64)),
            "nanosecond" => Ok(Value::Int(zdt.subsec_nanosecond() as i64)),
            "timezone" => {
                if let Some(iana) = zdt.time_zone().iana_name() {
                    // The IANA "Factory" timezone is valid in jiff but not in
                    // common timezone libraries (pytz). Treat it as a fixed
                    // offset to avoid interop issues.
                    if iana == "Factory" {
                        let off = zdt.offset();
                        let total_secs = off.seconds();
                        let h = total_secs / 3600;
                        let m = (total_secs.abs() % 3600) / 60;
                        Ok(Value::Str(format!("{:+03}:{:02}", h, m).into_boxed_str()))
                    } else {
                        Ok(Value::Str(iana.to_string().into_boxed_str()))
                    }
                } else {
                    // Fixed offset: format as "+HH:MM"
                    let off = zdt.offset();
                    let total_secs = off.seconds();
                    let h = total_secs / 3600;
                    let m = (total_secs.abs() % 3600) / 60;
                    Ok(Value::Str(format!("{:+03}:{:02}", h, m).into_boxed_str()))
                }
            }
            "offset" => {
                let off = zdt.offset();
                let total_secs = off.seconds();
                let h = total_secs / 3600;
                let m = (total_secs.abs() % 3600) / 60;
                Ok(Value::Str(format!("{:+03}:{:02}", h, m).into_boxed_str()))
            }
            "offsetMinutes" => {
                let off = zdt.offset();
                Ok(Value::Int((off.seconds() / 60) as i64))
            }
            "offsetSeconds" => {
                let off = zdt.offset();
                Ok(Value::Int(off.seconds() as i64))
            }
            "week" => {
                let d = jiff::civil::Date::from(&**zdt);
                let iso = d.iso_week_date();
                Ok(Value::Int(iso.week() as i64))
            }
            "weekYear" => {
                let d = jiff::civil::Date::from(&**zdt);
                let iso = d.iso_week_date();
                Ok(Value::Int(iso.year() as i64))
            }
            "dayOfWeek" | "weekDay" => {
                let d = jiff::civil::Date::from(&**zdt);
                Ok(Value::Int(d.weekday().to_monday_one_offset() as i64))
            }
            "dayOfYear" | "ordinalDay" => {
                let d = jiff::civil::Date::from(&**zdt);
                Ok(Value::Int(d.day_of_year() as i64))
            }
            "quarter" => Ok(Value::Int(((zdt.month() as i64 - 1) / 3) + 1)),
            "dayOfQuarter" => {
                let d = jiff::civil::Date::from(&**zdt);
                let q0 = ((d.month() as i64 - 1) / 3) as i32;
                let q_start = jiff::civil::Date::new(d.year(), (q0 * 3 + 1) as i8, 1)
                    .map_err(|e| temporal_parse_error("datetime.dayOfQuarter", e))?;
                let diff = q_start
                    .until(jiff::civil::DateDifference::new(d).largest(jiff::Unit::Day))
                    .map_err(|e| temporal_parse_error("datetime.dayOfQuarter", e))?;
                Ok(Value::Int(diff.get_days() as i64 + 1))
            }
            "epochMillis" => {
                let ts = zdt.timestamp();
                Ok(Value::Int(ts.as_millisecond()))
            }
            "epochSeconds" => {
                let ts = zdt.timestamp();
                Ok(Value::Int(ts.as_second()))
            }
            _ => Ok(Value::Null),
        },
        Value::Duration(d) => match key {
            "years" => Ok(Value::Int(d.months.div_euclid(12))),
            "months" => Ok(Value::Int(d.months)),
            "quarters" => Ok(Value::Int(d.months.div_euclid(3))),
            "quartersOfYear" => Ok(Value::Int(d.months.div_euclid(3).rem_euclid(4))),
            "monthsOfYear" => Ok(Value::Int(d.months.rem_euclid(12))),
            "monthsOfQuarter" => Ok(Value::Int(d.months.rem_euclid(3))),
            "weeks" => Ok(Value::Int(d.days.div_euclid(7))),
            "days" => Ok(Value::Int(d.days)),
            "daysOfWeek" => Ok(Value::Int(d.days.rem_euclid(7))),
            "hours" => Ok(Value::Int(d.nanos.div_euclid(3_600_000_000_000) as i64)),
            "minutes" => Ok(Value::Int(d.nanos.div_euclid(60_000_000_000) as i64)),
            "minutesOfHour" => Ok(Value::Int(
                (d.nanos.div_euclid(60_000_000_000).rem_euclid(60)) as i64,
            )),
            "seconds" => Ok(Value::Int(d.nanos.div_euclid(1_000_000_000) as i64)),
            "secondsOfMinute" => Ok(Value::Int(
                (d.nanos.div_euclid(1_000_000_000).rem_euclid(60)) as i64,
            )),
            "milliseconds" => Ok(Value::Int(d.nanos.div_euclid(1_000_000) as i64)),
            "millisecondsOfSecond" => Ok(Value::Int(
                (d.nanos.rem_euclid(1_000_000_000) / 1_000_000) as i64,
            )),
            "microseconds" => Ok(Value::Int(d.nanos.div_euclid(1_000) as i64)),
            "microsecondsOfSecond" => Ok(Value::Int(
                (d.nanos.rem_euclid(1_000_000_000) / 1_000) as i64,
            )),
            "nanoseconds" => {
                let nanos_i64 = i64::try_from(d.nanos).map_err(|_| {
                    GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: "duration nanoseconds within i64 range".to_string(),
                        actual: format!("nanoseconds {} overflow i64 range", d.nanos),
                    })
                })?;
                Ok(Value::Int(nanos_i64))
            }
            "nanosecondsOfSecond" => Ok(Value::Int(d.nanos.rem_euclid(1_000_000_000) as i64)),
            _ => Ok(Value::Null),
        },
        Value::Point(p) => match key {
            "x" => Ok(Value::Float(p.x)),
            "y" => Ok(Value::Float(p.y)),
            "z" => match p.z {
                Some(z) => Ok(Value::Float(z)),
                None => Ok(Value::Null),
            },
            "srid" => {
                let srid = match p.crs {
                    Crs::Wgs84 => 4326,
                    Crs::Wgs843d => 4979,
                    Crs::Cartesian => 7203,
                    Crs::Cartesian3d => 9157,
                };
                Ok(Value::Int(srid))
            }
            "crs" => {
                let name = match p.crs {
                    Crs::Wgs84 => "wgs-84",
                    Crs::Wgs843d => "wgs-84-3d",
                    Crs::Cartesian => "cartesian",
                    Crs::Cartesian3d => "cartesian-3d",
                };
                Ok(Value::Str(name.into()))
            }
            // For WGS84 points, also expose longitude/latitude/height aliases
            "longitude" if matches!(p.crs, Crs::Wgs84 | Crs::Wgs843d) => Ok(Value::Float(p.x)),
            "latitude" if matches!(p.crs, Crs::Wgs84 | Crs::Wgs843d) => Ok(Value::Float(p.y)),
            "height" if matches!(p.crs, Crs::Wgs843d) => match p.z {
                Some(z) => Ok(Value::Float(z)),
                None => Ok(Value::Null),
            },
            _ => Ok(Value::Null),
        },
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
    use super::*;
    use crate::types::value::GeoPoint;
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
