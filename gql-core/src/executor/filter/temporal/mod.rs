//! All temporal functions: date, time, datetime, duration, truncation, proleptic calendar.
//! Also includes temporal property access, temporal arithmetic, and duration operations.

mod arithmetic;
mod constructors;
mod parsing;
mod properties;
mod truncation;

use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

// Public API
pub use self::arithmetic::{
    duration_add, duration_div, duration_mul, duration_mul_f64, duration_sub, temporal_add_duration,
};
pub use self::properties::temporal_property_access;
pub(crate) use self::properties::time_to_utc_nanos;

// Crate-visible dispatch
pub(crate) use self::arithmetic::{
    eval_duration_between, eval_duration_in_days, eval_duration_in_months, eval_duration_in_seconds,
};
pub(crate) use self::constructors::{
    builtin_datetime_fromepoch, builtin_datetime_fromepochmillis, builtin_temporal_realtime_date,
    builtin_temporal_realtime_datetime, builtin_temporal_realtime_localdatetime,
    builtin_temporal_realtime_localtime, builtin_temporal_realtime_time, eval_date_constructor,
    eval_datetime_constructor, eval_duration_constructor, eval_localdatetime_constructor,
    eval_localtime_constructor, eval_time_constructor,
};
pub(crate) use self::truncation::{
    eval_date_truncate, eval_datetime_truncate, eval_localdatetime_truncate,
    eval_localtime_truncate, eval_time_truncate,
};

// Shared helpers
fn temporal_parse_error(func: &str, msg: impl std::fmt::Display) -> GqlError {
    GqlError::Execution(ExecutionError::TypeMismatch {
        expected: format!("valid {func}() argument"),
        actual: msg.to_string(),
    })
}

/// Extract an i64 field from a Value::Map, returning a default if missing.
fn map_get_int(
    m: &indexmap::IndexMap<Box<str>, Value>,
    key: &str,
    default: i64,
) -> Result<i64, GqlError> {
    match m.get(key) {
        Some(Value::Int(n)) => Ok(*n),
        Some(Value::Null) | None => Ok(default),
        Some(other) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: format!("Integer for '{key}'"),
            actual: other.type_name().to_string(),
        })),
    }
}

/// Extract an optional string field from a Value::Map.
fn map_get_str(m: &indexmap::IndexMap<Box<str>, Value>, key: &str) -> Option<String> {
    match m.get(key) {
        Some(Value::Str(s)) => Some(s.to_string()),
        _ => None,
    }
}

/// Parse a timezone string, supporting both IANA names (e.g. "America/New_York")
/// and fixed UTC offsets (e.g. "+01:00", "-05:30", "Z").
fn parse_timezone(tz_str: &str) -> Result<jiff::tz::TimeZone, String> {
    // Try fixed offset FIRST for strings starting with '+' or '-'
    // to avoid IANA lookup failures or ambiguous matches.
    if tz_str == "Z" || tz_str == "UTC" {
        return Ok(jiff::tz::TimeZone::UTC);
    }
    if let Some(offset_str) = tz_str
        .strip_prefix('+')
        .or_else(|| tz_str.strip_prefix('-'))
    {
        let negative = tz_str.starts_with('-');
        let parts: Vec<&str> = offset_str.split(':').collect();
        let total_seconds = if parts.len() == 3 {
            // HH:MM:SS format (e.g. "+02:05:59")
            let h: i32 = parts[0]
                .parse()
                .map_err(|e: std::num::ParseIntError| e.to_string())?;
            let m: i32 = parts[1]
                .parse()
                .map_err(|e: std::num::ParseIntError| e.to_string())?;
            let s: i32 = parts[2]
                .parse()
                .map_err(|e: std::num::ParseIntError| e.to_string())?;
            (h * 3600 + m * 60 + s) * if negative { -1 } else { 1 }
        } else if parts.len() == 2 {
            let h: i32 = parts[0]
                .parse()
                .map_err(|e: std::num::ParseIntError| e.to_string())?;
            let m: i32 = parts[1]
                .parse()
                .map_err(|e: std::num::ParseIntError| e.to_string())?;
            (h * 3600 + m * 60) * if negative { -1 } else { 1 }
        } else if offset_str.len() == 4 {
            let h: i32 = offset_str[..2]
                .parse()
                .map_err(|e: std::num::ParseIntError| e.to_string())?;
            let m: i32 = offset_str[2..]
                .parse()
                .map_err(|e: std::num::ParseIntError| e.to_string())?;
            (h * 3600 + m * 60) * if negative { -1 } else { 1 }
        } else if offset_str.len() == 2 {
            let h: i32 = offset_str
                .parse()
                .map_err(|e: std::num::ParseIntError| e.to_string())?;
            h * 3600 * if negative { -1 } else { 1 }
        } else {
            return Err(format!("invalid timezone offset: {tz_str}"));
        };
        let offset = jiff::tz::Offset::from_seconds(total_seconds).map_err(|e| e.to_string())?;
        return Ok(jiff::tz::TimeZone::fixed(offset));
    }
    // Try IANA name (for named timezones like 'Europe/Stockholm', 'America/New_York')
    if let Ok(tz) = jiff::tz::TimeZone::get(tz_str) {
        return Ok(tz);
    }
    Err(format!("unknown timezone: {tz_str}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse_tz_utc() {
        assert_eq!(parse_timezone("UTC").unwrap(), jiff::tz::TimeZone::UTC);
    }
    #[test]
    fn parse_tz_z() {
        assert_eq!(parse_timezone("Z").unwrap(), jiff::tz::TimeZone::UTC);
    }
    #[test]
    fn parse_tz_positive() {
        let tz = parse_timezone("+01:00").unwrap();
        assert_eq!(
            tz,
            jiff::tz::TimeZone::fixed(jiff::tz::Offset::from_seconds(3600).unwrap())
        );
    }
    #[test]
    fn parse_tz_negative() {
        let tz = parse_timezone("-05:30").unwrap();
        assert_eq!(
            tz,
            jiff::tz::TimeZone::fixed(jiff::tz::Offset::from_seconds(-5 * 3600 - 30 * 60).unwrap())
        );
    }
    #[test]
    fn parse_tz_compact() {
        let tz = parse_timezone("+0530").unwrap();
        assert_eq!(
            tz,
            jiff::tz::TimeZone::fixed(jiff::tz::Offset::from_seconds(5 * 3600 + 30 * 60).unwrap())
        );
    }
    #[test]
    fn parse_tz_iana() {
        assert!(parse_timezone("America/New_York")
            .unwrap()
            .iana_name()
            .is_some());
    }
    #[test]
    fn parse_tz_unknown() {
        assert!(parse_timezone("Not/A/Timezone").is_err());
    }
    #[test]
    fn map_get_int_present() {
        let mut m = indexmap::IndexMap::new();
        m.insert("year".into(), Value::Int(2024));
        assert_eq!(map_get_int(&m, "year", 0).unwrap(), 2024);
    }
    #[test]
    fn map_get_int_missing() {
        assert_eq!(
            map_get_int(&indexmap::IndexMap::new(), "year", 42).unwrap(),
            42
        );
    }
    #[test]
    fn map_get_int_wrong_type() {
        let mut m = indexmap::IndexMap::new();
        m.insert("year".into(), Value::Str("bad".into()));
        assert!(map_get_int(&m, "year", 0).is_err());
    }
    #[test]
    fn map_get_str_present() {
        let mut m = indexmap::IndexMap::new();
        m.insert("tz".into(), Value::Str("UTC".into()));
        assert_eq!(map_get_str(&m, "tz"), Some("UTC".to_string()));
    }
    #[test]
    fn map_get_str_missing() {
        assert_eq!(map_get_str(&indexmap::IndexMap::new(), "tz"), None);
    }
}
