//! Public `eval_*_truncate` entry points dispatched from temporal builtins.

use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

use super::super::super::builtins::type_error;
use super::super::{map_get_str, parse_timezone, temporal_parse_error};
use super::extract::{extract_date, extract_time, extract_tz};
use super::overrides::{apply_date_overrides, apply_time_overrides, zoned_to_time_value};
use super::units::{truncate_date_fields, truncate_time_fields};

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
