//! `duration.between(t1, t2)` — full-precision diff producing months + days + nanos.

use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

use super::super::super::parsing::{extract_large_date, large_date_duration_between};
use super::super::super::properties::time_to_utc_nanos;
use super::super::super::temporal_parse_error;
use super::super::{
    build_duration_span, civil_time_nanos, has_date_component, is_time_only,
    temporal_to_civil_datetime,
};

pub(crate) fn eval_duration_between(vals: &[Value]) -> Result<Value, GqlError> {
    if vals.len() != 2 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "duration.between(temporal1, temporal2)".to_string(),
            actual: format!("{} arguments", vals.len()),
        }));
    }
    if matches!(&vals[0], Value::Null) || matches!(&vals[1], Value::Null) {
        return Ok(Value::Null);
    }
    if let (Some((y1, m1, d1)), Some((y2, m2, d2))) =
        (extract_large_date(&vals[0]), extract_large_date(&vals[1]))
    {
        return Ok(Value::Duration(large_date_duration_between(
            y1, m1, d1, y2, m2, d2,
        )));
    }

    let both_have_dates = has_date_component(&vals[0]) && has_date_component(&vals[1]);
    let either_time_only = is_time_only(&vals[0]) || is_time_only(&vals[1]);

    if let (Value::DateTime(z1), Value::DateTime(z2)) = (&vals[0], &vals[1]) {
        if both_have_dates {
            let d1 = jiff::civil::Date::from(&**z1);
            let d2 = jiff::civil::Date::from(&**z2);
            let months_span = d1
                .until(jiff::civil::DateDifference::new(d2).largest(jiff::Unit::Year))
                .map_err(|e| temporal_parse_error("duration.between", e))?;
            let total_months =
                months_span.get_years() as i64 * 12 + months_span.get_months() as i64;

            let ts1 = z1.timestamp();
            let ts2 = z2.timestamp();
            let total_nanos = ts2.as_nanosecond() - ts1.as_nanosecond();

            let months_nanos = if total_months != 0 {
                let z1_plus_months = z1
                    .as_ref()
                    .clone()
                    .checked_add(jiff::Span::new().months(total_months))
                    .map_err(|e| temporal_parse_error("duration.between", e))?;
                z1_plus_months.timestamp().as_nanosecond() - ts1.as_nanosecond()
            } else {
                0
            };
            let time_nanos = total_nanos - months_nanos;
            let time_secs = (time_nanos / 1_000_000_000) as i64;
            let time_subsec_nanos = (time_nanos % 1_000_000_000) as i32;

            let sp = build_duration_span(total_months, 0, time_secs, time_subsec_nanos);
            return Ok(Value::Duration(sp));
        } else {
            let utc_tod_nanos = |zdt: &jiff::Zoned| -> i64 {
                let utc = zdt.with_time_zone(jiff::tz::TimeZone::UTC);
                let t = jiff::civil::Time::from(&utc);
                t.hour() as i64 * 3_600_000_000_000
                    + t.minute() as i64 * 60_000_000_000
                    + t.second() as i64 * 1_000_000_000
                    + t.subsec_nanosecond() as i64
            };
            let nanos_diff = utc_tod_nanos(z2) - utc_tod_nanos(z1);
            let secs = nanos_diff / 1_000_000_000;
            let subsec = (nanos_diff % 1_000_000_000) as i32;
            let sp = build_duration_span(0, 0, secs, subsec);
            return Ok(Value::Duration(sp));
        }
    }

    if let (Value::Time(t1, o1), Value::Time(t2, o2)) = (&vals[0], &vals[1]) {
        let utc1 = time_to_utc_nanos(t1, *o1);
        let utc2 = time_to_utc_nanos(t2, *o2);
        let diff = utc2 - utc1;
        let secs = diff / 1_000_000_000;
        let subsec = (diff % 1_000_000_000) as i32;
        let sp = build_duration_span(0, 0, secs, subsec);
        return Ok(Value::Duration(sp));
    }

    match (&vals[0], &vals[1]) {
        (Value::Time(t, off), Value::DateTime(zdt))
        | (Value::DateTime(zdt), Value::Time(t, off))
            if !has_date_component(&vals[0]) || !has_date_component(&vals[1]) =>
        {
            let utc_time_nanos = time_to_utc_nanos(t, *off);
            let utc_zdt = zdt.with_time_zone(jiff::tz::TimeZone::UTC);
            let zt = jiff::civil::Time::from(&utc_zdt);
            let utc_zdt_nanos = zt.hour() as i64 * 3_600_000_000_000
                + zt.minute() as i64 * 60_000_000_000
                + zt.second() as i64 * 1_000_000_000
                + zt.subsec_nanosecond() as i64;
            let (n1, n2) = if matches!(&vals[0], Value::Time(..)) {
                (utc_time_nanos, utc_zdt_nanos)
            } else {
                (utc_zdt_nanos, utc_time_nanos)
            };
            let diff = n2 - n1;
            let secs = diff / 1_000_000_000;
            let subsec = (diff % 1_000_000_000) as i32;
            let sp = build_duration_span(0, 0, secs, subsec);
            return Ok(Value::Duration(sp));
        }
        _ => {}
    }

    let dt1 = temporal_to_civil_datetime(&vals[0])
        .ok_or_else(|| temporal_parse_error("duration.between", "expected temporal argument"))?;
    let dt2 = temporal_to_civil_datetime(&vals[1])
        .ok_or_else(|| temporal_parse_error("duration.between", "expected temporal argument"))?;

    let both_local_datetime =
        matches!(&vals[0], Value::LocalDateTime(_)) && matches!(&vals[1], Value::LocalDateTime(_));

    if both_have_dates && !either_time_only {
        let d1 = dt1.date();
        let d2 = dt2.date();

        let months_span = d1
            .until(jiff::civil::DateDifference::new(d2).largest(jiff::Unit::Year))
            .map_err(|e| temporal_parse_error("duration.between", e))?;
        let total_months = months_span.get_years() as i64 * 12 + months_span.get_months() as i64;

        let d1_advanced = d1
            .checked_add(jiff::Span::new().months(total_months))
            .map_err(|e| temporal_parse_error("duration.between", e))?;

        if both_local_datetime {
            let dt1_advanced_dt = d1_advanced.to_datetime(dt1.time());
            let dt1_adv_ts = dt1_advanced_dt
                .to_zoned(jiff::tz::TimeZone::UTC)
                .map_err(|e| temporal_parse_error("duration.between", e))?
                .timestamp()
                .as_nanosecond();
            let dt2_ts = dt2
                .to_zoned(jiff::tz::TimeZone::UTC)
                .map_err(|e| temporal_parse_error("duration.between", e))?
                .timestamp()
                .as_nanosecond();
            let diff_nanos = dt2_ts - dt1_adv_ts;
            let secs = (diff_nanos / 1_000_000_000) as i64;
            let subsec = (diff_nanos % 1_000_000_000) as i32;
            let sp = build_duration_span(total_months, 0, secs, subsec);
            return Ok(Value::Duration(sp));
        }

        let remaining_days_span = d1_advanced
            .until(d2)
            .map_err(|e| temporal_parse_error("duration.between", e))?;
        let mut remaining_days = remaining_days_span.get_days() as i64;

        let t1 = dt1.time();
        let t2 = dt2.time();
        let t1_nanos = t1.hour() as i64 * 3_600_000_000_000
            + t1.minute() as i64 * 60_000_000_000
            + t1.second() as i64 * 1_000_000_000
            + t1.subsec_nanosecond() as i64;
        let t2_nanos = t2.hour() as i64 * 3_600_000_000_000
            + t2.minute() as i64 * 60_000_000_000
            + t2.second() as i64 * 1_000_000_000
            + t2.subsec_nanosecond() as i64;
        let mut time_diff_nanos = t2_nanos - t1_nanos;

        if remaining_days > 0 && time_diff_nanos < 0 {
            remaining_days -= 1;
            time_diff_nanos += 86_400_000_000_000;
        } else if remaining_days < 0 && time_diff_nanos > 0 {
            remaining_days += 1;
            time_diff_nanos -= 86_400_000_000_000;
        }

        let total_secs = time_diff_nanos / 1_000_000_000;
        let subsec_nanos = (time_diff_nanos % 1_000_000_000) as i32;

        let sp = build_duration_span(total_months, remaining_days, total_secs, subsec_nanos);
        Ok(Value::Duration(sp))
    } else {
        let t1_nanos = civil_time_nanos(&vals[0]);
        let t2_nanos = civil_time_nanos(&vals[1]);
        let diff = t2_nanos - t1_nanos;
        let secs = diff / 1_000_000_000;
        let subsec = (diff % 1_000_000_000) as i32;
        let sp = build_duration_span(0, 0, secs, subsec);
        Ok(Value::Duration(sp))
    }
}
