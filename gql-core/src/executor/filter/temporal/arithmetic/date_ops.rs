//! Date/time add/sub operations with durations.

use crate::error::{ExecutionError, GqlError};
use crate::types::{CypherDuration, Value};

use super::super::temporal_parse_error;

pub fn temporal_add_duration(
    temporal: &Value,
    dur: &CypherDuration,
    subtract: bool,
) -> Result<Value, GqlError> {
    let (months, days, nanos) = if subtract {
        (-dur.months, -dur.days, -dur.nanos)
    } else {
        (dur.months, dur.days, dur.nanos)
    };
    match temporal {
        Value::Date(d) => {
            let d1 = if months != 0 {
                d.checked_add(jiff::Span::new().months(months))
                    .map_err(|e| temporal_parse_error("temporal arithmetic", e))?
            } else {
                *d
            };
            let d2 = if days != 0 {
                d1.checked_add(jiff::Span::new().days(days))
                    .map_err(|e| temporal_parse_error("temporal arithmetic", e))?
            } else {
                d1
            };
            let extra_days = nanos / 86_400_000_000_000_i128;
            let d3 = if extra_days != 0 {
                d2.checked_add(jiff::Span::new().days(extra_days as i64))
                    .map_err(|e| temporal_parse_error("temporal arithmetic", e))?
            } else {
                d2
            };
            Ok(Value::Date(d3))
        }
        Value::LocalTime(t) => {
            let cur_nanos = t.hour() as i128 * 3_600_000_000_000
                + t.minute() as i128 * 60_000_000_000
                + t.second() as i128 * 1_000_000_000
                + t.subsec_nanosecond() as i128;
            let total = cur_nanos + nanos;
            let day_nanos: i128 = 24 * 3_600_000_000_000;
            let wrapped = ((total % day_nanos) + day_nanos) % day_nanos;
            let h = (wrapped / 3_600_000_000_000) as i8;
            let rem = wrapped % 3_600_000_000_000;
            let m = (rem / 60_000_000_000) as i8;
            let rem = rem % 60_000_000_000;
            let s = (rem / 1_000_000_000) as i8;
            let ns = (rem % 1_000_000_000) as i32;
            let result = jiff::civil::Time::new(h, m, s, ns)
                .map_err(|e| temporal_parse_error("temporal arithmetic", e))?;
            Ok(Value::LocalTime(result))
        }
        Value::Time(t, offset) => {
            let cur_nanos = t.hour() as i128 * 3_600_000_000_000
                + t.minute() as i128 * 60_000_000_000
                + t.second() as i128 * 1_000_000_000
                + t.subsec_nanosecond() as i128;
            let total = cur_nanos + nanos;
            let day_nanos: i128 = 24 * 3_600_000_000_000;
            let wrapped = ((total % day_nanos) + day_nanos) % day_nanos;
            let h = (wrapped / 3_600_000_000_000) as i8;
            let rem = wrapped % 3_600_000_000_000;
            let m = (rem / 60_000_000_000) as i8;
            let rem = rem % 60_000_000_000;
            let s = (rem / 1_000_000_000) as i8;
            let ns = (rem % 1_000_000_000) as i32;
            let result = jiff::civil::Time::new(h, m, s, ns)
                .map_err(|e| temporal_parse_error("temporal arithmetic", e))?;
            Ok(Value::Time(result, *offset))
        }
        Value::LocalDateTime(dt) => {
            let dt1 = if months != 0 {
                dt.checked_add(jiff::Span::new().months(months))
                    .map_err(|e| temporal_parse_error("temporal arithmetic", e))?
            } else {
                *dt
            };
            let dt2 = if days != 0 {
                dt1.checked_add(jiff::Span::new().days(days))
                    .map_err(|e| temporal_parse_error("temporal arithmetic", e))?
            } else {
                dt1
            };
            let dt3 = if nanos != 0 {
                let nanos_i64 = i64::try_from(nanos).map_err(|_| {
                    GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: "duration nanoseconds within i64 range".to_string(),
                        actual: format!("nanoseconds {nanos} overflow i64 range"),
                    })
                })?;
                dt2.checked_add(jiff::SignedDuration::from_nanos(nanos_i64))
                    .map_err(|e| temporal_parse_error("temporal arithmetic", e))?
            } else {
                dt2
            };
            Ok(Value::LocalDateTime(dt3))
        }
        Value::DateTime(zdt) => {
            let zdt1 = if months != 0 {
                zdt.as_ref()
                    .clone()
                    .checked_add(jiff::Span::new().months(months))
                    .map_err(|e| temporal_parse_error("temporal arithmetic", e))?
            } else {
                zdt.as_ref().clone()
            };
            let zdt2 = if days != 0 {
                zdt1.checked_add(jiff::Span::new().days(days))
                    .map_err(|e| temporal_parse_error("temporal arithmetic", e))?
            } else {
                zdt1
            };
            let zdt3 = if nanos != 0 {
                let nanos_i64 = i64::try_from(nanos).map_err(|_| {
                    GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: "duration nanoseconds within i64 range".to_string(),
                        actual: format!("nanoseconds {nanos} overflow i64 range"),
                    })
                })?;
                zdt2.checked_add(jiff::SignedDuration::from_nanos(nanos_i64))
                    .map_err(|e| temporal_parse_error("temporal arithmetic", e))?
            } else {
                zdt2
            };
            Ok(Value::DateTime(Box::new(zdt3)))
        }
        _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "temporal value for arithmetic".to_string(),
            actual: temporal.type_name().to_string(),
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::CypherDuration;

    fn dur(months: i64, days: i64, nanos: i128) -> CypherDuration {
        CypherDuration {
            months,
            days,
            nanos,
        }
    }

    #[test]
    fn add_to_date() {
        let d = Value::Date(jiff::civil::Date::new(2024, 1, 1).unwrap());
        match temporal_add_duration(&d, &dur(1, 5, 0), false).unwrap() {
            Value::Date(d) => {
                assert_eq!(d.month(), 2);
                assert_eq!(d.day(), 6);
            }
            _ => panic!("Date"),
        }
    }
    #[test]
    fn sub_from_date() {
        let d = Value::Date(jiff::civil::Date::new(2024, 3, 15).unwrap());
        match temporal_add_duration(&d, &dur(1, 0, 0), true).unwrap() {
            Value::Date(d) => assert_eq!(d.month(), 2),
            _ => panic!("Date"),
        }
    }
    #[test]
    fn add_to_lt() {
        let t = Value::LocalTime(jiff::civil::Time::new(10, 30, 0, 0).unwrap());
        match temporal_add_duration(&t, &dur(0, 0, 3_600_000_000_000), false).unwrap() {
            Value::LocalTime(t) => assert_eq!(t.hour(), 11),
            _ => panic!("LT"),
        }
    }
    #[test]
    fn add_to_lt_wrap() {
        let t = Value::LocalTime(jiff::civil::Time::new(23, 0, 0, 0).unwrap());
        match temporal_add_duration(&t, &dur(0, 0, 2 * 3_600_000_000_000), false).unwrap() {
            Value::LocalTime(t) => assert_eq!(t.hour(), 1),
            _ => panic!("LT"),
        }
    }
    #[test]
    fn add_to_time() {
        let t = Value::Time(jiff::civil::Time::new(10, 0, 0, 0).unwrap(), 3600);
        match temporal_add_duration(&t, &dur(0, 0, 3_600_000_000_000), false).unwrap() {
            Value::Time(t, off) => {
                assert_eq!(t.hour(), 11);
                assert_eq!(off, 3600);
            }
            _ => panic!("T"),
        }
    }
    #[test]
    fn add_to_ldt() {
        let dt = Value::LocalDateTime(jiff::civil::DateTime::new(2024, 1, 1, 10, 0, 0, 0).unwrap());
        match temporal_add_duration(&dt, &dur(0, 1, 3_600_000_000_000), false).unwrap() {
            Value::LocalDateTime(dt) => {
                assert_eq!(dt.day(), 2);
                assert_eq!(dt.hour(), 11);
            }
            _ => panic!("LDT"),
        }
    }
    #[test]
    fn add_to_zdt() {
        let z = Value::DateTime(Box::new(
            jiff::civil::DateTime::new(2024, 1, 1, 10, 0, 0, 0)
                .unwrap()
                .to_zoned(jiff::tz::TimeZone::UTC)
                .unwrap(),
        ));
        match temporal_add_duration(&z, &dur(1, 0, 0), false).unwrap() {
            Value::DateTime(z) => assert_eq!(z.month(), 2),
            _ => panic!("DT"),
        }
    }
    #[test]
    fn add_non_temporal() {
        assert!(temporal_add_duration(&Value::Int(42), &dur(0, 0, 0), false).is_err());
    }
}
