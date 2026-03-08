//! Temporal constructor functions: date(), time(), localtime(), datetime(),
//! localdatetime(), duration(), and datetime.fromEpoch/fromEpochMillis.

mod date;
mod datetime;
mod duration;
mod time;

use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

use super::{map_get_int, temporal_parse_error};

// Re-export all public constructors
pub(crate) use self::date::{builtin_temporal_realtime_date, eval_date_constructor};
pub(crate) use self::datetime::{
    builtin_datetime_fromepoch, builtin_datetime_fromepochmillis,
    builtin_temporal_realtime_datetime, builtin_temporal_realtime_localdatetime,
    eval_datetime_constructor, eval_localdatetime_constructor,
};
pub(crate) use self::duration::eval_duration_constructor;
pub(crate) use self::time::{
    builtin_temporal_realtime_localtime, builtin_temporal_realtime_time, eval_localtime_constructor,
    eval_time_constructor,
};

// Shared helpers used by multiple constructor submodules

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
