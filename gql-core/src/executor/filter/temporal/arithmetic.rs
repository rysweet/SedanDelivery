//! Temporal arithmetic: temporal +/- duration, duration add/sub/mul/div,
//! and duration.between / duration.inMonths / duration.inDays / duration.inSeconds.

use crate::error::{ExecutionError, GqlError};
use crate::types::{CypherDuration, Value};

use super::parsing::{
    extract_large_date, extract_large_ldt, large_date_duration_between, large_ldt_in_seconds,
};
use super::properties::time_to_utc_nanos;
use super::temporal_parse_error;

pub fn temporal_add_duration(
    temporal: &Value,
    dur: &CypherDuration,
    subtract: bool,
) -> Result<Value, GqlError> {
    let (months, days, nanos) = if subtract {
        (-dur.months, -dur.days, -dur.nanos)
    } else {
        (dur.months, dur.days, dur.nanos)
    };
    match temporal {
        Value::Date(d) => {
            let d1 = if months != 0 {
                d.checked_add(jiff::Span::new().months(months))
                    .map_err(|e| temporal_parse_error("temporal arithmetic", e))?
            } else {
                *d
            };
            let d2 = if days != 0 {
                d1.checked_add(jiff::Span::new().days(days))
                    .map_err(|e| temporal_parse_error("temporal arithmetic", e))?
            } else {
                d1
            };
            // For Date arithmetic, the nanos (time component) contributes whole days only.
            // Fractional durations (e.g. 12.5 years) cascade: years→months→days→nanos.
            // We must apply any whole-day portion from nanos to the date.
            // Use truncating division (toward zero) to correctly handle both
            // positive (add) and negative (subtract) nanos without sign errors.
            let extra_days = nanos / 86_400_000_000_000_i128;
            let d3 = if extra_days != 0 {
                d2.checked_add(jiff::Span::new().days(extra_days as i64))
                    .map_err(|e| temporal_parse_error("temporal arithmetic", e))?
            } else {
                d2
            };
            Ok(Value::Date(d3))
        }
        Value::LocalTime(t) => {
            let cur_nanos = t.hour() as i128 * 3_600_000_000_000
                + t.minute() as i128 * 60_000_000_000
                + t.second() as i128 * 1_000_000_000
                + t.subsec_nanosecond() as i128;
            let total = cur_nanos + nanos;
            let day_nanos: i128 = 24 * 3_600_000_000_000;
            let wrapped = ((total % day_nanos) + day_nanos) % day_nanos;
            let h = (wrapped / 3_600_000_000_000) as i8;
            let rem = wrapped % 3_600_000_000_000;
            let m = (rem / 60_000_000_000) as i8;
            let rem = rem % 60_000_000_000;
            let s = (rem / 1_000_000_000) as i8;
            let ns = (rem % 1_000_000_000) as i32;
            let result = jiff::civil::Time::new(h, m, s, ns)
                .map_err(|e| temporal_parse_error("temporal arithmetic", e))?;
            Ok(Value::LocalTime(result))
        }
        Value::Time(t, offset) => {
            // Same as LocalTime but preserves offset
            let cur_nanos = t.hour() as i128 * 3_600_000_000_000
                + t.minute() as i128 * 60_000_000_000
                + t.second() as i128 * 1_000_000_000
                + t.subsec_nanosecond() as i128;
            let total = cur_nanos + nanos;
            let day_nanos: i128 = 24 * 3_600_000_000_000;
            let wrapped = ((total % day_nanos) + day_nanos) % day_nanos;
            let h = (wrapped / 3_600_000_000_000) as i8;
            let rem = wrapped % 3_600_000_000_000;
            let m = (rem / 60_000_000_000) as i8;
            let rem = rem % 60_000_000_000;
            let s = (rem / 1_000_000_000) as i8;
            let ns = (rem % 1_000_000_000) as i32;
            let result = jiff::civil::Time::new(h, m, s, ns)
                .map_err(|e| temporal_parse_error("temporal arithmetic", e))?;
            Ok(Value::Time(result, *offset))
        }
        Value::LocalDateTime(dt) => {
            let dt1 = if months != 0 {
                dt.checked_add(jiff::Span::new().months(months))
                    .map_err(|e| temporal_parse_error("temporal arithmetic", e))?
            } else {
                *dt
            };
            let dt2 = if days != 0 {
                dt1.checked_add(jiff::Span::new().days(days))
                    .map_err(|e| temporal_parse_error("temporal arithmetic", e))?
            } else {
                dt1
            };
            let dt3 = if nanos != 0 {
                let nanos_i64 = i64::try_from(nanos).map_err(|_| {
                    GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: "duration nanoseconds within i64 range".to_string(),
                        actual: format!("nanoseconds {nanos} overflow i64 range"),
                    })
                })?;
                dt2.checked_add(jiff::SignedDuration::from_nanos(nanos_i64))
                    .map_err(|e| temporal_parse_error("temporal arithmetic", e))?
            } else {
                dt2
            };
            Ok(Value::LocalDateTime(dt3))
        }
        Value::DateTime(zdt) => {
            let zdt1 = if months != 0 {
                zdt.as_ref()
                    .clone()
                    .checked_add(jiff::Span::new().months(months))
                    .map_err(|e| temporal_parse_error("temporal arithmetic", e))?
            } else {
                zdt.as_ref().clone()
            };
            let zdt2 = if days != 0 {
                zdt1.checked_add(jiff::Span::new().days(days))
                    .map_err(|e| temporal_parse_error("temporal arithmetic", e))?
            } else {
                zdt1
            };
            let zdt3 = if nanos != 0 {
                let nanos_i64 = i64::try_from(nanos).map_err(|_| {
                    GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: "duration nanoseconds within i64 range".to_string(),
                        actual: format!("nanoseconds {nanos} overflow i64 range"),
                    })
                })?;
                zdt2.checked_add(jiff::SignedDuration::from_nanos(nanos_i64))
                    .map_err(|e| temporal_parse_error("temporal arithmetic", e))?
            } else {
                zdt2
            };
            Ok(Value::DateTime(Box::new(zdt3)))
        }
        _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "temporal value for arithmetic".to_string(),
            actual: temporal.type_name().to_string(),
        })),
    }
}

/// Add two durations.
pub fn duration_add(a: &CypherDuration, b: &CypherDuration) -> CypherDuration {
    CypherDuration {
        months: a.months + b.months,
        days: a.days + b.days,
        nanos: a.nanos + b.nanos,
    }
}

/// Subtract two durations.
pub fn duration_sub(a: &CypherDuration, b: &CypherDuration) -> CypherDuration {
    CypherDuration {
        months: a.months - b.months,
        days: a.days - b.days,
        nanos: a.nanos - b.nanos,
    }
}

/// Multiply a duration by a float. Uses rational approximation for clean fractions (e.g. 0.5 → ÷2).
pub fn duration_mul_f64(d: &CypherDuration, f: f64) -> CypherDuration {
    // Detect simple rational fractions: if f ≈ 1/k for small integer k, use integer division.
    if f != 0.0 {
        let inv = 1.0 / f;
        let k = inv.round() as i64;
        if k != 0 && (inv - k as f64).abs() < 1e-9 {
            if let Ok(result) = duration_div(d, k) {
                return result;
            }
        }
    }
    // Detect integer multiples: if f ≈ k for small integer k, use integer multiplication.
    let n = f.round() as i64;
    if (f - n as f64).abs() < 1e-9 {
        return duration_mul(d, n);
    }
    // General float multiplication: scale months, then cascade remainder to days+nanos.
    let new_months_f = d.months as f64 * f;
    let new_months = new_months_f.trunc() as i64;
    let frac_months = new_months_f.fract();
    const MONTH_NANOS_F: f64 = 2_629_746_000_000_000.0;
    const DAY_NANOS_F: f64 = 86_400_000_000_000.0;
    let remaining_nanos_f =
        frac_months * MONTH_NANOS_F + d.days as f64 * f * DAY_NANOS_F + d.nanos as f64 * f;
    let new_days = (remaining_nanos_f / DAY_NANOS_F).trunc() as i64;
    let new_nanos = (remaining_nanos_f - new_days as f64 * DAY_NANOS_F).round() as i128;
    CypherDuration {
        months: new_months,
        days: new_days,
        nanos: new_nanos,
    }
}

/// Multiply a duration by an integer.
pub fn duration_mul(d: &CypherDuration, n: i64) -> CypherDuration {
    CypherDuration {
        months: d.months * n,
        days: d.days * n,
        nanos: d.nanos * n as i128,
    }
}

/// Divide a duration by an integer.
pub fn duration_div(d: &CypherDuration, n: i64) -> Result<CypherDuration, GqlError> {
    if n == 0 {
        return Err(GqlError::Execution(ExecutionError::DivisionByZero));
    }
    let new_months = d.months / n;
    let rem_months = d.months % n;
    // Convert remainder months + days + nanos into total sub-nanos, then divide.
    // Use i128 to avoid overflow for large values.
    // Average Gregorian month = 2,629,746 seconds.
    const MONTH_NANOS: i128 = 2_629_746_000_000_000;
    const DAY_NANOS: i128 = 86_400_000_000_000;
    let total_sub: i128 = rem_months as i128 * MONTH_NANOS + d.days as i128 * DAY_NANOS + d.nanos;
    let new_sub = total_sub / n as i128;
    // Split into days and nanos, preserving sign (use floor division for days)
    let new_days = new_sub.div_euclid(DAY_NANOS) as i64;
    let new_nanos = new_sub.rem_euclid(DAY_NANOS);
    Ok(CypherDuration {
        months: new_months,
        days: new_days,
        nanos: new_nanos,
    })
}

/// Helper: extract civil DateTime from any temporal for duration calculations.
fn temporal_to_civil_datetime(val: &Value) -> Option<jiff::civil::DateTime> {
    match val {
        Value::Date(d) => Some(d.to_datetime(jiff::civil::Time::midnight())),
        Value::LocalTime(t) => {
            // safe: constant date year=0 month=1 day=1 is always valid
            Some(
                jiff::civil::Date::new(0, 1, 1)
                    .expect("ISO 8601 year 0")
                    .to_datetime(*t),
            )
        }
        Value::Time(t, _) => {
            // safe: constant epoch date 1970-01-01 is always valid
            Some(
                jiff::civil::Date::new(1970, 1, 1)
                    .expect("epoch")
                    .to_datetime(*t),
            )
        }
        Value::LocalDateTime(dt) => Some(*dt),
        Value::DateTime(zdt) => Some(jiff::civil::DateTime::from(&**zdt)),
        _ => None,
    }
}

/// Check if a temporal has only time (no date component meaningful for calendar math).
/// Includes `time()` values stored as DateTime with epoch date 1970-01-01.
fn is_time_only(val: &Value) -> bool {
    match val {
        Value::LocalTime(_) => true,
        Value::Time(_, _) => true,
        // ZonedTime is stored as DateTime with epoch date 1970-01-01
        Value::DateTime(zdt) => {
            let d = jiff::civil::Date::from(zdt.as_ref());
            d.year() == 1970 && d.month() == 1 && d.day() == 1
        }
        _ => false,
    }
}

/// Extract a date from any temporal value.
fn extract_date(val: &Value) -> Option<jiff::civil::Date> {
    match val {
        Value::Date(d) => Some(*d),
        Value::LocalDateTime(dt) => Some(dt.date()),
        Value::DateTime(zdt) => Some(jiff::civil::Date::from(&**zdt)),
        _ => None,
    }
}

/// Check if a temporal has date component.
fn has_date_component(val: &Value) -> bool {
    match val {
        Value::Date(_) | Value::LocalDateTime(_) => true,
        // DateTime has date component unless it's a ZonedTime (epoch date 1970-01-01)
        Value::DateTime(zdt) => {
            let d = jiff::civil::Date::from(zdt.as_ref());
            !(d.year() == 1970 && d.month() == 1 && d.day() == 1)
        }
        _ => false,
    }
}

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
            // Calendar-based diff for months only; remaining as pure nanoseconds (no calendar days).
            // This correctly handles DST transitions (e.g. PT6H instead of P1DT-19H).
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

            // Subtract only the months portion from total nanos
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
            // Mixed DateTime pair (e.g. time + real-datetime): use UTC time-of-day only.
            // Using full timestamps would include the date difference (wrong for time-only cases).
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

    // Check if both inputs have a time component (i.e. both are LocalDateTime)
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
            // Both are LocalDateTime: compute remaining as pure nanoseconds (no calendar days).
            // e.g. 2018-01-01T12:00 → 2018-01-02T10:00 = PT22H (not P1DT-2H)
            let dt1_advanced_dt = d1_advanced.to_datetime(dt1.time());
            // Use UTC zone for civil datetime → timestamp conversion (no DST effects)
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

        // At least one is Date-only: include calendar days + time diff
        let remaining_days_span = d1_advanced
            .until(d2)
            .map_err(|e| temporal_parse_error("duration.between", e))?;
        let mut remaining_days = remaining_days_span.get_days() as i64;

        // Time diff using local civil times
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

        // Normalize: avoid mixed-sign days and time (borrow/carry 1 day when needed)
        // e.g. 3 days + -21:40:36s → 2 days + 2:19:24s
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
        // Time-only branch: at least one arg is LocalTime (no timezone).
        // Use civil time-of-day for all comparisons (ignores TZ of any ZonedTime arg).
        // Note: ZonedTime+ZonedTime is handled by the (DateTime,DateTime) branch above.
        let t1_nanos = civil_time_nanos(&vals[0]);
        let t2_nanos = civil_time_nanos(&vals[1]);
        let diff = t2_nanos - t1_nanos;
        let secs = diff / 1_000_000_000;
        let subsec = (diff % 1_000_000_000) as i32;
        let sp = build_duration_span(0, 0, secs, subsec);
        Ok(Value::Duration(sp))
    }
}

/// Get the civil time-of-day nanoseconds for a temporal value.
fn civil_time_nanos(val: &Value) -> i64 {
    let dt = temporal_to_civil_datetime(val).unwrap_or_else(|| {
        // safe: constant date 0001-01-01 is always valid
        jiff::civil::Date::new(1, 1, 1)
            .expect("0001-01-01")
            .to_datetime(jiff::civil::Time::midnight())
    });
    let t = dt.time();
    t.hour() as i64 * 3_600_000_000_000
        + t.minute() as i64 * 60_000_000_000
        + t.second() as i64 * 1_000_000_000
        + t.subsec_nanosecond() as i64
}

fn build_duration_span(
    total_months: i64,
    days: i64,
    secs: i64,
    subsec_nanos: i32,
) -> CypherDuration {
    let total_nanos = secs as i128 * 1_000_000_000 + subsec_nanos as i128;
    CypherDuration {
        months: total_months,
        days,
        nanos: total_nanos,
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
    // If either is time-only, return zero duration
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
    // LocalDateTime → DateTime is incompatible (no timezone in LHS): return PT0S
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

    // Adjust total_months by 1 if the time-of-day comparison shows we haven't
    // fully completed the last month. Only applies when BOTH values are DateTime
    // (with timezone), or when LHS is DateTime and RHS is Date (treat Date as midnight UTC).
    // For LocalDateTime comparisons, date-only calculation is already exact.
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
    // LocalDateTime → DateTime is incompatible (no timezone in LHS): return PT0S
    if matches!(&vals[0], Value::LocalDateTime(_)) && matches!(&vals[1], Value::DateTime(_)) {
        return Ok(Value::Duration(CypherDuration {
            months: 0,
            days: 0,
            nanos: 0,
        }));
    }

    // If either value is a DateTime (with timezone), use timestamp-based calculation.
    // Convert both to timestamps treating Date as midnight UTC.
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
            // Floor division: round toward negative infinity
            let day_ns: i128 = 86_400_000_000_000;
            let diff_days = diff_ns.div_euclid(day_ns) as i64;
            return Ok(Value::Duration(CypherDuration {
                months: 0,
                days: diff_days,
                nanos: 0,
            }));
        }
    }
    // Fall back to calendar date diff for non-zoned values
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

    // For zoned datetime pairs, use timestamp-based or time-of-day calculation.
    if let (Value::DateTime(z1), Value::DateTime(z2)) = (&vals[0], &vals[1]) {
        let v1_has_date = has_date_component(&vals[0]);
        let v2_has_date = has_date_component(&vals[1]);
        if v1_has_date && v2_has_date {
            // Both are real datetimes: use exact timestamp diff.
            let ts1 = z1.timestamp();
            let ts2 = z2.timestamp();
            let total_nanos = ts2.as_nanosecond() - ts1.as_nanosecond();
            return Ok(Value::Duration(CypherDuration {
                months: 0,
                days: 0,
                nanos: total_nanos,
            }));
        } else {
            // Mixed: one is time-only (epoch date). Compare UTC time-of-day only.
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
        // For LocalTime input, combine with the DateTime's date for proper DST-aware calc.
        let other_civil: Option<jiff::civil::DateTime> = match other_val {
            Value::LocalTime(t) => {
                // Use the date from the zoned datetime, combined with the local time
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

    // For mixed datetime + time or two localdatetimes, use civil calculation
    let dt1 = temporal_to_civil_datetime(&vals[0])
        .ok_or_else(|| temporal_parse_error("duration.inSeconds", "expected temporal"))?;
    let dt2 = temporal_to_civil_datetime(&vals[1])
        .ok_or_else(|| temporal_parse_error("duration.inSeconds", "expected temporal"))?;

    let both_have_dates = has_date_component(&vals[0]) && has_date_component(&vals[1]);

    if both_have_dates {
        // Convert both to total seconds from epoch-like reference
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
        // Time-only
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
    fn dur(months: i64, days: i64, nanos: i128) -> CypherDuration {
        CypherDuration {
            months,
            days,
            nanos,
        }
    }
    #[test]
    fn add_durs() {
        let r = duration_add(&dur(12, 5, 1_000_000_000), &dur(3, 2, 500_000_000));
        assert_eq!(r.months, 15);
        assert_eq!(r.days, 7);
    }
    #[test]
    fn sub_durs() {
        let r = duration_sub(&dur(12, 5, 1_000_000_000), &dur(3, 2, 500_000_000));
        assert_eq!(r.months, 9);
        assert_eq!(r.days, 3);
    }
    #[test]
    fn mul_int() {
        let r = duration_mul(&dur(2, 3, 1_000_000_000), 3);
        assert_eq!(r.months, 6);
        assert_eq!(r.days, 9);
    }
    #[test]
    fn div_int() {
        assert_eq!(duration_div(&dur(6, 0, 0), 2).unwrap().months, 3);
    }
    #[test]
    fn div_zero() {
        assert!(duration_div(&dur(6, 0, 0), 0).is_err());
    }
    #[test]
    fn mul_half() {
        assert_eq!(duration_mul_f64(&dur(12, 0, 0), 0.5).months, 6);
    }
    #[test]
    fn add_to_date() {
        let d = Value::Date(jiff::civil::Date::new(2024, 1, 1).unwrap());
        match temporal_add_duration(&d, &dur(1, 5, 0), false).unwrap() {
            Value::Date(d) => {
                assert_eq!(d.month(), 2);
                assert_eq!(d.day(), 6);
            }
            _ => panic!("Date"),
        }
    }
    #[test]
    fn sub_from_date() {
        let d = Value::Date(jiff::civil::Date::new(2024, 3, 15).unwrap());
        match temporal_add_duration(&d, &dur(1, 0, 0), true).unwrap() {
            Value::Date(d) => assert_eq!(d.month(), 2),
            _ => panic!("Date"),
        }
    }
    #[test]
    fn add_to_lt() {
        let t = Value::LocalTime(jiff::civil::Time::new(10, 30, 0, 0).unwrap());
        match temporal_add_duration(&t, &dur(0, 0, 3_600_000_000_000), false).unwrap() {
            Value::LocalTime(t) => assert_eq!(t.hour(), 11),
            _ => panic!("LT"),
        }
    }
    #[test]
    fn add_to_lt_wrap() {
        let t = Value::LocalTime(jiff::civil::Time::new(23, 0, 0, 0).unwrap());
        match temporal_add_duration(&t, &dur(0, 0, 2 * 3_600_000_000_000), false).unwrap() {
            Value::LocalTime(t) => assert_eq!(t.hour(), 1),
            _ => panic!("LT"),
        }
    }
    #[test]
    fn add_to_time() {
        let t = Value::Time(jiff::civil::Time::new(10, 0, 0, 0).unwrap(), 3600);
        match temporal_add_duration(&t, &dur(0, 0, 3_600_000_000_000), false).unwrap() {
            Value::Time(t, off) => {
                assert_eq!(t.hour(), 11);
                assert_eq!(off, 3600);
            }
            _ => panic!("T"),
        }
    }
    #[test]
    fn add_to_ldt() {
        let dt = Value::LocalDateTime(jiff::civil::DateTime::new(2024, 1, 1, 10, 0, 0, 0).unwrap());
        match temporal_add_duration(&dt, &dur(0, 1, 3_600_000_000_000), false).unwrap() {
            Value::LocalDateTime(dt) => {
                assert_eq!(dt.day(), 2);
                assert_eq!(dt.hour(), 11);
            }
            _ => panic!("LDT"),
        }
    }
    #[test]
    fn add_to_zdt() {
        let z = Value::DateTime(Box::new(
            jiff::civil::DateTime::new(2024, 1, 1, 10, 0, 0, 0)
                .unwrap()
                .to_zoned(jiff::tz::TimeZone::UTC)
                .unwrap(),
        ));
        match temporal_add_duration(&z, &dur(1, 0, 0), false).unwrap() {
            Value::DateTime(z) => assert_eq!(z.month(), 2),
            _ => panic!("DT"),
        }
    }
    #[test]
    fn add_non_temporal() {
        assert!(temporal_add_duration(&Value::Int(42), &dur(0, 0, 0), false).is_err());
    }
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
    fn has_date_lt() {
        assert!(!has_date_component(&Value::LocalTime(
            jiff::civil::Time::new(10, 0, 0, 0).unwrap()
        )));
    }
}
