//! `localdatetime()` constructor — full evaluator covering string, value, and
//! map inputs.

use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

use super::super::super::super::builtins::type_error;
use super::super::super::super::get_query_now;
use super::super::super::parsing::{
    large_ldt_value, parse_large_date_str, parse_localdatetime_str,
};
use super::super::super::{map_get_int, temporal_parse_error};
use super::super::{extract_base_date, extract_base_time, resolve_date_from_map};

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
        Value::Str(s) => from_string(s),
        Value::LocalDateTime(dt) => Ok(Value::LocalDateTime(*dt)),
        Value::Date(d) => Ok(Value::LocalDateTime(
            d.to_datetime(jiff::civil::Time::midnight()),
        )),
        Value::DateTime(zdt) => Ok(Value::LocalDateTime(jiff::civil::DateTime::from(
            zdt.as_ref(),
        ))),
        Value::Map(m) => from_map(m),
        other => type_error("localdatetime", other),
    }
}

fn from_string(s: &str) -> Result<Value, GqlError> {
    match parse_localdatetime_str(s) {
        Ok(dt) => Ok(Value::LocalDateTime(dt)),
        Err(_) => {
            if let Some((y, m, d)) = parse_large_date_str(s) {
                Ok(large_ldt_value(y, m, d, 0, 0, 0, 0))
            } else if let Some(t_idx) = s.find(['T', 't']) {
                let date_part = &s[..t_idx];
                let time_part = &s[t_idx + 1..];
                if let Some((y, mo, d)) = parse_large_date_str(date_part) {
                    let time_str = time_part
                        .trim_end_matches(|c: char| !c.is_ascii_digit() && c != '.' && c != ':');
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

fn from_map(m: &indexmap::IndexMap<Box<str>, Value>) -> Result<Value, GqlError> {
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
        jiff::civil::Date::new(0, 1, 1).map_err(|e| temporal_parse_error("localdatetime", e))?
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
