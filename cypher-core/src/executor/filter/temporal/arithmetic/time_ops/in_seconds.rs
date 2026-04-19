//! `duration.inSeconds(t1, t2)` — diff producing nanos (with day-aware handling).

use crate::error::{ExecutionError, GqlError};
use crate::types::{CypherDuration, Value};

use super::super::super::parsing::{extract_large_ldt, large_ldt_in_seconds};
use super::super::super::properties::time_to_utc_nanos;
use super::super::super::temporal_parse_error;
use super::super::{has_date_component, temporal_to_civil_datetime};

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
