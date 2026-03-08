//! datetime() and localdatetime() constructors.

use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

use super::super::super::builtins::type_error;
use super::super::super::get_query_now;
use super::super::parsing::{
    large_ldt_value, parse_datetime_with_offset, parse_large_date_str, parse_localdatetime_str,
};
use super::super::{map_get_int, map_get_str, parse_timezone, temporal_parse_error};
use super::{extract_base_date, extract_base_time, resolve_date_from_map};

pub(crate) fn builtin_temporal_realtime_datetime(vals: Vec<Value>) -> Result<Value, GqlError> {
    if vals.first().map(|v| v.is_null()).unwrap_or(false) {
        return Ok(Value::Null);
    }
    if !vals.is_empty() {
        return eval_datetime_constructor(&vals);
    }
    Ok(Value::DateTime(Box::new(get_query_now())))
}

pub(crate) fn builtin_temporal_realtime_localdatetime(
    vals: Vec<Value>,
) -> Result<Value, GqlError> {
    if vals.first().map(|v| v.is_null()).unwrap_or(false) {
        return Ok(Value::Null);
    }
    if !vals.is_empty() {
        return eval_localdatetime_constructor(&vals);
    }
    let now = get_query_now();
    Ok(Value::LocalDateTime(jiff::civil::DateTime::from(now)))
}

pub(crate) fn builtin_datetime_fromepoch(vals: Vec<Value>) -> Result<Value, GqlError> {
    if vals.is_empty() || vals.len() > 2 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "datetime.fromEpoch(seconds[, nanoseconds])".to_string(),
            actual: format!("{} arguments", vals.len()),
        }));
    }
    let secs = match &vals[0] {
        Value::Int(n) => *n,
        other => return type_error("datetime.fromEpoch", other),
    };
    let nanos = if vals.len() == 2 {
        match &vals[1] {
            Value::Int(n) => *n,
            _ => 0,
        }
    } else {
        0
    };
    let ts = jiff::Timestamp::new(secs, nanos as i32)
        .map_err(|e| temporal_parse_error("datetime.fromEpoch", e))?;
    let zoned = ts.to_zoned(jiff::tz::TimeZone::UTC);
    Ok(Value::DateTime(Box::new(zoned)))
}

pub(crate) fn builtin_datetime_fromepochmillis(vals: Vec<Value>) -> Result<Value, GqlError> {
    if vals.len() != 1 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "datetime.fromEpochMillis(millis)".to_string(),
            actual: format!("{} arguments", vals.len()),
        }));
    }
    let millis = match &vals[0] {
        Value::Int(n) => *n,
        other => return type_error("datetime.fromEpochMillis", other),
    };
    let ts = jiff::Timestamp::from_millisecond(millis)
        .map_err(|e| temporal_parse_error("datetime.fromEpochMillis", e))?;
    let zoned = ts.to_zoned(jiff::tz::TimeZone::UTC);
    Ok(Value::DateTime(Box::new(zoned)))
}

pub(crate) fn eval_datetime_constructor(vals: &[Value]) -> Result<Value, GqlError> {
    if vals.is_empty() {
        return Ok(Value::DateTime(Box::new(get_query_now())));
    }
    if vals.len() != 1 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "datetime() expects 0 or 1 argument".to_string(),
            actual: format!("{} arguments", vals.len()),
        }));
    }
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Str(s) => {
            if let Ok(zoned) = s.parse::<jiff::Zoned>() {
                return Ok(Value::DateTime(Box::new(zoned)));
            }
            let zoned =
                parse_datetime_with_offset(s).map_err(|e| temporal_parse_error("datetime", e))?;
            Ok(Value::DateTime(Box::new(zoned)))
        }
        Value::DateTime(zdt) => Ok(Value::DateTime(zdt.clone())),
        Value::LocalDateTime(dt) => {
            let zoned = dt
                .to_zoned(jiff::tz::TimeZone::UTC)
                .map_err(|e| temporal_parse_error("datetime", e))?;
            Ok(Value::DateTime(Box::new(zoned)))
        }
        Value::Date(d) => {
            let dt = d.to_datetime(jiff::civil::Time::midnight());
            let zoned = dt
                .to_zoned(jiff::tz::TimeZone::UTC)
                .map_err(|e| temporal_parse_error("datetime", e))?;
            Ok(Value::DateTime(Box::new(zoned)))
        }
        Value::Map(m) => {
            let base_date = extract_base_date(m)?;
            let base_time = extract_base_time(m)?;
            let base_is_local_dt = matches!(m.get("datetime"), Some(Value::LocalDateTime(_)));
            let base_zdt: Option<(jiff::civil::DateTime, jiff::tz::TimeZone)> =
                match m.get("datetime") {
                    Some(Value::DateTime(zdt)) => Some((
                        jiff::civil::DateTime::from(zdt.as_ref()),
                        zdt.time_zone().clone(),
                    )),
                    Some(Value::LocalDateTime(dt)) => Some((*dt, jiff::tz::TimeZone::UTC)),
                    _ => None,
                };

            let base_time_tz: Option<jiff::tz::TimeZone> = match m.get("time") {
                Some(Value::DateTime(zdt)) => Some(zdt.time_zone().clone()),
                Some(Value::Time(_, offset)) => {
                    let off =
                        jiff::tz::Offset::from_seconds(*offset).unwrap_or(jiff::tz::Offset::UTC);
                    Some(jiff::tz::TimeZone::fixed(off))
                }
                _ => None,
            };

            let has_date_fields = m.contains_key("week")
                || m.contains_key("ordinalDay")
                || m.contains_key("quarter")
                || m.contains_key("year")
                || m.contains_key("month")
                || m.contains_key("day")
                || base_date.is_some();
            let effective_base = if base_date.is_some() {
                base_date
            } else if let Some((bdt, _)) = &base_zdt {
                Some(
                    jiff::civil::Date::new(bdt.year(), bdt.month(), bdt.day())
                        .map_err(|e| temporal_parse_error("datetime", e))?,
                )
            } else {
                None
            };
            let date_part = if has_date_fields {
                resolve_date_from_map(m, &effective_base)?
            } else if let Some((bdt, _)) = &base_zdt {
                jiff::civil::Date::new(bdt.year(), bdt.month(), bdt.day())
                    .map_err(|e| temporal_parse_error("datetime", e))?
            } else {
                jiff::civil::Date::new(0, 1, 1).map_err(|e| temporal_parse_error("datetime", e))?
            };

            let (base_h, base_mi, base_s, base_ns) = if let Some(bt) = &base_time {
                (bt.hour(), bt.minute(), bt.second(), bt.subsec_nanosecond())
            } else if let Some((bdt, _)) = &base_zdt {
                (
                    bdt.hour(),
                    bdt.minute(),
                    bdt.second(),
                    bdt.subsec_nanosecond(),
                )
            } else {
                (0i8, 0i8, 0i8, 0i32)
            };
            let hour = if m.contains_key("hour") {
                map_get_int(m, "hour", 0)? as i8
            } else {
                base_h
            };
            let minute = if m.contains_key("minute") {
                map_get_int(m, "minute", 0)? as i8
            } else {
                base_mi
            };
            let second = if m.contains_key("second") {
                map_get_int(m, "second", 0)? as i8
            } else {
                base_s
            };
            let nanosecond = if m.contains_key("nanosecond")
                || m.contains_key("microsecond")
                || m.contains_key("millisecond")
            {
                let millis = map_get_int(m, "millisecond", 0)? * 1_000_000;
                let micros = map_get_int(m, "microsecond", 0)? * 1_000;
                let nanos = map_get_int(m, "nanosecond", 0)?;
                (millis + micros + nanos) as i32
            } else {
                base_ns
            };

            let tz_name = map_get_str(m, "timezone");

            let has_field_overrides = m.contains_key("year")
                || m.contains_key("month")
                || m.contains_key("day")
                || m.contains_key("hour")
                || m.contains_key("minute")
                || m.contains_key("second")
                || m.contains_key("nanosecond")
                || m.contains_key("microsecond")
                || m.contains_key("millisecond")
                || m.contains_key("week")
                || m.contains_key("ordinalDay")
                || m.contains_key("quarter")
                || m.contains_key("date")
                || m.contains_key("time");

            if let Some(ref tz_str) = tz_name {
                let new_tz =
                    parse_timezone(tz_str).map_err(|e| temporal_parse_error("datetime", e))?;

                if !has_field_overrides {
                    if let Some(Value::DateTime(base_zdt_val)) = m.get("datetime") {
                        let converted = base_zdt_val.with_time_zone(new_tz);
                        return Ok(Value::DateTime(Box::new(converted)));
                    }
                    if let Some(Value::LocalDateTime(ldt)) = m.get("datetime") {
                        let zoned = ldt
                            .to_zoned(new_tz)
                            .map_err(|e| temporal_parse_error("datetime", e))?;
                        return Ok(Value::DateTime(Box::new(zoned)));
                    }
                }

                let t = jiff::civil::Time::new(hour, minute, second, nanosecond)
                    .map_err(|e| temporal_parse_error("datetime", e))?;
                let dt = date_part.to_datetime(t);
                if let Some((_, ref base_tz)) = base_zdt {
                    if base_is_local_dt {
                        let zoned = dt
                            .to_zoned(new_tz)
                            .map_err(|e| temporal_parse_error("datetime", e))?;
                        Ok(Value::DateTime(Box::new(zoned)))
                    } else {
                        let with_base_tz = dt
                            .to_zoned(base_tz.clone())
                            .map_err(|e| temporal_parse_error("datetime", e))?;
                        let zoned = with_base_tz.with_time_zone(new_tz);
                        Ok(Value::DateTime(Box::new(zoned)))
                    }
                } else if let Some(ref time_tz) = base_time_tz {
                    let with_time_tz = dt
                        .to_zoned(time_tz.clone())
                        .map_err(|e| temporal_parse_error("datetime", e))?;
                    let zoned = with_time_tz.with_time_zone(new_tz);
                    Ok(Value::DateTime(Box::new(zoned)))
                } else {
                    let zoned = dt
                        .to_zoned(new_tz)
                        .map_err(|e| temporal_parse_error("datetime", e))?;
                    Ok(Value::DateTime(Box::new(zoned)))
                }
            } else {
                let t = jiff::civil::Time::new(hour, minute, second, nanosecond)
                    .map_err(|e| temporal_parse_error("datetime", e))?;
                let dt = date_part.to_datetime(t);
                let tz = if let Some((_, ref base_tz)) = base_zdt {
                    if base_is_local_dt {
                        jiff::tz::TimeZone::UTC
                    } else {
                        base_tz.clone()
                    }
                } else if let Some(ref time_tz) = base_time_tz {
                    time_tz.clone()
                } else {
                    jiff::tz::TimeZone::UTC
                };
                let zoned = dt
                    .to_zoned(tz)
                    .map_err(|e| temporal_parse_error("datetime", e))?;
                Ok(Value::DateTime(Box::new(zoned)))
            }
        }
        other => type_error("datetime", other),
    }
}

pub(crate) fn eval_localdatetime_constructor(vals: &[Value]) -> Result<Value, GqlError> {
    if vals.is_empty() {
        let now = get_query_now();
        return Ok(Value::LocalDateTime(jiff::civil::DateTime::from(now)));
    }
    if vals.len() != 1 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "localdatetime() expects 0 or 1 argument".to_string(),
            actual: format!("{} arguments", vals.len()),
        }));
    }
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Str(s) => {
            match parse_localdatetime_str(s) {
                Ok(dt) => Ok(Value::LocalDateTime(dt)),
                Err(_) => {
                    if let Some((y, m, d)) = parse_large_date_str(s) {
                        Ok(large_ldt_value(y, m, d, 0, 0, 0, 0))
                    } else if let Some(t_idx) = s.find(['T', 't']) {
                        let date_part = &s[..t_idx];
                        let time_part = &s[t_idx + 1..];
                        if let Some((y, mo, d)) = parse_large_date_str(date_part) {
                            let time_str = time_part.trim_end_matches(|c: char| {
                                !c.is_ascii_digit() && c != '.' && c != ':'
                            });
                            let time = time_str
                                .parse::<jiff::civil::Time>()
                                .unwrap_or(jiff::civil::Time::midnight());
                            Ok(large_ldt_value(
                                y,
                                mo,
                                d,
                                time.hour() as i32,
                                time.minute() as i32,
                                time.second() as i32,
                                time.subsec_nanosecond(),
                            ))
                        } else {
                            Err(temporal_parse_error(
                                "localdatetime",
                                format!("cannot parse datetime from '{s}'"),
                            ))
                        }
                    } else {
                        Err(temporal_parse_error(
                            "localdatetime",
                            format!("cannot parse datetime from '{s}'"),
                        ))
                    }
                }
            }
        }
        Value::LocalDateTime(dt) => Ok(Value::LocalDateTime(*dt)),
        Value::Date(d) => Ok(Value::LocalDateTime(
            d.to_datetime(jiff::civil::Time::midnight()),
        )),
        Value::DateTime(zdt) => Ok(Value::LocalDateTime(jiff::civil::DateTime::from(
            zdt.as_ref(),
        ))),
        Value::Map(m) => {
            let base_date = extract_base_date(m)?;
            let base_time = extract_base_time(m)?;
            let base_dt = match m.get("datetime") {
                Some(Value::LocalDateTime(dt)) => Some(*dt),
                Some(Value::DateTime(zdt)) => Some(jiff::civil::DateTime::from(zdt.as_ref())),
                _ => None,
            };

            let has_date_fields = m.contains_key("week")
                || m.contains_key("ordinalDay")
                || m.contains_key("quarter")
                || m.contains_key("year")
                || m.contains_key("month")
                || m.contains_key("day")
                || base_date.is_some();
            let effective_base = if base_date.is_some() {
                base_date
            } else if let Some(bdt) = &base_dt {
                Some(
                    jiff::civil::Date::new(bdt.year(), bdt.month(), bdt.day())
                        .map_err(|e| temporal_parse_error("localdatetime", e))?,
                )
            } else {
                None
            };
            let date_part = if has_date_fields {
                resolve_date_from_map(m, &effective_base)?
            } else if let Some(bdt) = &base_dt {
                jiff::civil::Date::new(bdt.year(), bdt.month(), bdt.day())
                    .map_err(|e| temporal_parse_error("localdatetime", e))?
            } else {
                jiff::civil::Date::new(0, 1, 1)
                    .map_err(|e| temporal_parse_error("localdatetime", e))?
            };

            let (base_h, base_mi, base_s, base_ns) = if let Some(bt) = &base_time {
                (bt.hour(), bt.minute(), bt.second(), bt.subsec_nanosecond())
            } else if let Some(bdt) = &base_dt {
                (
                    bdt.hour(),
                    bdt.minute(),
                    bdt.second(),
                    bdt.subsec_nanosecond(),
                )
            } else {
                (0i8, 0i8, 0i8, 0i32)
            };
            let hour = if m.contains_key("hour") {
                map_get_int(m, "hour", 0)? as i8
            } else {
                base_h
            };
            let minute = if m.contains_key("minute") {
                map_get_int(m, "minute", 0)? as i8
            } else {
                base_mi
            };
            let second = if m.contains_key("second") {
                map_get_int(m, "second", 0)? as i8
            } else {
                base_s
            };
            let nanosecond = if m.contains_key("nanosecond")
                || m.contains_key("microsecond")
                || m.contains_key("millisecond")
            {
                let millis = map_get_int(m, "millisecond", 0)? * 1_000_000;
                let micros = map_get_int(m, "microsecond", 0)? * 1_000;
                let nanos = map_get_int(m, "nanosecond", 0)?;
                (millis + micros + nanos) as i32
            } else {
                base_ns
            };

            let t = jiff::civil::Time::new(hour, minute, second, nanosecond)
                .map_err(|e| temporal_parse_error("localdatetime", e))?;
            Ok(Value::LocalDateTime(date_part.to_datetime(t)))
        }
        other => type_error("localdatetime", other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn datetime_from_string() {
        match eval_datetime_constructor(&[Value::Str("2015-07-21T21:40:32+01:00".into())]).unwrap()
        {
            Value::DateTime(z) => assert_eq!(z.year(), 2015),
            _ => panic!("DT"),
        }
    }
    #[test]
    fn datetime_from_null() {
        assert_eq!(
            eval_datetime_constructor(&[Value::Null]).unwrap(),
            Value::Null
        );
    }
    #[test]
    fn ldt_from_string() {
        match eval_localdatetime_constructor(&[Value::Str("2024-03-15T10:30:00".into())]).unwrap() {
            Value::LocalDateTime(dt) => assert_eq!(dt.year(), 2024),
            _ => panic!("LDT"),
        }
    }
    #[test]
    fn ldt_from_null() {
        assert_eq!(
            eval_localdatetime_constructor(&[Value::Null]).unwrap(),
            Value::Null
        );
    }
    #[test]
    fn fromepoch_zero() {
        match builtin_datetime_fromepoch(vec![Value::Int(0)]).unwrap() {
            Value::DateTime(z) => assert_eq!(z.year(), 1970),
            _ => panic!("DT"),
        }
    }
    #[test]
    fn fromepoch_bad() {
        assert!(builtin_datetime_fromepoch(vec![]).is_err());
    }
    #[test]
    fn fromepochmillis_zero() {
        match builtin_datetime_fromepochmillis(vec![Value::Int(0)]).unwrap() {
            Value::DateTime(z) => assert_eq!(z.year(), 1970),
            _ => panic!("DT"),
        }
    }
    #[test]
    fn fromepochmillis_bad() {
        assert!(builtin_datetime_fromepochmillis(vec![]).is_err());
    }
}
