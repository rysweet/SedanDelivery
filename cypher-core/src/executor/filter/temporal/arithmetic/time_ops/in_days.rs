//! `duration.inDays(t1, t2)` — diff truncated to whole days.

use crate::error::{ExecutionError, GqlError};
use crate::types::{CypherDuration, Value};

use super::super::super::temporal_parse_error;
use super::super::{extract_date, has_date_component, is_time_only};

pub(crate) fn eval_duration_in_days(vals: &[Value]) -> Result<Value, GqlError> {
    if vals.len() != 2 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "duration.inDays(temporal1, temporal2)".to_string(),
            actual: format!("{} arguments", vals.len()),
        }));
    }
    if matches!(&vals[0], Value::Null) || matches!(&vals[1], Value::Null) {
        return Ok(Value::Null);
    }
    if is_time_only(&vals[0])
        || is_time_only(&vals[1])
        || !has_date_component(&vals[0])
        || !has_date_component(&vals[1])
    {
        return Ok(Value::Duration(CypherDuration {
            months: 0,
            days: 0,
            nanos: 0,
        }));
    }
    if matches!(&vals[0], Value::LocalDateTime(_)) && matches!(&vals[1], Value::DateTime(_)) {
        return Ok(Value::Duration(CypherDuration {
            months: 0,
            days: 0,
            nanos: 0,
        }));
    }

    let ts1_opt: Option<i128> = match &vals[0] {
        Value::DateTime(z) => Some(z.timestamp().as_nanosecond()),
        Value::Date(d) => d
            .to_datetime(jiff::civil::Time::midnight())
            .to_zoned(jiff::tz::TimeZone::UTC)
            .ok()
            .map(|z| z.timestamp().as_nanosecond()),
        Value::LocalDateTime(dt) => dt
            .to_zoned(jiff::tz::TimeZone::UTC)
            .ok()
            .map(|z| z.timestamp().as_nanosecond()),
        _ => None,
    };
    let ts2_opt: Option<i128> = match &vals[1] {
        Value::DateTime(z) => Some(z.timestamp().as_nanosecond()),
        Value::Date(d) => d
            .to_datetime(jiff::civil::Time::midnight())
            .to_zoned(jiff::tz::TimeZone::UTC)
            .ok()
            .map(|z| z.timestamp().as_nanosecond()),
        Value::LocalDateTime(dt) => dt
            .to_zoned(jiff::tz::TimeZone::UTC)
            .ok()
            .map(|z| z.timestamp().as_nanosecond()),
        _ => None,
    };
    let either_zoned =
        matches!(&vals[0], Value::DateTime(_)) || matches!(&vals[1], Value::DateTime(_));
    if either_zoned {
        if let (Some(ts1_ns), Some(ts2_ns)) = (ts1_opt, ts2_opt) {
            let diff_ns = ts2_ns - ts1_ns;
            let day_ns: i128 = 86_400_000_000_000;
            let diff_days = diff_ns.div_euclid(day_ns) as i64;
            return Ok(Value::Duration(CypherDuration {
                months: 0,
                days: diff_days,
                nanos: 0,
            }));
        }
    }
    let d1 = extract_date(&vals[0])
        .ok_or_else(|| temporal_parse_error("duration.inDays", "expected temporal with date"))?;
    let d2 = extract_date(&vals[1])
        .ok_or_else(|| temporal_parse_error("duration.inDays", "expected temporal with date"))?;
    let diff_span = d1
        .until(jiff::civil::DateDifference::new(d2).largest(jiff::Unit::Day))
        .map_err(|e| temporal_parse_error("duration.inDays", e))?;
    let diff_days = diff_span.get_days() as i64;
    Ok(Value::Duration(CypherDuration {
        months: 0,
        days: diff_days,
        nanos: 0,
    }))
}
