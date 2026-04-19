//! localtime() constructor: time-of-day without timezone.

use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

use super::super::super::super::builtins::type_error;
use super::super::super::super::get_query_now;
use super::super::super::{map_get_int, temporal_parse_error};
use super::super::extract_base_time;

pub(crate) fn builtin_temporal_realtime_localtime(vals: Vec<Value>) -> Result<Value, GqlError> {
    if vals.first().map(|v| v.is_null()).unwrap_or(false) {
        return Ok(Value::Null);
    }
    if !vals.is_empty() {
        return eval_localtime_constructor(&vals);
    }
    let now = get_query_now();
    Ok(Value::LocalTime(jiff::civil::Time::from(now)))
}

pub(crate) fn eval_localtime_constructor(vals: &[Value]) -> Result<Value, GqlError> {
    if vals.is_empty() {
        let now = get_query_now();
        return Ok(Value::LocalTime(jiff::civil::Time::from(now)));
    }
    if vals.len() != 1 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "localtime() expects 0 or 1 argument".to_string(),
            actual: format!("{} arguments", vals.len()),
        }));
    }
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Str(s) => {
            let t: jiff::civil::Time = s
                .parse()
                .map_err(|e| temporal_parse_error("localtime", e))?;
            Ok(Value::LocalTime(t))
        }
        Value::LocalTime(t) => Ok(Value::LocalTime(*t)),
        Value::LocalDateTime(dt) => Ok(Value::LocalTime(
            jiff::civil::Time::new(dt.hour(), dt.minute(), dt.second(), dt.subsec_nanosecond())
                .map_err(|e| temporal_parse_error("localtime", e))?,
        )),
        Value::Time(t, _offset) => Ok(Value::LocalTime(*t)),
        Value::DateTime(zdt) => {
            let dt = jiff::civil::DateTime::from(zdt.as_ref());
            Ok(Value::LocalTime(
                jiff::civil::Time::new(dt.hour(), dt.minute(), dt.second(), dt.subsec_nanosecond())
                    .map_err(|e| temporal_parse_error("localtime", e))?,
            ))
        }
        Value::Map(m) => localtime_from_map(m),
        other => type_error("localtime", other),
    }
}

fn localtime_from_map(m: &indexmap::IndexMap<Box<str>, Value>) -> Result<Value, GqlError> {
    let base_time = extract_base_time(m)?;
    let hour = if m.contains_key("hour") {
        map_get_int(m, "hour", 0)? as i8
    } else if let Some(bt) = &base_time {
        bt.hour()
    } else {
        0
    };
    let minute = if m.contains_key("minute") {
        map_get_int(m, "minute", 0)? as i8
    } else if let Some(bt) = &base_time {
        bt.minute()
    } else {
        0
    };
    let second = if m.contains_key("second") {
        map_get_int(m, "second", 0)? as i8
    } else if let Some(bt) = &base_time {
        bt.second()
    } else {
        0
    };
    let nanosecond = if m.contains_key("nanosecond")
        || m.contains_key("microsecond")
        || m.contains_key("millisecond")
    {
        let millis = map_get_int(m, "millisecond", 0)? * 1_000_000;
        let micros = map_get_int(m, "microsecond", 0)? * 1_000;
        let nanos = map_get_int(m, "nanosecond", 0)?;
        (millis + micros + nanos) as i32
    } else if let Some(bt) = &base_time {
        bt.subsec_nanosecond()
    } else {
        0
    };
    let t = jiff::civil::Time::new(hour, minute, second, nanosecond)
        .map_err(|e| temporal_parse_error("localtime", e))?;
    Ok(Value::LocalTime(t))
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;

    #[test]
    fn localtime_from_string() {
        match eval_localtime_constructor(&[Value::Str("10:30:45".into())]).unwrap() {
            Value::LocalTime(t) => assert_eq!(t.hour(), 10),
            _ => panic!("LT"),
        }
    }
    #[test]
    fn localtime_from_null() {
        assert_eq!(
            eval_localtime_constructor(&[Value::Null]).unwrap(),
            Value::Null
        );
    }
    #[test]
    fn localtime_no_args() {
        assert!(matches!(
            eval_localtime_constructor(&[]).unwrap(),
            Value::LocalTime(_)
        ));
    }
    #[test]
    fn localtime_too_many_args() {
        assert!(eval_localtime_constructor(&[Value::Null, Value::Null]).is_err());
    }
    #[test]
    fn localtime_from_int_errs() {
        assert!(eval_localtime_constructor(&[Value::Int(0)]).is_err());
    }
    #[test]
    fn localtime_from_localtime() {
        let v = Value::LocalTime(jiff::civil::Time::new(7, 0, 0, 0).unwrap());
        if let Value::LocalTime(t) = eval_localtime_constructor(&[v]).unwrap() {
            assert_eq!(t.hour(), 7);
        } else {
            panic!()
        }
    }
    #[test]
    fn localtime_from_time() {
        let v = Value::Time(jiff::civil::Time::new(7, 0, 0, 0).unwrap(), 3600);
        if let Value::LocalTime(t) = eval_localtime_constructor(&[v]).unwrap() {
            assert_eq!(t.hour(), 7);
        } else {
            panic!()
        }
    }
    #[test]
    fn localtime_from_datetime() {
        let z: jiff::Zoned = "2024-01-01T05:00:00+01:00[Europe/Berlin]".parse().unwrap();
        if let Value::LocalTime(t) =
            eval_localtime_constructor(&[Value::DateTime(Box::new(z))]).unwrap()
        {
            assert_eq!(t.hour(), 5);
        } else {
            panic!()
        }
    }
    #[test]
    fn localtime_from_localdatetime() {
        let v = Value::LocalDateTime(jiff::civil::DateTime::new(2024, 1, 1, 7, 0, 0, 0).unwrap());
        if let Value::LocalTime(t) = eval_localtime_constructor(&[v]).unwrap() {
            assert_eq!(t.hour(), 7);
        } else {
            panic!()
        }
    }
    #[test]
    fn localtime_from_map() {
        let mut m = IndexMap::<Box<str>, Value>::new();
        m.insert("hour".into(), Value::Int(15));
        m.insert("minute".into(), Value::Int(30));
        if let Value::LocalTime(t) = eval_localtime_constructor(&[Value::Map(m)]).unwrap() {
            assert_eq!(t.hour(), 15);
        } else {
            panic!()
        }
    }
    #[test]
    fn realtime_localtime_works() {
        assert!(matches!(
            builtin_temporal_realtime_localtime(vec![]).unwrap(),
            Value::LocalTime(_)
        ));
        assert_eq!(
            builtin_temporal_realtime_localtime(vec![Value::Null]).unwrap(),
            Value::Null
        );
    }
}
