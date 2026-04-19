//! duration.between, duration.inMonths, duration.inDays, duration.inSeconds operations.

use crate::error::{ExecutionError, GqlError};
use crate::types::{CypherDuration, Value};

use super::super::parsing::{
    extract_large_date, extract_large_ldt, large_date_duration_between, large_ldt_in_seconds,
};
use super::super::properties::time_to_utc_nanos;
use super::super::temporal_parse_error;
use super::{
    build_duration_span, civil_time_nanos, extract_date, has_date_component, is_time_only,
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
    // Handle large-year date sentinels
    if let (Some((y1, m1, d1)), Some((y2, m2, d2))) =
        (extract_large_date(&vals[0]), extract_large_date(&vals[1]))
    {
        return Ok(Value::Duration(large_date_duration_between(
            y1, m1, d1, y2, m2, d2,
        )));
    }

    let both_have_dates = has_date_component(&vals[0]) && has_date_component(&vals[1]);
    let either_time_only = is_time_only(&vals[0]) || is_time_only(&vals[1]);

    // For zoned datetime pairs, use timestamp-based calculation
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

    // Two zoned times (Value::Time): diff by UTC-normalized instants
    if let (Value::Time(t1, o1), Value::Time(t2, o2)) = (&vals[0], &vals[1]) {
        let utc1 = time_to_utc_nanos(t1, *o1);
        let utc2 = time_to_utc_nanos(t2, *o2);
        let diff = utc2 - utc1;
        let secs = diff / 1_000_000_000;
        let subsec = (diff % 1_000_000_000) as i32;
        let sp = build_duration_span(0, 0, secs, subsec);
        return Ok(Value::Duration(sp));
    }

    // Mixed Time + DateTime: both have offsets, use UTC-normalized time-of-day
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

pub(crate) fn eval_duration_in_seconds(vals: &[Value]) -> Result<Value, GqlError> {
    if vals.len() != 2 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "duration.inSeconds(temporal1, temporal2)".to_string(),
            actual: format!("{} arguments", vals.len()),
        }));
    }
    if matches!(&vals[0], Value::Null) || matches!(&vals[1], Value::Null) {
        return Ok(Value::Null);
    }
    // Handle large-year datetime sentinels
    if let (Some((y1, mo1, d1, h1, min1, s1, ns1)), Some((y2, mo2, d2, h2, min2, s2, ns2))) =
        (extract_large_ldt(&vals[0]), extract_large_ldt(&vals[1]))
    {
        return Ok(Value::Duration(large_ldt_in_seconds(
            (y1, mo1, d1, h1, min1, s1, ns1),
            (y2, mo2, d2, h2, min2, s2, ns2),
        )));
    }

    if let (Value::DateTime(z1), Value::DateTime(z2)) = (&vals[0], &vals[1]) {
        let v1_has_date = has_date_component(&vals[0]);
        let v2_has_date = has_date_component(&vals[1]);
        if v1_has_date && v2_has_date {
            let ts1 = z1.timestamp();
            let ts2 = z2.timestamp();
            let total_nanos = ts2.as_nanosecond() - ts1.as_nanosecond();
            return Ok(Value::Duration(CypherDuration {
                months: 0,
                days: 0,
                nanos: total_nanos,
            }));
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
            return Ok(Value::Duration(CypherDuration {
                months: 0,
                days: 0,
                nanos: nanos_diff as i128,
            }));
        }
    }

    // Two zoned times (Value::Time): diff by UTC-normalized instants
    if let (Value::Time(t1, o1), Value::Time(t2, o2)) = (&vals[0], &vals[1]) {
        let utc1 = time_to_utc_nanos(t1, *o1);
        let utc2 = time_to_utc_nanos(t2, *o2);
        let diff = (utc2 - utc1) as i128;
        return Ok(Value::Duration(CypherDuration {
            months: 0,
            days: 0,
            nanos: diff,
        }));
    }

    // Mixed Time + DateTime: both have offsets, use UTC-normalized time-of-day
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
            let diff = (n2 - n1) as i128;
            return Ok(Value::Duration(CypherDuration {
                months: 0,
                days: 0,
                nanos: diff,
            }));
        }
        _ => {}
    }

    // Mixed DateTime + LocalDateTime/LocalTime: use DST-aware calculation if possible.
    let zoned_val = match (&vals[0], &vals[1]) {
        (Value::DateTime(z), _) if has_date_component(&vals[0]) => Some((z.as_ref(), true)),
        (_, Value::DateTime(z)) if has_date_component(&vals[1]) => Some((z.as_ref(), false)),
        _ => None,
    };
    if let Some((zdt, zdt_is_first)) = zoned_val {
        let tz = zdt.time_zone().clone();
        let other_val = if zdt_is_first { &vals[1] } else { &vals[0] };
        let other_civil: Option<jiff::civil::DateTime> = match other_val {
            Value::LocalTime(t) => {
                let zdt_date = jiff::civil::Date::from(zdt);
                Some(zdt_date.to_datetime(*t))
            }
            other => temporal_to_civil_datetime(other),
        };
        if let Some(other_civil_dt) = other_civil {
            if let Ok(other_zoned) = other_civil_dt.to_zoned(tz) {
                let (ts1, ts2) = if zdt_is_first {
                    (zdt.timestamp(), other_zoned.timestamp())
                } else {
                    (other_zoned.timestamp(), zdt.timestamp())
                };
                let total_nanos = ts2.as_nanosecond() - ts1.as_nanosecond();
                return Ok(Value::Duration(CypherDuration {
                    months: 0,
                    days: 0,
                    nanos: total_nanos,
                }));
            }
        }
    }

    let dt1 = temporal_to_civil_datetime(&vals[0])
        .ok_or_else(|| temporal_parse_error("duration.inSeconds", "expected temporal"))?;
    let dt2 = temporal_to_civil_datetime(&vals[1])
        .ok_or_else(|| temporal_parse_error("duration.inSeconds", "expected temporal"))?;

    let both_have_dates = has_date_component(&vals[0]) && has_date_component(&vals[1]);

    if both_have_dates {
        let d1 = dt1.date();
        let d2 = dt2.date();
        let days_span = d1
            .until(jiff::civil::DateDifference::new(d2).largest(jiff::Unit::Day))
            .map_err(|e| temporal_parse_error("duration.inSeconds", e))?;
        let diff_days = days_span.get_days() as i128;

        let t1 = dt1.time();
        let t2 = dt2.time();
        let t1_nanos = t1.hour() as i128 * 3_600_000_000_000
            + t1.minute() as i128 * 60_000_000_000
            + t1.second() as i128 * 1_000_000_000
            + t1.subsec_nanosecond() as i128;
        let t2_nanos = t2.hour() as i128 * 3_600_000_000_000
            + t2.minute() as i128 * 60_000_000_000
            + t2.second() as i128 * 1_000_000_000
            + t2.subsec_nanosecond() as i128;

        let total_nanos = diff_days * 86_400_000_000_000_i128 + (t2_nanos - t1_nanos);
        Ok(Value::Duration(CypherDuration {
            months: 0,
            days: 0,
            nanos: total_nanos,
        }))
    } else {
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
        let diff = t2_nanos - t1_nanos;
        Ok(Value::Duration(CypherDuration {
            months: 0,
            days: 0,
            nanos: diff as i128,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn between_dates() {
        let d1 = Value::Date(jiff::civil::Date::new(2024, 1, 1).unwrap());
        let d2 = Value::Date(jiff::civil::Date::new(2024, 3, 15).unwrap());
        match eval_duration_between(&[d1, d2]).unwrap() {
            Value::Duration(d) => {
                assert_eq!(d.months, 2);
                assert!(d.days > 0);
            }
            _ => panic!("Dur"),
        }
    }
    #[test]
    fn between_null() {
        assert_eq!(
            eval_duration_between(&[
                Value::Null,
                Value::Date(jiff::civil::Date::new(2024, 1, 1).unwrap())
            ])
            .unwrap(),
            Value::Null
        );
    }
    #[test]
    fn between_bad() {
        assert!(eval_duration_between(&[Value::Int(1)]).is_err());
    }
    #[test]
    fn in_months_dates() {
        let d1 = Value::Date(jiff::civil::Date::new(2024, 1, 1).unwrap());
        let d2 = Value::Date(jiff::civil::Date::new(2024, 7, 1).unwrap());
        match eval_duration_in_months(&[d1, d2]).unwrap() {
            Value::Duration(d) => assert_eq!(d.months, 6),
            _ => panic!("Dur"),
        }
    }
    #[test]
    fn in_months_null() {
        assert_eq!(
            eval_duration_in_months(&[
                Value::Null,
                Value::Date(jiff::civil::Date::new(2024, 1, 1).unwrap())
            ])
            .unwrap(),
            Value::Null
        );
    }
    #[test]
    fn in_days_dates() {
        let d1 = Value::Date(jiff::civil::Date::new(2024, 1, 1).unwrap());
        let d2 = Value::Date(jiff::civil::Date::new(2024, 1, 11).unwrap());
        match eval_duration_in_days(&[d1, d2]).unwrap() {
            Value::Duration(d) => assert_eq!(d.days, 10),
            _ => panic!("Dur"),
        }
    }
    #[test]
    fn in_days_null() {
        assert_eq!(
            eval_duration_in_days(&[
                Value::Null,
                Value::Date(jiff::civil::Date::new(2024, 1, 1).unwrap())
            ])
            .unwrap(),
            Value::Null
        );
    }
    #[test]
    fn in_seconds_ldts() {
        let d1 = Value::LocalDateTime(jiff::civil::DateTime::new(2024, 1, 1, 10, 0, 0, 0).unwrap());
        let d2 = Value::LocalDateTime(jiff::civil::DateTime::new(2024, 1, 1, 12, 0, 0, 0).unwrap());
        match eval_duration_in_seconds(&[d1, d2]).unwrap() {
            Value::Duration(d) => assert_eq!(d.nanos, 2 * 3_600_000_000_000i128),
            _ => panic!("Dur"),
        }
    }
    #[test]
    fn in_seconds_null() {
        assert_eq!(
            eval_duration_in_seconds(&[
                Value::Null,
                Value::LocalDateTime(jiff::civil::DateTime::new(2024, 1, 1, 0, 0, 0, 0).unwrap())
            ])
            .unwrap(),
            Value::Null
        );
    }
    #[test]
    fn is_time_only_lt() {
        assert!(is_time_only(&Value::LocalTime(
            jiff::civil::Time::new(10, 0, 0, 0).unwrap()
        )));
    }
    #[test]
    fn is_time_only_date() {
        assert!(!is_time_only(&Value::Date(
            jiff::civil::Date::new(2024, 1, 1).unwrap()
        )));
    }
    #[test]
    fn has_date_date() {
        assert!(has_date_component(&Value::Date(
            jiff::civil::Date::new(2024, 1, 1).unwrap()
        )));
    }
    #[test]
    fn between_datetimes() {
        let z1: jiff::Zoned = "2024-01-01T00:00:00+00:00[UTC]".parse().unwrap();
        let z2: jiff::Zoned = "2024-06-01T12:00:00+00:00[UTC]".parse().unwrap();
        match eval_duration_between(&[Value::DateTime(Box::new(z1)), Value::DateTime(Box::new(z2))])
            .unwrap()
        {
            Value::Duration(d) => assert_eq!(d.months, 5),
            _ => panic!(),
        }
    }
    #[test]
    fn between_localdatetimes() {
        let dt1 = jiff::civil::DateTime::new(2024, 1, 1, 0, 0, 0, 0).unwrap();
        let dt2 = jiff::civil::DateTime::new(2024, 1, 2, 0, 0, 0, 0).unwrap();
        match eval_duration_between(&[Value::LocalDateTime(dt1), Value::LocalDateTime(dt2)])
            .unwrap()
        {
            Value::Duration(d) => assert!(d.days >= 0),
            _ => panic!(),
        }
    }
    #[test]
    fn between_local_times() {
        let t1 = Value::LocalTime(jiff::civil::Time::new(10, 0, 0, 0).unwrap());
        let t2 = Value::LocalTime(jiff::civil::Time::new(12, 0, 0, 0).unwrap());
        match eval_duration_between(&[t1, t2]).unwrap() {
            Value::Duration(d) => assert!(d.nanos > 0),
            _ => panic!(),
        }
    }
    #[test]
    fn between_zoned_times() {
        let t1 = Value::Time(jiff::civil::Time::new(10, 0, 0, 0).unwrap(), 0);
        let t2 = Value::Time(jiff::civil::Time::new(12, 0, 0, 0).unwrap(), 0);
        match eval_duration_between(&[t1, t2]).unwrap() {
            Value::Duration(d) => assert!(d.nanos > 0),
            _ => panic!(),
        }
    }
    #[test]
    fn between_too_few_args_errs() {
        assert!(eval_duration_between(&[]).is_err());
    }
    #[test]
    fn in_months_too_few_args_errs() {
        assert!(eval_duration_in_months(&[]).is_err());
    }
    #[test]
    fn in_days_too_few_args_errs() {
        assert!(eval_duration_in_days(&[]).is_err());
    }
    #[test]
    fn in_seconds_too_few_args_errs() {
        assert!(eval_duration_in_seconds(&[]).is_err());
    }
    #[test]
    fn in_seconds_null_first_arg() {
        assert_eq!(
            eval_duration_in_seconds(&[
                Value::LocalDateTime(jiff::civil::DateTime::new(2024, 1, 1, 0, 0, 0, 0).unwrap()),
                Value::Null
            ])
            .unwrap(),
            Value::Null
        );
    }
    #[test]
    fn in_months_datetimes() {
        let z1: jiff::Zoned = "2024-01-01T00:00:00+00:00[UTC]".parse().unwrap();
        let z2: jiff::Zoned = "2024-04-01T00:00:00+00:00[UTC]".parse().unwrap();
        match eval_duration_in_months(&[
            Value::DateTime(Box::new(z1)),
            Value::DateTime(Box::new(z2)),
        ])
        .unwrap()
        {
            Value::Duration(d) => assert_eq!(d.months, 3),
            _ => panic!(),
        }
    }
    #[test]
    fn in_days_datetimes() {
        let z1: jiff::Zoned = "2024-01-01T00:00:00+00:00[UTC]".parse().unwrap();
        let z2: jiff::Zoned = "2024-01-08T00:00:00+00:00[UTC]".parse().unwrap();
        match eval_duration_in_days(&[Value::DateTime(Box::new(z1)), Value::DateTime(Box::new(z2))])
            .unwrap()
        {
            Value::Duration(d) => assert_eq!(d.days, 7),
            _ => panic!(),
        }
    }
    #[test]
    fn in_seconds_datetimes() {
        let z1: jiff::Zoned = "2024-01-01T00:00:00+00:00[UTC]".parse().unwrap();
        let z2: jiff::Zoned = "2024-01-01T00:01:00+00:00[UTC]".parse().unwrap();
        match eval_duration_in_seconds(&[
            Value::DateTime(Box::new(z1)),
            Value::DateTime(Box::new(z2)),
        ])
        .unwrap()
        {
            Value::Duration(d) => assert!(d.nanos > 0),
            _ => panic!(),
        }
    }
    #[test]
    fn is_time_only_zoned_time() {
        assert!(is_time_only(&Value::Time(
            jiff::civil::Time::new(0, 0, 0, 0).unwrap(),
            0
        )));
    }
    #[test]
    fn has_date_localdatetime() {
        assert!(has_date_component(&Value::LocalDateTime(
            jiff::civil::DateTime::new(2024, 1, 1, 0, 0, 0, 0).unwrap()
        )));
    }
}
