//! Temporal constructor functions: date(), time(), localtime(), datetime(),
//! localdatetime(), duration(), and datetime.fromEpoch/fromEpochMillis.

use crate::error::{ExecutionError, GqlError};
use crate::types::{CypherDuration, Value};

use super::builtins::type_error;
use super::get_query_now;
use super::parsing::{
    expand_compact_time, extract_offset_from_string, large_date_value, large_ldt_value,
    parse_date_str, parse_datetime_with_offset, parse_duration_str, parse_large_date_str,
    parse_localdatetime_str, strip_tz_offset_from_time,
};
use super::{map_get_int, map_get_str, parse_timezone, temporal_parse_error};

pub(crate) fn builtin_temporal_realtime_date(vals: Vec<Value>) -> Result<Value, GqlError> {
    if vals.first().map(|v| v.is_null()).unwrap_or(false) {
        return Ok(Value::Null);
    }
    if !vals.is_empty() {
        return eval_date_constructor(&vals);
    }
    let today = jiff::civil::Date::from(get_query_now());
    Ok(Value::Date(today))
}

pub(crate) fn builtin_temporal_realtime_datetime(vals: Vec<Value>) -> Result<Value, GqlError> {
    if vals.first().map(|v| v.is_null()).unwrap_or(false) {
        return Ok(Value::Null);
    }
    if !vals.is_empty() {
        return eval_datetime_constructor(&vals);
    }
    Ok(Value::DateTime(Box::new(get_query_now())))
}

pub(crate) fn builtin_temporal_realtime_localdatetime(vals: Vec<Value>) -> Result<Value, GqlError> {
    if vals.first().map(|v| v.is_null()).unwrap_or(false) {
        return Ok(Value::Null);
    }
    if !vals.is_empty() {
        return eval_localdatetime_constructor(&vals);
    }
    let now = get_query_now();
    Ok(Value::LocalDateTime(jiff::civil::DateTime::from(now)))
}

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

pub(crate) fn builtin_datetime_fromepoch(vals: Vec<Value>) -> Result<Value, GqlError> {
    // datetime.fromepoch(seconds, nanoseconds)
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

fn compute_week_date(
    year: i64,
    week: i64,
    day_of_week: i64,
) -> Result<jiff::civil::Date, GqlError> {
    let jan4 =
        jiff::civil::Date::new(year as i16, 1, 4).map_err(|e| temporal_parse_error("date", e))?;
    let jan4_dow = jan4.weekday().to_monday_one_offset() as i64;
    let week1_monday_ordinal = 4 - jan4_dow + 1;
    let target_ordinal = week1_monday_ordinal + (week - 1) * 7 + (day_of_week - 1);
    let d =
        jiff::civil::Date::new(year as i16, 1, 1).map_err(|e| temporal_parse_error("date", e))?;
    d.checked_add(jiff::Span::new().days(target_ordinal - 1))
        .map_err(|e| temporal_parse_error("date", e))
}

/// Resolve the date part from a map, handling week-based dates.
fn resolve_date_from_map(
    m: &indexmap::IndexMap<Box<str>, Value>,
    base_date: &Option<jiff::civil::Date>,
) -> Result<jiff::civil::Date, GqlError> {
    if m.contains_key("week") {
        let year = if m.contains_key("year") {
            map_get_int(m, "year", 0)?
        } else if let Some(bd) = base_date {
            // Use ISO week year (not Gregorian year) since week dates cross year boundaries.
            bd.iso_week_date().year() as i64
        } else {
            0
        };
        let week = map_get_int(m, "week", 1)?;
        let day_of_week = if m.contains_key("dayOfWeek") {
            map_get_int(m, "dayOfWeek", 1)?
        } else if let Some(bd) = base_date {
            bd.weekday().to_monday_one_offset() as i64
        } else {
            1
        };
        compute_week_date(year, week, day_of_week)
    } else if m.contains_key("ordinalDay") {
        let year = if m.contains_key("year") {
            map_get_int(m, "year", 0)?
        } else if let Some(bd) = base_date {
            bd.year() as i64
        } else {
            0
        };
        let ordinal = map_get_int(m, "ordinalDay", 1)?;
        let d = jiff::civil::Date::new(year as i16, 1, 1)
            .map_err(|e| temporal_parse_error("date", e))?;
        d.checked_add(jiff::Span::new().days(ordinal - 1))
            .map_err(|e| temporal_parse_error("date", e))
    } else if m.contains_key("quarter") {
        let year = if m.contains_key("year") {
            map_get_int(m, "year", 0)?
        } else if let Some(bd) = base_date {
            bd.year() as i64
        } else {
            0
        };
        let quarter = map_get_int(m, "quarter", 1)?;
        if m.contains_key("dayOfQuarter") {
            let day_of_quarter = map_get_int(m, "dayOfQuarter", 1)?;
            let month = ((quarter - 1) * 3 + 1) as i8;
            let d = jiff::civil::Date::new(year as i16, month, 1)
                .map_err(|e| temporal_parse_error("date", e))?;
            d.checked_add(jiff::Span::new().days(day_of_quarter - 1))
                .map_err(|e| temporal_parse_error("date", e))
        } else if let Some(bd) = base_date {
            let month_in_quarter = (bd.month() as i64 - 1) % 3;
            let new_month = (quarter - 1) * 3 + 1 + month_in_quarter;
            jiff::civil::Date::new(year as i16, new_month as i8, bd.day())
                .map_err(|e| temporal_parse_error("date", e))
        } else {
            let month = ((quarter - 1) * 3 + 1) as i8;
            jiff::civil::Date::new(year as i16, month, 1)
                .map_err(|e| temporal_parse_error("date", e))
        }
    } else {
        let year = if m.contains_key("year") {
            map_get_int(m, "year", 0)? as i16
        } else if let Some(bd) = base_date {
            bd.year()
        } else {
            0
        };
        let month = if m.contains_key("month") {
            map_get_int(m, "month", 1)? as i8
        } else if let Some(bd) = base_date {
            bd.month()
        } else {
            1
        };
        let day = if m.contains_key("day") {
            map_get_int(m, "day", 1)? as i8
        } else if let Some(bd) = base_date {
            bd.day()
        } else {
            1
        };
        jiff::civil::Date::new(year, month, day).map_err(|e| temporal_parse_error("date", e))
    }
}

/// Extract a base date from a map's "date" key (handles Date, LocalDateTime, DateTime).
fn extract_base_date(
    m: &indexmap::IndexMap<Box<str>, Value>,
) -> Result<Option<jiff::civil::Date>, GqlError> {
    match m.get("date") {
        Some(Value::Date(d)) => Ok(Some(*d)),
        Some(Value::LocalDateTime(dt)) => Ok(Some(
            jiff::civil::Date::new(dt.year(), dt.month(), dt.day())
                .map_err(|e| temporal_parse_error("date", e))?,
        )),
        Some(Value::DateTime(zdt)) => {
            let dt = jiff::civil::DateTime::from(zdt.as_ref());
            Ok(Some(
                jiff::civil::Date::new(dt.year(), dt.month(), dt.day())
                    .map_err(|e| temporal_parse_error("date", e))?,
            ))
        }
        Some(Value::Null) => Ok(None),
        None => Ok(None),
        Some(other) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "Date, LocalDateTime, or DateTime".to_string(),
            actual: other.type_name().to_string(),
        })),
    }
}

/// Extract a base time from a map's "time" key (handles LocalTime, DateTime, LocalDateTime).
fn extract_base_time(
    m: &indexmap::IndexMap<Box<str>, Value>,
) -> Result<Option<jiff::civil::Time>, GqlError> {
    match m.get("time") {
        Some(Value::LocalTime(t)) => Ok(Some(*t)),
        Some(Value::Time(t, _)) => Ok(Some(*t)),
        Some(Value::LocalDateTime(dt)) => Ok(Some(
            jiff::civil::Time::new(dt.hour(), dt.minute(), dt.second(), dt.subsec_nanosecond())
                .map_err(|e| temporal_parse_error("time", e))?,
        )),
        Some(Value::DateTime(zdt)) => {
            let dt = jiff::civil::DateTime::from(zdt.as_ref());
            Ok(Some(
                jiff::civil::Time::new(dt.hour(), dt.minute(), dt.second(), dt.subsec_nanosecond())
                    .map_err(|e| temporal_parse_error("time", e))?,
            ))
        }
        Some(Value::Null) => Ok(None),
        None => Ok(None),
        Some(other) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "LocalTime, LocalDateTime, or DateTime".to_string(),
            actual: other.type_name().to_string(),
        })),
    }
}

/// Convert a jiff::Zoned (at epoch date) to Value::Time by extracting time + offset.
fn zoned_to_time_value(z: &jiff::Zoned) -> Value {
    let dt = jiff::civil::DateTime::from(z);
    let t = dt.time();
    let offset_secs = z.offset().seconds();
    Value::Time(t, offset_secs)
}

pub(crate) fn eval_date_constructor(vals: &[Value]) -> Result<Value, GqlError> {
    if vals.is_empty() {
        let today = jiff::civil::Date::from(get_query_now());
        return Ok(Value::Date(today));
    }
    if vals.len() != 1 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "date() expects 0 or 1 argument".to_string(),
            actual: format!("{} arguments", vals.len()),
        }));
    }
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Str(s) => {
            match parse_date_str(s) {
                Ok(d) => Ok(Value::Date(d)),
                Err(_) => {
                    // Try large-year fallback (e.g. -999999999-01-01)
                    if let Some((y, m, d)) = parse_large_date_str(s) {
                        Ok(large_date_value(y, m, d))
                    } else {
                        Err(temporal_parse_error(
                            "date",
                            format!("cannot parse date from '{s}'"),
                        ))
                    }
                }
            }
        }
        // Accept temporal values directly — extract date component
        Value::Date(d) => Ok(Value::Date(*d)),
        Value::LocalDateTime(dt) => Ok(Value::Date(
            jiff::civil::Date::new(dt.year(), dt.month(), dt.day())
                .map_err(|e| temporal_parse_error("date", e))?,
        )),
        Value::DateTime(zdt) => {
            let dt = jiff::civil::DateTime::from(zdt.as_ref());
            Ok(Value::Date(
                jiff::civil::Date::new(dt.year(), dt.month(), dt.day())
                    .map_err(|e| temporal_parse_error("date", e))?,
            ))
        }
        Value::Map(m) => {
            let base_date = extract_base_date(m)?;
            let d = resolve_date_from_map(m, &base_date)?;
            Ok(Value::Date(d))
        }
        other => type_error("date", other),
    }
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
            // time() always returns a ZonedTime; use explicit offset if present, else UTC
            let tz = extract_offset_from_string(s).unwrap_or(jiff::tz::TimeZone::UTC);
            let epoch_dt = jiff::civil::Date::new(1970, 1, 1).unwrap().to_datetime(t);
            let zoned = epoch_dt
                .to_zoned(tz)
                .map_err(|e| temporal_parse_error("time", e))?;
            Ok(zoned_to_time_value(&zoned))
        }
        // Accept temporal values directly — extract time with timezone
        // time() on LocalTime returns a ZonedTime (UTC by default per Cypher spec)
        Value::LocalTime(t) => {
            Ok(Value::Time(*t, 0)) // UTC offset = 0
        }
        Value::Time(t, offset) => {
            // time() on a Time just returns it as-is
            Ok(Value::Time(*t, *offset))
        }
        Value::DateTime(zdt) => {
            // time() on a DateTime preserves the timezone as a zoned time
            let dt = jiff::civil::DateTime::from(zdt.as_ref());
            let t =
                jiff::civil::Time::new(dt.hour(), dt.minute(), dt.second(), dt.subsec_nanosecond())
                    .map_err(|e| temporal_parse_error("time", e))?;
            let offset_secs = zdt.offset().seconds();
            Ok(Value::Time(t, offset_secs))
        }
        Value::LocalDateTime(dt) => {
            // time(LocalDateTime) -> ZonedTime with UTC timezone
            let t =
                jiff::civil::Time::new(dt.hour(), dt.minute(), dt.second(), dt.subsec_nanosecond())
                    .map_err(|e| temporal_parse_error("time", e))?;
            Ok(Value::Time(t, 0)) // UTC offset = 0
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
            // Combine millisecond + microsecond + nanosecond (Cypher spec: all additive)
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
            let epoch_dt = jiff::civil::Date::new(1970, 1, 1).unwrap().to_datetime(t);
            // Get base timezone if base is a ZonedTime (could be DateTime or Time)
            let base_tz = if let Some(Value::DateTime(base_zdt)) = m.get("time") {
                Some(base_zdt.time_zone().clone())
            } else if let Some(Value::Time(_, offset)) = m.get("time") {
                let off = jiff::tz::Offset::from_seconds(*offset).unwrap_or(jiff::tz::Offset::UTC);
                Some(jiff::tz::TimeZone::fixed(off))
            } else {
                None
            };

            // Determine the target timezone
            let target_tz = if let Some(ref tz_str) = tz_name {
                parse_timezone(tz_str).map_err(|e| temporal_parse_error("time", e))?
            } else if let Some(ref tz) = base_tz {
                tz.clone()
            } else {
                jiff::tz::TimeZone::UTC
            };

            // When the target timezone differs from the base timezone, we must
            // construct the time in the BASE timezone first (applying any overrides
            // to the base's civil time), then convert the UTC instant to the target tz.
            if let (Some(_), Some(ref btz)) = (&tz_name, &base_tz) {
                // Create zoned at base tz (with any overridden components),
                // then convert to target tz.
                let base_zoned = epoch_dt
                    .to_zoned(btz.clone())
                    .map_err(|e| temporal_parse_error("time", e))?;
                let converted = base_zoned.with_time_zone(target_tz);
                return Ok(zoned_to_time_value(&converted));
            }

            // Create zoned time
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
        // Accept temporal values directly — extract time component
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
            // Combine millisecond + microsecond + nanosecond (Cypher spec: all additive)
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
        // Accept temporal values directly
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
            // Track whether the "datetime" base is a LocalDateTime (has no real timezone).
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

            // Extract timezone from "time" key when it is a ZonedTime.
            // This preserves e.g. +01:00 from time({..., timezone: '+01:00'}).
            let base_time_tz: Option<jiff::tz::TimeZone> = match m.get("time") {
                Some(Value::DateTime(zdt)) => Some(zdt.time_zone().clone()),
                Some(Value::Time(_, offset)) => {
                    let off =
                        jiff::tz::Offset::from_seconds(*offset).unwrap_or(jiff::tz::Offset::UTC);
                    Some(jiff::tz::TimeZone::fixed(off))
                }
                _ => None,
            };

            // Resolve date part using shared logic (handles week, ordinalDay, quarter)
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

            // Resolve time part
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
            // Combine millisecond + microsecond + nanosecond (Cypher spec: all additive)
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

            // When base_zdt provides the datetime and timezone is overridden,
            // convert the instant to the new timezone (same UTC moment, different wall clock).
            // Only do this when no field overrides are applied beyond what's in base_zdt.
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
                    // Pure timezone conversion for real ZonedDatetime: same instant, new TZ.
                    if let Some(Value::DateTime(base_zdt_val)) = m.get("datetime") {
                        let converted = base_zdt_val.with_time_zone(new_tz);
                        return Ok(Value::DateTime(Box::new(converted)));
                    }
                    // LocalDateTime has no timezone: just apply new TZ as wall-clock timezone.
                    if let Some(Value::LocalDateTime(ldt)) = m.get("datetime") {
                        let zoned = ldt
                            .to_zoned(new_tz)
                            .map_err(|e| temporal_parse_error("datetime", e))?;
                        return Ok(Value::DateTime(Box::new(zoned)));
                    }
                }

                // Field overrides + timezone: apply overrides in base TZ, then convert to new TZ.
                let t = jiff::civil::Time::new(hour, minute, second, nanosecond)
                    .map_err(|e| temporal_parse_error("datetime", e))?;
                let dt = date_part.to_datetime(t);
                if let Some((_, ref base_tz)) = base_zdt {
                    if base_is_local_dt {
                        // LocalDateTime: wall-clock time is already resolved; just set new TZ.
                        let zoned = dt
                            .to_zoned(new_tz)
                            .map_err(|e| temporal_parse_error("datetime", e))?;
                        Ok(Value::DateTime(Box::new(zoned)))
                    } else {
                        // Real ZonedDateTime: apply overrides in base TZ, then convert.
                        let with_base_tz = dt
                            .to_zoned(base_tz.clone())
                            .map_err(|e| temporal_parse_error("datetime", e))?;
                        let zoned = with_base_tz.with_time_zone(new_tz);
                        Ok(Value::DateTime(Box::new(zoned)))
                    }
                } else if let Some(ref time_tz) = base_time_tz {
                    // "time" key provided a timezone: create in that TZ, then convert.
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
                // Use timezone from "datetime" key (if it's a real ZonedDateTime),
                // "time" key, or default to UTC. LocalDateTime base has no TZ.
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
                    // Try large-year fallback (e.g. -999999999-01-01)
                    if let Some((y, m, d)) = parse_large_date_str(s) {
                        Ok(large_ldt_value(y, m, d, 0, 0, 0, 0))
                    } else if let Some(t_idx) = s.find(['T', 't']) {
                        let date_part = &s[..t_idx];
                        let time_part = &s[t_idx + 1..];
                        if let Some((y, mo, d)) = parse_large_date_str(date_part) {
                            // Parse time component
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
        // Accept temporal values directly
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

            // Resolve date part — handles week, ordinalDay, quarter, or standard year/month/day
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

            // Resolve time part
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
            // Combine millisecond + microsecond + nanosecond (Cypher spec: all additive)
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
            // Helper: get a numeric value as f64, supporting both Int and Float.
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

            // Phase 1: years + months → total months
            let int_years = raw_years as i64;
            let frac_years = raw_years - int_years as f64;
            let total_month_float = frac_years * 12.0 + raw_months;
            let int_months_raw = total_month_float as i64;
            let frac_months = total_month_float - int_months_raw as f64;
            let total_months = int_years * 12 + int_months_raw;

            // Phase 2: fractional months + weeks + days → total days
            // 1 month ≈ 30.436875 days (2_629_746 s / 86400 s/day)
            let total_days_float =
                frac_months * (2_629_746.0 / 86_400.0) + raw_weeks * 7.0 + raw_days;
            let int_days = total_days_float.trunc() as i64;
            let frac_days = total_days_float - int_days as f64;

            // Phase 3: fractional days + hours + minutes + seconds → nanos
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
    fn date_from_string() {
        match eval_date_constructor(&[Value::Str("2024-03-15".into())]).unwrap() {
            Value::Date(d) => {
                assert_eq!(d.year(), 2024);
                assert_eq!(d.day(), 15);
            }
            _ => panic!("expected Date"),
        }
    }
    #[test]
    fn date_from_null() {
        assert_eq!(eval_date_constructor(&[Value::Null]).unwrap(), Value::Null);
    }
    #[test]
    fn date_from_map() {
        let mut m = indexmap::IndexMap::new();
        m.insert("year".into(), Value::Int(2024));
        m.insert("month".into(), Value::Int(6));
        m.insert("day".into(), Value::Int(15));
        match eval_date_constructor(&[Value::Map(m)]).unwrap() {
            Value::Date(d) => assert_eq!(d.day(), 15),
            _ => panic!("Date"),
        }
    }
    #[test]
    fn date_too_many() {
        assert!(eval_date_constructor(&[Value::Int(1), Value::Int(2)]).is_err());
    }
    #[test]
    fn date_large_year() {
        assert!(matches!(
            eval_date_constructor(&[Value::Str("-999999999-01-01".into())]).unwrap(),
            Value::Map(_)
        ));
    }
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
