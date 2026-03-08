//! time() and localtime() constructors.

use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

use super::super::super::builtins::type_error;
use super::super::super::get_query_now;
use super::super::parsing::{expand_compact_time, extract_offset_from_string, strip_tz_offset_from_time};
use super::super::{map_get_int, map_get_str, parse_timezone, temporal_parse_error};
use super::{extract_base_time, zoned_to_time_value};

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

pub(crate) fn builtin_temporal_realtime_time(vals: Vec<Value>) -> Result<Value, GqlError> {
    if vals.first().map(|v| v.is_null()).unwrap_or(false) {
        return Ok(Value::Null);
    }
    if !vals.is_empty() {
        return eval_time_constructor(&vals);
    }
    Ok(Value::DateTime(Box::new(get_query_now())))
}

pub(crate) fn eval_time_constructor(vals: &[Value]) -> Result<Value, GqlError> {
    if vals.is_empty() {
        let now = get_query_now();
        let t = jiff::civil::Time::from(&now);
        let offset_secs = now.offset().seconds();
        return Ok(Value::Time(t, offset_secs));
    }
    if vals.len() != 1 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "time() expects 0 or 1 argument".to_string(),
            actual: format!("{} arguments", vals.len()),
        }));
    }
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Str(s) => {
            let time_part = strip_tz_offset_from_time(s);
            let t: jiff::civil::Time = if let Ok(t) = s.parse::<jiff::civil::Time>() {
                t
            } else {
                let expanded = expand_compact_time(time_part);
                expanded
                    .parse::<jiff::civil::Time>()
                    .map_err(|e| temporal_parse_error("time", e))?
            };
            let tz = extract_offset_from_string(s).unwrap_or(jiff::tz::TimeZone::UTC);
            let epoch_dt = jiff::civil::Date::new(1970, 1, 1)
                .map_err(|e| temporal_parse_error("time", e))?
                .to_datetime(t);
            let zoned = epoch_dt
                .to_zoned(tz)
                .map_err(|e| temporal_parse_error("time", e))?;
            Ok(zoned_to_time_value(&zoned))
        }
        Value::LocalTime(t) => Ok(Value::Time(*t, 0)),
        Value::Time(t, offset) => Ok(Value::Time(*t, *offset)),
        Value::DateTime(zdt) => {
            let dt = jiff::civil::DateTime::from(zdt.as_ref());
            let t =
                jiff::civil::Time::new(dt.hour(), dt.minute(), dt.second(), dt.subsec_nanosecond())
                    .map_err(|e| temporal_parse_error("time", e))?;
            let offset_secs = zdt.offset().seconds();
            Ok(Value::Time(t, offset_secs))
        }
        Value::LocalDateTime(dt) => {
            let t =
                jiff::civil::Time::new(dt.hour(), dt.minute(), dt.second(), dt.subsec_nanosecond())
                    .map_err(|e| temporal_parse_error("time", e))?;
            Ok(Value::Time(t, 0))
        }
        Value::Map(m) => {
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
            let tz_name = map_get_str(m, "timezone");
            let t = jiff::civil::Time::new(hour, minute, second, nanosecond)
                .map_err(|e| temporal_parse_error("time", e))?;
            let epoch_dt = jiff::civil::Date::new(1970, 1, 1)
                .map_err(|e| temporal_parse_error("time", e))?
                .to_datetime(t);
            let base_tz = if let Some(Value::DateTime(base_zdt)) = m.get("time") {
                Some(base_zdt.time_zone().clone())
            } else if let Some(Value::Time(_, offset)) = m.get("time") {
                let off = jiff::tz::Offset::from_seconds(*offset).unwrap_or(jiff::tz::Offset::UTC);
                Some(jiff::tz::TimeZone::fixed(off))
            } else {
                None
            };

            let target_tz = if let Some(ref tz_str) = tz_name {
                parse_timezone(tz_str).map_err(|e| temporal_parse_error("time", e))?
            } else if let Some(ref tz) = base_tz {
                tz.clone()
            } else {
                jiff::tz::TimeZone::UTC
            };

            if let (Some(_), Some(ref btz)) = (&tz_name, &base_tz) {
                let base_zoned = epoch_dt
                    .to_zoned(btz.clone())
                    .map_err(|e| temporal_parse_error("time", e))?;
                let converted = base_zoned.with_time_zone(target_tz);
                return Ok(zoned_to_time_value(&converted));
            }

            let zoned = epoch_dt
                .to_zoned(target_tz)
                .map_err(|e| temporal_parse_error("time", e))?;
            Ok(zoned_to_time_value(&zoned))
        }
        other => type_error("time", other),
    }
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
        Value::Map(m) => {
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
        other => type_error("localtime", other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_from_string() {
        match eval_time_constructor(&[Value::Str("21:40:32+01:00".into())]).unwrap() {
            Value::Time(t, off) => {
                assert_eq!(t.hour(), 21);
                assert_eq!(off, 3600);
            }
            _ => panic!("Time"),
        }
    }
    #[test]
    fn time_from_null() {
        assert_eq!(eval_time_constructor(&[Value::Null]).unwrap(), Value::Null);
    }
    #[test]
    fn time_too_many() {
        assert!(eval_time_constructor(&[Value::Int(1), Value::Int(2)]).is_err());
    }
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
}
