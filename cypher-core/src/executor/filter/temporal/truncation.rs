//! Temporal truncation functions (date.truncate, datetime.truncate, etc.)

use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

use super::super::builtins::type_error;
use super::{map_get_str, parse_timezone, temporal_parse_error};

/// Extract a date (year, month, day) from any temporal value.
fn extract_date(val: &Value) -> Option<jiff::civil::Date> {
    match val {
        Value::Date(d) => Some(*d),
        Value::LocalDateTime(dt) => Some(dt.date()),
        Value::DateTime(zdt) => Some(jiff::civil::Date::from(&**zdt)),
        _ => None,
    }
}

/// Extract a time (hour, minute, second, nanosecond) from any temporal value.
fn extract_time(val: &Value) -> Option<jiff::civil::Time> {
    match val {
        Value::LocalTime(t) => Some(*t),
        Value::Time(t, _) => Some(*t),
        Value::LocalDateTime(dt) => Some(dt.time()),
        Value::DateTime(zdt) => Some(jiff::civil::Time::from(&**zdt)),
        _ => None,
    }
}

/// Extract timezone from a temporal value, if available.
fn extract_tz(val: &Value) -> Option<jiff::tz::TimeZone> {
    match val {
        Value::Time(_, offset) => {
            let off = jiff::tz::Offset::from_seconds(*offset).unwrap_or(jiff::tz::Offset::UTC);
            Some(jiff::tz::TimeZone::fixed(off))
        }
        Value::DateTime(zdt) => Some(zdt.time_zone().clone()),
        _ => None,
    }
}

/// Truncate a date to the given unit, returning (year, month, day).
fn truncate_date_fields(d: jiff::civil::Date, unit: &str) -> Result<jiff::civil::Date, GqlError> {
    let y = d.year() as i64;
    match unit {
        "millennium" => {
            let ty = (y / 1000) * 1000;
            jiff::civil::Date::new(ty as i16, 1, 1).map_err(|e| temporal_parse_error("truncate", e))
        }
        "century" => {
            let ty = (y / 100) * 100;
            jiff::civil::Date::new(ty as i16, 1, 1).map_err(|e| temporal_parse_error("truncate", e))
        }
        "decade" => {
            let ty = (y / 10) * 10;
            jiff::civil::Date::new(ty as i16, 1, 1).map_err(|e| temporal_parse_error("truncate", e))
        }
        "year" => {
            jiff::civil::Date::new(d.year(), 1, 1).map_err(|e| temporal_parse_error("truncate", e))
        }
        "weekyear" => {
            // ISO week year: find the Monday of week 1 of the ISO week year.
            let iso = d.iso_week_date();
            let iso_year = iso.year();
            // Jan 4 is always in week 1 of the ISO year.
            let jan4 = jiff::civil::Date::new(iso_year, 1, 4)
                .map_err(|e| temporal_parse_error("truncate", e))?;
            let jan4_dow = jan4.weekday().to_monday_one_offset() as i64;
            // Monday of week 1
            let monday_ordinal = 4 - jan4_dow + 1;
            let base = jiff::civil::Date::new(iso_year, 1, 1)
                .map_err(|e| temporal_parse_error("truncate", e))?;
            base.checked_add(jiff::Span::new().days(monday_ordinal - 1))
                .map_err(|e| temporal_parse_error("truncate", e))
        }
        "quarter" => {
            let q = ((d.month() as i64 - 1) / 3) * 3 + 1;
            jiff::civil::Date::new(d.year(), q as i8, 1)
                .map_err(|e| temporal_parse_error("truncate", e))
        }
        "month" => jiff::civil::Date::new(d.year(), d.month(), 1)
            .map_err(|e| temporal_parse_error("truncate", e)),
        "week" => {
            // Monday of the current week (ISO weeks start on Monday)
            let dow = d.weekday().to_monday_one_offset() as i64; // 1=Mon..7=Sun
            d.checked_add(jiff::Span::new().days(1 - dow))
                .map_err(|e| temporal_parse_error("truncate", e))
        }
        "day" | "hour" | "minute" | "second" | "millisecond" | "microsecond" => Ok(d),
        other => Err(temporal_parse_error(
            "truncate",
            format!("unknown unit '{other}'"),
        )),
    }
}

/// Truncate a time to the given unit.
fn truncate_time_fields(t: jiff::civil::Time, unit: &str) -> Result<jiff::civil::Time, GqlError> {
    match unit {
        "day" | "millennium" | "century" | "decade" | "year" | "weekyear" | "quarter" | "month"
        | "week" => Ok(jiff::civil::Time::midnight()),
        "hour" => jiff::civil::Time::new(t.hour(), 0, 0, 0)
            .map_err(|e| temporal_parse_error("truncate", e)),
        "minute" => jiff::civil::Time::new(t.hour(), t.minute(), 0, 0)
            .map_err(|e| temporal_parse_error("truncate", e)),
        "second" => jiff::civil::Time::new(t.hour(), t.minute(), t.second(), 0)
            .map_err(|e| temporal_parse_error("truncate", e)),
        "millisecond" => {
            let ns = (t.subsec_nanosecond() / 1_000_000) * 1_000_000;
            jiff::civil::Time::new(t.hour(), t.minute(), t.second(), ns)
                .map_err(|e| temporal_parse_error("truncate", e))
        }
        "microsecond" => {
            let ns = (t.subsec_nanosecond() / 1_000) * 1_000;
            jiff::civil::Time::new(t.hour(), t.minute(), t.second(), ns)
                .map_err(|e| temporal_parse_error("truncate", e))
        }
        other => Err(temporal_parse_error(
            "truncate",
            format!("unknown unit '{other}'"),
        )),
    }
}

/// Apply map overrides for date fields: day, dayOfWeek.
fn apply_date_overrides(
    d: jiff::civil::Date,
    overrides: &indexmap::IndexMap<Box<str>, Value>,
) -> Result<jiff::civil::Date, GqlError> {
    let mut result = d;
    if let Some(Value::Int(day)) = overrides.get("day") {
        result = jiff::civil::Date::new(result.year(), result.month(), *day as i8)
            .map_err(|e| temporal_parse_error("truncate", e))?;
    }
    if let Some(Value::Int(dow)) = overrides.get("dayOfWeek") {
        // Current day-of-week and desired day-of-week (1=Monday)
        let current_dow = result.weekday().to_monday_one_offset() as i64;
        let offset = *dow - current_dow;
        result = result
            .checked_add(jiff::Span::new().days(offset))
            .map_err(|e| temporal_parse_error("truncate", e))?;
    }
    Ok(result)
}

/// Apply map overrides for time fields: hour, minute, second, millisecond, microsecond, nanosecond.
fn apply_time_overrides(
    t: jiff::civil::Time,
    overrides: &indexmap::IndexMap<Box<str>, Value>,
) -> Result<jiff::civil::Time, GqlError> {
    let hour = if let Some(Value::Int(v)) = overrides.get("hour") {
        *v as i8
    } else {
        t.hour()
    };
    let minute = if let Some(Value::Int(v)) = overrides.get("minute") {
        *v as i8
    } else {
        t.minute()
    };
    let second = if let Some(Value::Int(v)) = overrides.get("second") {
        *v as i8
    } else {
        t.second()
    };
    // Sub-second components are hierarchical: millisecond > microsecond > nanosecond
    let base_ns = t.subsec_nanosecond();
    let base_ms = base_ns / 1_000_000;
    let base_us = (base_ns % 1_000_000) / 1_000;
    let base_ns_only = base_ns % 1_000;

    let ms = if let Some(Value::Int(v)) = overrides.get("millisecond") {
        *v as i32
    } else {
        base_ms
    };
    let us = if let Some(Value::Int(v)) = overrides.get("microsecond") {
        *v as i32
    } else if overrides.contains_key("millisecond") {
        0
    } else {
        base_us
    };
    let ns_only = if let Some(Value::Int(v)) = overrides.get("nanosecond") {
        *v as i32
    } else if overrides.contains_key("microsecond") || overrides.contains_key("millisecond") {
        0
    } else {
        base_ns_only
    };
    let nanosecond = ms * 1_000_000 + us * 1_000 + ns_only;
    jiff::civil::Time::new(hour, minute, second, nanosecond)
        .map_err(|e| temporal_parse_error("truncate", e))
}

/// Convert a jiff::Zoned (at epoch date) to Value::Time.
fn zoned_to_time_value(z: &jiff::Zoned) -> Value {
    let dt = jiff::civil::DateTime::from(z);
    let t = dt.time();
    Value::Time(t, z.offset().seconds())
}

pub(crate) fn eval_date_truncate(vals: &[Value]) -> Result<Value, GqlError> {
    if vals.len() < 2 || vals.len() > 3 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "date.truncate(unit, temporal[, map])".to_string(),
            actual: format!("{} arguments", vals.len()),
        }));
    }
    let unit = match &vals[0] {
        Value::Str(s) => s.to_ascii_lowercase(),
        other => return type_error("date.truncate", other),
    };
    let d = extract_date(&vals[1])
        .ok_or_else(|| temporal_parse_error("date.truncate", "expected a temporal with date"))?;
    let truncated = truncate_date_fields(d, &unit)?;
    let overrides = match vals.get(2) {
        Some(Value::Map(m)) => m,
        None => &indexmap::IndexMap::new(),
        _ => return type_error("date.truncate", &vals[2]),
    };
    let result = apply_date_overrides(truncated, overrides)?;
    Ok(Value::Date(result))
}

pub(crate) fn eval_datetime_truncate(vals: &[Value]) -> Result<Value, GqlError> {
    if vals.len() < 2 || vals.len() > 3 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "datetime.truncate(unit, temporal[, map])".to_string(),
            actual: format!("{} arguments", vals.len()),
        }));
    }
    let unit = match &vals[0] {
        Value::Str(s) => s.to_ascii_lowercase(),
        other => return type_error("datetime.truncate", other),
    };
    let d = extract_date(&vals[1]).ok_or_else(|| {
        temporal_parse_error("datetime.truncate", "expected a temporal with date")
    })?;
    let t = extract_time(&vals[1]).unwrap_or(jiff::civil::Time::midnight());
    let base_tz = extract_tz(&vals[1]);

    let truncated_date = truncate_date_fields(d, &unit)?;
    let truncated_time = truncate_time_fields(t, &unit)?;

    let empty_map = indexmap::IndexMap::new();
    let overrides = match vals.get(2) {
        Some(Value::Map(m)) => m,
        None => &empty_map,
        _ => return type_error("datetime.truncate", &vals[2]),
    };

    let final_date = apply_date_overrides(truncated_date, overrides)?;
    let final_time = apply_time_overrides(truncated_time, overrides)?;

    // Determine timezone: map override > input timezone > UTC
    let tz = if let Some(tz_str) = map_get_str(overrides, "timezone") {
        parse_timezone(&tz_str).map_err(|e| temporal_parse_error("datetime.truncate", e))?
    } else if let Some(ref btz) = base_tz {
        btz.clone()
    } else {
        jiff::tz::TimeZone::UTC
    };

    let dt = jiff::civil::DateTime::new(
        final_date.year(),
        final_date.month(),
        final_date.day(),
        final_time.hour(),
        final_time.minute(),
        final_time.second(),
        final_time.subsec_nanosecond(),
    )
    .map_err(|e| temporal_parse_error("datetime.truncate", e))?;

    let zoned = dt
        .to_zoned(tz)
        .map_err(|e| temporal_parse_error("datetime.truncate", e))?;
    Ok(Value::DateTime(Box::new(zoned)))
}

pub(crate) fn eval_localdatetime_truncate(vals: &[Value]) -> Result<Value, GqlError> {
    if vals.len() < 2 || vals.len() > 3 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "localdatetime.truncate(unit, temporal[, map])".to_string(),
            actual: format!("{} arguments", vals.len()),
        }));
    }
    let unit = match &vals[0] {
        Value::Str(s) => s.to_ascii_lowercase(),
        other => return type_error("localdatetime.truncate", other),
    };
    let d = extract_date(&vals[1]).ok_or_else(|| {
        temporal_parse_error("localdatetime.truncate", "expected a temporal with date")
    })?;
    let t = extract_time(&vals[1]).unwrap_or(jiff::civil::Time::midnight());

    let truncated_date = truncate_date_fields(d, &unit)?;
    let truncated_time = truncate_time_fields(t, &unit)?;

    let empty_map = indexmap::IndexMap::new();
    let overrides = match vals.get(2) {
        Some(Value::Map(m)) => m,
        None => &empty_map,
        _ => return type_error("localdatetime.truncate", &vals[2]),
    };

    let final_date = apply_date_overrides(truncated_date, overrides)?;
    let final_time = apply_time_overrides(truncated_time, overrides)?;

    let dt = jiff::civil::DateTime::new(
        final_date.year(),
        final_date.month(),
        final_date.day(),
        final_time.hour(),
        final_time.minute(),
        final_time.second(),
        final_time.subsec_nanosecond(),
    )
    .map_err(|e| temporal_parse_error("localdatetime.truncate", e))?;
    Ok(Value::LocalDateTime(dt))
}

pub(crate) fn eval_localtime_truncate(vals: &[Value]) -> Result<Value, GqlError> {
    if vals.len() < 2 || vals.len() > 3 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "localtime.truncate(unit, temporal[, map])".to_string(),
            actual: format!("{} arguments", vals.len()),
        }));
    }
    let unit = match &vals[0] {
        Value::Str(s) => s.to_ascii_lowercase(),
        other => return type_error("localtime.truncate", other),
    };
    let t = extract_time(&vals[1]).ok_or_else(|| {
        temporal_parse_error("localtime.truncate", "expected a temporal with time")
    })?;
    let truncated = truncate_time_fields(t, &unit)?;
    let empty_map = indexmap::IndexMap::new();
    let overrides = match vals.get(2) {
        Some(Value::Map(m)) => m,
        None => &empty_map,
        _ => return type_error("localtime.truncate", &vals[2]),
    };
    let result = apply_time_overrides(truncated, overrides)?;
    Ok(Value::LocalTime(result))
}

pub(crate) fn eval_time_truncate(vals: &[Value]) -> Result<Value, GqlError> {
    if vals.len() < 2 || vals.len() > 3 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "time.truncate(unit, temporal[, map])".to_string(),
            actual: format!("{} arguments", vals.len()),
        }));
    }
    let unit = match &vals[0] {
        Value::Str(s) => s.to_ascii_lowercase(),
        other => return type_error("time.truncate", other),
    };
    let t = extract_time(&vals[1])
        .ok_or_else(|| temporal_parse_error("time.truncate", "expected a temporal with time"))?;
    let base_tz = extract_tz(&vals[1]);
    let truncated = truncate_time_fields(t, &unit)?;
    let empty_map = indexmap::IndexMap::new();
    let overrides = match vals.get(2) {
        Some(Value::Map(m)) => m,
        None => &empty_map,
        _ => return type_error("time.truncate", &vals[2]),
    };
    let result = apply_time_overrides(truncated, overrides)?;

    // time.truncate returns a zoned time (DateTime) — use timezone from override > input > UTC
    let tz = if let Some(tz_str) = map_get_str(overrides, "timezone") {
        parse_timezone(&tz_str).map_err(|e| temporal_parse_error("time.truncate", e))?
    } else if let Some(ref btz) = base_tz {
        btz.clone()
    } else {
        jiff::tz::TimeZone::UTC
    };

    // Build a zoned time with the truncated time and determined offset.
    let epoch_dt = jiff::civil::DateTime::new(
        1970,
        1,
        1,
        result.hour(),
        result.minute(),
        result.second(),
        result.subsec_nanosecond(),
    )
    .map_err(|e| temporal_parse_error("time.truncate", e))?;
    let zoned = epoch_dt
        .to_zoned(tz)
        .map_err(|e| temporal_parse_error("time.truncate", e))?;
    Ok(zoned_to_time_value(&zoned))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn date_val(y: i16, m: i8, d: i8) -> Value {
        Value::Date(jiff::civil::Date::new(y, m, d).unwrap())
    }
    fn ldt_val(y: i16, m: i8, d: i8, h: i8, mi: i8, s: i8) -> Value {
        Value::LocalDateTime(jiff::civil::DateTime::new(y, m, d, h, mi, s, 0).unwrap())
    }
    fn dt_val(y: i16, m: i8, d: i8, h: i8, mi: i8, s: i8) -> Value {
        Value::DateTime(Box::new(
            jiff::civil::DateTime::new(y, m, d, h, mi, s, 0)
                .unwrap()
                .to_zoned(jiff::tz::TimeZone::UTC)
                .unwrap(),
        ))
    }
    fn lt_val(h: i8, m: i8, s: i8) -> Value {
        Value::LocalTime(jiff::civil::Time::new(h, m, s, 0).unwrap())
    }
    fn time_val(h: i8, m: i8, s: i8, off: i32) -> Value {
        Value::Time(jiff::civil::Time::new(h, m, s, 0).unwrap(), off)
    }

    #[test]
    fn trunc_year() {
        assert_eq!(
            truncate_date_fields(jiff::civil::Date::new(2024, 6, 15).unwrap(), "year").unwrap(),
            jiff::civil::Date::new(2024, 1, 1).unwrap()
        );
    }
    #[test]
    fn trunc_quarter() {
        assert_eq!(
            truncate_date_fields(jiff::civil::Date::new(2024, 5, 15).unwrap(), "quarter").unwrap(),
            jiff::civil::Date::new(2024, 4, 1).unwrap()
        );
    }
    #[test]
    fn trunc_month() {
        assert_eq!(
            truncate_date_fields(jiff::civil::Date::new(2024, 6, 15).unwrap(), "month").unwrap(),
            jiff::civil::Date::new(2024, 6, 1).unwrap()
        );
    }
    #[test]
    fn trunc_week() {
        assert_eq!(
            truncate_date_fields(jiff::civil::Date::new(2024, 6, 15).unwrap(), "week").unwrap(),
            jiff::civil::Date::new(2024, 6, 10).unwrap()
        );
    }
    #[test]
    fn trunc_millennium() {
        assert_eq!(
            truncate_date_fields(jiff::civil::Date::new(2024, 6, 15).unwrap(), "millennium")
                .unwrap()
                .year(),
            2000
        );
    }
    #[test]
    fn trunc_unknown() {
        assert!(
            truncate_date_fields(jiff::civil::Date::new(2024, 6, 15).unwrap(), "unknown").is_err()
        );
    }
    #[test]
    fn trunc_time_day() {
        assert_eq!(
            truncate_time_fields(
                jiff::civil::Time::new(14, 30, 45, 123456789).unwrap(),
                "day"
            )
            .unwrap(),
            jiff::civil::Time::midnight()
        );
    }
    #[test]
    fn trunc_time_hour() {
        let r =
            truncate_time_fields(jiff::civil::Time::new(14, 30, 45, 0).unwrap(), "hour").unwrap();
        assert_eq!(r.hour(), 14);
        assert_eq!(r.minute(), 0);
    }
    #[test]
    fn trunc_time_ms() {
        assert_eq!(
            truncate_time_fields(
                jiff::civil::Time::new(14, 30, 45, 123456789).unwrap(),
                "millisecond"
            )
            .unwrap()
            .subsec_nanosecond(),
            123_000_000
        );
    }
    #[test]
    fn trunc_time_unknown() {
        assert!(
            truncate_time_fields(jiff::civil::Time::new(14, 30, 0, 0).unwrap(), "unknown").is_err()
        );
    }
    #[test]
    fn eval_date_year() {
        assert_eq!(
            eval_date_truncate(&[Value::Str("year".into()), date_val(2024, 6, 15)]).unwrap(),
            date_val(2024, 1, 1)
        );
    }
    #[test]
    fn eval_date_bad() {
        assert!(eval_date_truncate(&[Value::Str("year".into())]).is_err());
    }
    #[test]
    fn eval_lt_hour() {
        match eval_localtime_truncate(&[Value::Str("hour".into()), lt_val(14, 30, 45)]).unwrap() {
            Value::LocalTime(t) => {
                assert_eq!(t.hour(), 14);
                assert_eq!(t.minute(), 0);
            }
            _ => panic!("LT"),
        }
    }
    #[test]
    fn eval_ldt_year() {
        match eval_localdatetime_truncate(&[
            Value::Str("year".into()),
            ldt_val(2024, 6, 15, 14, 30, 45),
        ])
        .unwrap()
        {
            Value::LocalDateTime(dt) => {
                assert_eq!(dt.year(), 2024);
                assert_eq!(dt.month(), 1);
                assert_eq!(dt.hour(), 0);
            }
            _ => panic!("LDT"),
        }
    }
    #[test]
    fn eval_dt_year() {
        match eval_datetime_truncate(&[Value::Str("year".into()), dt_val(2024, 6, 15, 14, 30, 45)])
            .unwrap()
        {
            Value::DateTime(z) => {
                assert_eq!(z.year(), 2024);
                assert_eq!(z.month(), 1);
            }
            _ => panic!("DT"),
        }
    }
    #[test]
    fn eval_time_hour() {
        match eval_time_truncate(&[Value::Str("hour".into()), time_val(14, 30, 45, 3600)]).unwrap()
        {
            Value::Time(t, off) => {
                assert_eq!(t.hour(), 14);
                assert_eq!(t.minute(), 0);
                assert_eq!(off, 3600);
            }
            _ => panic!("T"),
        }
    }
    #[test]
    fn day_override() {
        let mut m = indexmap::IndexMap::new();
        m.insert("day".into(), Value::Int(15));
        assert_eq!(
            apply_date_overrides(jiff::civil::Date::new(2024, 6, 1).unwrap(), &m)
                .unwrap()
                .day(),
            15
        );
    }
    #[test]
    fn hour_override() {
        let mut m = indexmap::IndexMap::new();
        m.insert("hour".into(), Value::Int(10));
        assert_eq!(
            apply_time_overrides(jiff::civil::Time::new(0, 0, 0, 0).unwrap(), &m)
                .unwrap()
                .hour(),
            10
        );
    }
    #[test]
    fn extract_date_ok() {
        assert!(extract_date(&date_val(2024, 6, 15)).is_some());
    }
    #[test]
    fn extract_date_fail() {
        assert!(extract_date(&Value::Int(42)).is_none());
    }
    #[test]
    fn extract_tz_ok() {
        assert!(extract_tz(&time_val(10, 0, 0, 3600)).is_some());
    }
    #[test]
    fn extract_tz_fail() {
        assert!(extract_tz(&Value::Int(42)).is_none());
    }

    // ── eval_*_truncate public function tests ─────────────────────────────

    fn s(x: &str) -> Value {
        Value::Str(x.into())
    }

    #[test]
    fn date_truncate_year() {
        let r = eval_date_truncate(&[s("year"), date_val(2024, 6, 15)]).unwrap();
        assert_eq!(r, date_val(2024, 1, 1));
    }
    #[test]
    fn date_truncate_century() {
        let r = eval_date_truncate(&[s("century"), date_val(2024, 6, 15)]).unwrap();
        if let Value::Date(d) = r {
            assert_eq!(d.year(), 2000);
        } else {
            panic!()
        }
    }
    #[test]
    fn date_truncate_decade() {
        let r = eval_date_truncate(&[s("decade"), date_val(2024, 6, 15)]).unwrap();
        if let Value::Date(d) = r {
            assert_eq!(d.year(), 2020);
        } else {
            panic!()
        }
    }
    #[test]
    fn date_truncate_with_overrides() {
        let mut m = indexmap::IndexMap::new();
        m.insert("day".into(), Value::Int(15));
        let r = eval_date_truncate(&[s("month"), date_val(2024, 6, 1), Value::Map(m)]).unwrap();
        assert_eq!(r, date_val(2024, 6, 15));
    }
    #[test]
    fn date_truncate_arity_errs() {
        assert!(eval_date_truncate(&[]).is_err());
        assert!(eval_date_truncate(&[s("year")]).is_err());
        assert!(
            eval_date_truncate(&[s("year"), date_val(2024, 1, 1), Value::Null, Value::Null])
                .is_err()
        );
    }
    #[test]
    fn date_truncate_unit_not_string_errs() {
        assert!(eval_date_truncate(&[Value::Int(0), date_val(2024, 1, 1)]).is_err());
    }
    #[test]
    fn date_truncate_no_date_errs() {
        assert!(eval_date_truncate(&[s("year"), Value::Int(0)]).is_err());
    }
    #[test]
    fn date_truncate_overrides_not_map_errs() {
        assert!(eval_date_truncate(&[s("year"), date_val(2024, 1, 1), Value::Int(0)]).is_err());
    }

    #[test]
    fn datetime_truncate_year_from_localdatetime() {
        let r = eval_datetime_truncate(&[s("year"), ldt_val(2024, 6, 15, 12, 30, 45)]).unwrap();
        assert!(matches!(r, Value::DateTime(_)));
    }
    #[test]
    fn datetime_truncate_with_tz_override() {
        let mut m = indexmap::IndexMap::new();
        m.insert("timezone".into(), Value::Str("UTC".into()));
        let r = eval_datetime_truncate(&[s("day"), dt_val(2024, 6, 15, 12, 0, 0), Value::Map(m)])
            .unwrap();
        assert!(matches!(r, Value::DateTime(_)));
    }
    #[test]
    fn datetime_truncate_keeps_input_tz() {
        let r = eval_datetime_truncate(&[s("month"), dt_val(2024, 6, 15, 12, 0, 0)]).unwrap();
        assert!(matches!(r, Value::DateTime(_)));
    }
    #[test]
    fn datetime_truncate_arity_errs() {
        assert!(eval_datetime_truncate(&[]).is_err());
    }
    #[test]
    fn datetime_truncate_unit_errs() {
        assert!(eval_datetime_truncate(&[Value::Int(0), dt_val(2024, 6, 15, 0, 0, 0)]).is_err());
    }
    #[test]
    fn datetime_truncate_no_date_errs() {
        assert!(eval_datetime_truncate(&[s("year"), Value::Int(0)]).is_err());
    }
    #[test]
    fn datetime_truncate_invalid_overrides_errs() {
        assert!(
            eval_datetime_truncate(&[s("year"), dt_val(2024, 6, 15, 0, 0, 0), Value::Int(0)])
                .is_err()
        );
    }

    #[test]
    fn localdatetime_truncate_works() {
        let r = eval_localdatetime_truncate(&[s("year"), ldt_val(2024, 6, 15, 12, 30, 0)]).unwrap();
        assert!(matches!(r, Value::LocalDateTime(_)));
    }
    #[test]
    fn localdatetime_truncate_with_overrides() {
        let mut m = indexmap::IndexMap::new();
        m.insert("hour".into(), Value::Int(5));
        let r = eval_localdatetime_truncate(&[
            s("day"),
            ldt_val(2024, 6, 15, 12, 30, 0),
            Value::Map(m),
        ])
        .unwrap();
        if let Value::LocalDateTime(dt) = r {
            assert_eq!(dt.hour(), 5);
        } else {
            panic!()
        }
    }
    #[test]
    fn localdatetime_truncate_arity_errs() {
        assert!(eval_localdatetime_truncate(&[]).is_err());
    }
    #[test]
    fn localdatetime_truncate_no_date_errs() {
        assert!(eval_localdatetime_truncate(&[s("year"), Value::Int(0)]).is_err());
    }
    #[test]
    fn localdatetime_truncate_unit_errs() {
        assert!(
            eval_localdatetime_truncate(&[Value::Int(0), ldt_val(2024, 6, 15, 0, 0, 0)]).is_err()
        );
    }
    #[test]
    fn localdatetime_truncate_overrides_errs() {
        assert!(eval_localdatetime_truncate(&[
            s("year"),
            ldt_val(2024, 6, 15, 0, 0, 0),
            Value::Int(0)
        ])
        .is_err());
    }

    #[test]
    fn localtime_truncate_works() {
        let r = eval_localtime_truncate(&[s("hour"), lt_val(14, 30, 45)]).unwrap();
        if let Value::LocalTime(t) = r {
            assert_eq!(t.hour(), 14);
            assert_eq!(t.minute(), 0);
        } else {
            panic!()
        }
    }
    #[test]
    fn localtime_truncate_arity_errs() {
        assert!(eval_localtime_truncate(&[]).is_err());
    }
    #[test]
    fn localtime_truncate_unit_errs() {
        assert!(eval_localtime_truncate(&[Value::Int(0), lt_val(1, 0, 0)]).is_err());
    }
    #[test]
    fn localtime_truncate_no_time_errs() {
        assert!(eval_localtime_truncate(&[s("hour"), Value::Int(0)]).is_err());
    }
    #[test]
    fn localtime_truncate_overrides_errs() {
        assert!(eval_localtime_truncate(&[s("hour"), lt_val(1, 0, 0), Value::Int(0)]).is_err());
    }

    #[test]
    fn time_truncate_works() {
        let r = eval_time_truncate(&[s("minute"), time_val(14, 30, 45, 3600)]).unwrap();
        if let Value::Time(t, off) = r {
            assert_eq!(t.minute(), 30);
            assert_eq!(t.second(), 0);
            assert_eq!(off, 3600);
        } else {
            panic!()
        }
    }
    #[test]
    fn time_truncate_with_tz_override() {
        let mut m = indexmap::IndexMap::new();
        m.insert("timezone".into(), Value::Str("UTC".into()));
        let r = eval_time_truncate(&[s("hour"), lt_val(14, 30, 45), Value::Map(m)]).unwrap();
        assert!(matches!(r, Value::Time(_, _)));
    }
    #[test]
    fn time_truncate_arity_errs() {
        assert!(eval_time_truncate(&[]).is_err());
    }
    #[test]
    fn time_truncate_unit_errs() {
        assert!(eval_time_truncate(&[Value::Int(0), lt_val(1, 0, 0)]).is_err());
    }
    #[test]
    fn time_truncate_no_time_errs() {
        assert!(eval_time_truncate(&[s("hour"), Value::Int(0)]).is_err());
    }
    #[test]
    fn time_truncate_overrides_errs() {
        assert!(eval_time_truncate(&[s("hour"), lt_val(1, 0, 0), Value::Int(0)]).is_err());
    }

    #[test]
    fn trunc_weekyear_works() {
        let r =
            truncate_date_fields(jiff::civil::Date::new(2024, 6, 15).unwrap(), "weekyear").unwrap();
        assert_eq!(r.year(), 2024);
    }
    #[test]
    fn trunc_decade_century() {
        assert_eq!(
            truncate_date_fields(jiff::civil::Date::new(2024, 6, 15).unwrap(), "decade")
                .unwrap()
                .year(),
            2020
        );
        assert_eq!(
            truncate_date_fields(jiff::civil::Date::new(2024, 6, 15).unwrap(), "century")
                .unwrap()
                .year(),
            2000
        );
    }
    #[test]
    fn trunc_time_minute_second_milli_micro() {
        let t = jiff::civil::Time::new(14, 30, 45, 123_456_789).unwrap();
        assert_eq!(truncate_time_fields(t, "minute").unwrap().second(), 0);
        assert_eq!(
            truncate_time_fields(t, "second")
                .unwrap()
                .subsec_nanosecond(),
            0
        );
        assert_eq!(
            truncate_time_fields(t, "millisecond")
                .unwrap()
                .subsec_nanosecond(),
            123_000_000
        );
        assert_eq!(
            truncate_time_fields(t, "microsecond")
                .unwrap()
                .subsec_nanosecond(),
            123_456_000
        );
    }
    #[test]
    fn trunc_time_unknown_errs() {
        let t = jiff::civil::Time::new(0, 0, 0, 0).unwrap();
        assert!(truncate_time_fields(t, "unknown").is_err());
    }
    #[test]
    fn apply_overrides_dayofweek() {
        let mut m = indexmap::IndexMap::new();
        m.insert("dayOfWeek".into(), Value::Int(1));
        // 2024-06-15 is a Saturday → Monday is 2024-06-10
        let r = apply_date_overrides(jiff::civil::Date::new(2024, 6, 15).unwrap(), &m).unwrap();
        assert_eq!(r, jiff::civil::Date::new(2024, 6, 10).unwrap());
    }
    #[test]
    fn apply_overrides_minute_second_ms_us_ns() {
        let mut m = indexmap::IndexMap::new();
        m.insert("minute".into(), Value::Int(45));
        m.insert("second".into(), Value::Int(30));
        let r = apply_time_overrides(jiff::civil::Time::new(10, 0, 0, 0).unwrap(), &m).unwrap();
        assert_eq!(r.minute(), 45);
        assert_eq!(r.second(), 30);

        let mut m2 = indexmap::IndexMap::new();
        m2.insert("microsecond".into(), Value::Int(5));
        let r2 = apply_time_overrides(jiff::civil::Time::new(0, 0, 0, 0).unwrap(), &m2).unwrap();
        assert_eq!(r2.subsec_nanosecond(), 5_000);

        let mut m3 = indexmap::IndexMap::new();
        m3.insert("nanosecond".into(), Value::Int(7));
        let r3 = apply_time_overrides(jiff::civil::Time::new(0, 0, 0, 0).unwrap(), &m3).unwrap();
        assert_eq!(r3.subsec_nanosecond(), 7);
    }
    #[test]
    fn extract_time_variants() {
        assert!(extract_time(&lt_val(1, 0, 0)).is_some());
        assert!(extract_time(&time_val(1, 0, 0, 0)).is_some());
        assert!(extract_time(&ldt_val(2024, 1, 1, 1, 0, 0)).is_some());
        assert!(extract_time(&dt_val(2024, 1, 1, 1, 0, 0)).is_some());
        assert!(extract_time(&Value::Int(0)).is_none());
    }
    #[test]
    fn extract_date_from_dt_ldt() {
        assert!(extract_date(&ldt_val(2024, 6, 15, 0, 0, 0)).is_some());
        assert!(extract_date(&dt_val(2024, 6, 15, 0, 0, 0)).is_some());
    }
}
