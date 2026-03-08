//! Temporal arithmetic: temporal +/- duration, duration add/sub/mul/div,
//! and duration.between / duration.inMonths / duration.inDays / duration.inSeconds.

mod date_ops;
mod duration_ops;
mod time_ops;

use crate::types::{CypherDuration, Value};

// Re-export all public items
pub use self::duration_ops::{duration_add, duration_div, duration_mul, duration_mul_f64, duration_sub};
pub use self::date_ops::temporal_add_duration;
pub(crate) use self::time_ops::{
    eval_duration_between, eval_duration_in_days, eval_duration_in_months, eval_duration_in_seconds,
};

// Shared helpers used by multiple arithmetic submodules

/// Helper: extract civil DateTime from any temporal for duration calculations.
fn temporal_to_civil_datetime(val: &Value) -> Option<jiff::civil::DateTime> {
    match val {
        Value::Date(d) => Some(d.to_datetime(jiff::civil::Time::midnight())),
        Value::LocalTime(t) => {
            jiff::civil::Date::new(0, 1, 1)
                .ok()
                .map(|d| d.to_datetime(*t))
        }
        Value::Time(t, _) => {
            jiff::civil::Date::new(1970, 1, 1)
                .ok()
                .map(|d| d.to_datetime(*t))
        }
        Value::LocalDateTime(dt) => Some(*dt),
        Value::DateTime(zdt) => Some(jiff::civil::DateTime::from(&**zdt)),
        _ => None,
    }
}

/// Check if a temporal has only time (no date component meaningful for calendar math).
fn is_time_only(val: &Value) -> bool {
    match val {
        Value::LocalTime(_) => true,
        Value::Time(_, _) => true,
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
        Value::DateTime(zdt) => {
            let d = jiff::civil::Date::from(zdt.as_ref());
            !(d.year() == 1970 && d.month() == 1 && d.day() == 1)
        }
        _ => false,
    }
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

/// Get the civil time-of-day nanoseconds for a temporal value.
fn civil_time_nanos(val: &Value) -> i64 {
    let dt = temporal_to_civil_datetime(val).unwrap_or_else(|| {
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
