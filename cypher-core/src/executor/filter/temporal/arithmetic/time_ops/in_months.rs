//! `duration.inMonths(t1, t2)` — diff truncated to whole months.

use crate::error::{ExecutionError, GqlError};
use crate::types::{CypherDuration, Value};

use super::super::super::temporal_parse_error;
use super::super::{has_date_component, is_time_only, temporal_to_civil_datetime};

pub(crate) fn eval_duration_in_months(vals: &[Value]) -> Result<Value, GqlError> {
    if vals.len() != 2 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "duration.inMonths(temporal1, temporal2)".to_string(),
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
    let dt1 = temporal_to_civil_datetime(&vals[0])
        .ok_or_else(|| temporal_parse_error("duration.inMonths", "expected temporal"))?;
    let dt2 = temporal_to_civil_datetime(&vals[1])
        .ok_or_else(|| temporal_parse_error("duration.inMonths", "expected temporal"))?;

    let d1 = dt1.date();
    let d2 = dt2.date();
    let months_span = d1
        .until(jiff::civil::DateDifference::new(d2).largest(jiff::Unit::Year))
        .map_err(|e| temporal_parse_error("duration.inMonths", e))?;
    let mut total_months = months_span.get_years() as i64 * 12 + months_span.get_months() as i64;

    let lhs_is_zoned = matches!(&vals[0], Value::DateTime(_));
    let rhs_is_zoned = matches!(&vals[1], Value::DateTime(_));
    if lhs_is_zoned || (rhs_is_zoned && !matches!(&vals[0], Value::LocalDateTime(_))) {
        let lhs_zoned: Option<jiff::Zoned> = match &vals[0] {
            Value::DateTime(z) => Some(z.as_ref().clone()),
            Value::Date(d) => d
                .to_datetime(jiff::civil::Time::midnight())
                .to_zoned(jiff::tz::TimeZone::UTC)
                .ok(),
            _ => None,
        };
        let rhs_ts_ns: Option<i128> = match &vals[1] {
            Value::DateTime(z) => Some(z.timestamp().as_nanosecond()),
            Value::Date(d) => d
                .to_datetime(jiff::civil::Time::midnight())
                .to_zoned(jiff::tz::TimeZone::UTC)
                .ok()
                .map(|z| z.timestamp().as_nanosecond()),
            _ => None,
        };
        if let (Some(lz), Some(ts2_ns)) = (lhs_zoned, rhs_ts_ns) {
            if let Ok(advanced) = lz.checked_add(jiff::Span::new().months(total_months)) {
                let adv_ns = advanced.timestamp().as_nanosecond();
                if total_months >= 0 && adv_ns > ts2_ns {
                    total_months -= 1;
                } else if total_months < 0 && adv_ns < ts2_ns {
                    total_months += 1;
                }
            }
        }
    }

    Ok(Value::Duration(CypherDuration {
        months: total_months,
        days: 0,
        nanos: 0,
    }))
}
