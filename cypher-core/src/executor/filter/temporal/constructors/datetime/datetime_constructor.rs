//! `datetime()` constructor — full evaluator covering string, value, and map
//! inputs (including timezone resolution and field overrides).

use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

use super::super::super::super::builtins::type_error;
use super::super::super::super::get_query_now;
use super::super::super::parsing::parse_datetime_with_offset;
use super::super::super::{map_get_int, map_get_str, parse_timezone, temporal_parse_error};
use super::super::{extract_base_date, extract_base_time, resolve_date_from_map};

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
        Value::Map(m) => from_map(m),
        other => type_error("datetime", other),
    }
}

fn from_map(m: &indexmap::IndexMap<Box<str>, Value>) -> Result<Value, GqlError> {
    let base_date = extract_base_date(m)?;
    let base_time = extract_base_time(m)?;
    let base_is_local_dt = matches!(m.get("datetime"), Some(Value::LocalDateTime(_)));
    let base_zdt: Option<(jiff::civil::DateTime, jiff::tz::TimeZone)> = match m.get("datetime") {
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
            let off = jiff::tz::Offset::from_seconds(*offset).unwrap_or(jiff::tz::Offset::UTC);
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
        let new_tz = parse_timezone(tz_str).map_err(|e| temporal_parse_error("datetime", e))?;

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
