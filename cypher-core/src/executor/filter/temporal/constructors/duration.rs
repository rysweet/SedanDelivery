//! duration() constructor.

use crate::error::{ExecutionError, GqlError};
use crate::types::{CypherDuration, Value};

use super::super::super::builtins::type_error;
use super::super::parsing::parse_duration_str;

pub(crate) fn eval_duration_constructor(vals: &[Value]) -> Result<Value, GqlError> {
    if vals.is_empty() {
        return Ok(Value::Duration(CypherDuration {
            months: 0,
            days: 0,
            nanos: 0,
        }));
    }
    if vals.len() != 1 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "duration() expects 0 or 1 argument".to_string(),
            actual: format!("{} arguments", vals.len()),
        }));
    }
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Str(s) => {
            let d = parse_duration_str(s)?;
            Ok(Value::Duration(d))
        }
        Value::Map(m) => {
            fn dur_get(m: &indexmap::IndexMap<Box<str>, Value>, key: &str) -> f64 {
                match m.get(key) {
                    Some(Value::Int(v)) => *v as f64,
                    Some(Value::Float(v)) => *v,
                    _ => 0.0,
                }
            }
            let raw_years = dur_get(m, "years") + dur_get(m, "year");
            let raw_months = dur_get(m, "months") + dur_get(m, "month");
            let raw_weeks = dur_get(m, "weeks") + dur_get(m, "week");
            let raw_days = dur_get(m, "days") + dur_get(m, "day");
            let raw_hours = dur_get(m, "hours") + dur_get(m, "hour");
            let raw_minutes = dur_get(m, "minutes") + dur_get(m, "minute");
            let raw_seconds = dur_get(m, "seconds") + dur_get(m, "second");
            let raw_millis = dur_get(m, "milliseconds") + dur_get(m, "millisecond");
            let raw_micros = dur_get(m, "microseconds") + dur_get(m, "microsecond");
            let raw_nanos_sub = dur_get(m, "nanoseconds") + dur_get(m, "nanosecond");
            let sub_sec_nanos = raw_millis * 1_000_000.0 + raw_micros * 1_000.0 + raw_nanos_sub;

            let int_years = raw_years as i64;
            let frac_years = raw_years - int_years as f64;
            let total_month_float = frac_years * 12.0 + raw_months;
            let int_months_raw = total_month_float as i64;
            let frac_months = total_month_float - int_months_raw as f64;
            let total_months = int_years * 12 + int_months_raw;

            let total_days_float =
                frac_months * (2_629_746.0 / 86_400.0) + raw_weeks * 7.0 + raw_days;
            let int_days = total_days_float.trunc() as i64;
            let frac_days = total_days_float - int_days as f64;

            let total_time_secs = frac_days * 86_400.0
                + raw_hours * 3_600.0
                + raw_minutes * 60.0
                + raw_seconds
                + sub_sec_nanos / 1_000_000_000.0;
            let total_nanos = (total_time_secs * 1_000_000_000.0).round() as i128;

            Ok(Value::Duration(CypherDuration {
                months: total_months,
                days: int_days,
                nanos: total_nanos,
            }))
        }
        other => type_error("duration", other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dur_empty() {
        match eval_duration_constructor(&[]).unwrap() {
            Value::Duration(d) => {
                assert_eq!(d.months, 0);
                assert_eq!(d.days, 0);
            }
            _ => panic!("Dur"),
        }
    }
    #[test]
    fn dur_from_string() {
        match eval_duration_constructor(&[Value::Str("P1Y2M3D".into())]).unwrap() {
            Value::Duration(d) => {
                assert_eq!(d.months, 14);
                assert_eq!(d.days, 3);
            }
            _ => panic!("Dur"),
        }
    }
    #[test]
    fn dur_from_null() {
        assert_eq!(
            eval_duration_constructor(&[Value::Null]).unwrap(),
            Value::Null
        );
    }

    #[test]
    fn dur_too_many_args_errs() {
        assert!(eval_duration_constructor(&[Value::Null, Value::Null]).is_err());
    }
    #[test]
    fn dur_from_int_errs() {
        assert!(eval_duration_constructor(&[Value::Int(0)]).is_err());
    }
    #[test]
    fn dur_from_map_full() {
        let mut m = indexmap::IndexMap::<Box<str>, Value>::new();
        m.insert("years".into(), Value::Int(1));
        m.insert("months".into(), Value::Int(2));
        m.insert("weeks".into(), Value::Int(1));
        m.insert("days".into(), Value::Int(3));
        m.insert("hours".into(), Value::Int(4));
        m.insert("minutes".into(), Value::Int(5));
        m.insert("seconds".into(), Value::Int(6));
        m.insert("milliseconds".into(), Value::Int(7));
        m.insert("microseconds".into(), Value::Int(8));
        m.insert("nanoseconds".into(), Value::Int(9));
        if let Value::Duration(d) = eval_duration_constructor(&[Value::Map(m)]).unwrap() {
            assert_eq!(d.months, 14);
            assert_eq!(d.days, 10);
            assert!(d.nanos > 0);
        } else {
            panic!()
        }
    }
    #[test]
    fn dur_from_map_singular_keys() {
        let mut m = indexmap::IndexMap::<Box<str>, Value>::new();
        m.insert("year".into(), Value::Int(1));
        m.insert("month".into(), Value::Int(2));
        m.insert("week".into(), Value::Int(1));
        m.insert("day".into(), Value::Int(3));
        m.insert("hour".into(), Value::Int(4));
        m.insert("minute".into(), Value::Int(5));
        m.insert("second".into(), Value::Int(6));
        m.insert("millisecond".into(), Value::Int(7));
        m.insert("microsecond".into(), Value::Int(8));
        m.insert("nanosecond".into(), Value::Int(9));
        if let Value::Duration(d) = eval_duration_constructor(&[Value::Map(m)]).unwrap() {
            assert_eq!(d.months, 14);
            assert_eq!(d.days, 10);
        } else {
            panic!()
        }
    }
    #[test]
    fn dur_from_map_floats_fractional() {
        let mut m = indexmap::IndexMap::<Box<str>, Value>::new();
        m.insert("years".into(), Value::Float(1.5));
        m.insert("days".into(), Value::Float(0.5));
        if let Value::Duration(d) = eval_duration_constructor(&[Value::Map(m)]).unwrap() {
            assert_eq!(d.months, 18);
            assert!(d.nanos > 0);
        } else {
            panic!()
        }
    }
    #[test]
    fn dur_from_map_empty() {
        let m = indexmap::IndexMap::<Box<str>, Value>::new();
        if let Value::Duration(d) = eval_duration_constructor(&[Value::Map(m)]).unwrap() {
            assert_eq!(d.months, 0);
            assert_eq!(d.days, 0);
            assert_eq!(d.nanos, 0);
        } else {
            panic!()
        }
    }
    #[test]
    fn dur_invalid_string_errs() {
        assert!(eval_duration_constructor(&[Value::Str("not-a-duration".into())]).is_err());
    }
}
