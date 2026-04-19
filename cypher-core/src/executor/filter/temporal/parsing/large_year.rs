//! Large-year date/datetime sentinel handling and proleptic Gregorian helpers.
//!
//! For years outside jiff's supported range (roughly ±9999), we store
//! large dates as special Map values with sentinel keys.

use crate::types::{CypherDuration, Value};

pub(crate) const LARGE_DATE_KEY: &str = "__ldate_y";

/// Parse a date string with potentially extreme years (e.g. -999999999-01-01).
/// Returns (year, month, day) on success, None if the string doesn't match.
pub(crate) fn parse_large_date_str(s: &str) -> Option<(i64, i32, i32)> {
    let s = s.trim();
    let (sign, rest) = if let Some(r) = s.strip_prefix('-') {
        (-1i64, r)
    } else if let Some(r) = s.strip_prefix('+') {
        (1i64, r)
    } else {
        (1i64, s)
    };
    let mut parts = rest.splitn(3, '-');
    let year_str = parts.next()?;
    let month_str = parts.next()?;
    let day_str = parts.next()?;
    let year: i64 = year_str.parse().ok()?;
    let month: i32 = month_str.parse().ok()?;
    let day: i32 = day_str.parse().ok()?;
    Some((sign * year, month, day))
}

/// Create a large-date sentinel Map value.
pub(crate) fn large_date_value(year: i64, month: i32, day: i32) -> Value {
    let mut m = indexmap::IndexMap::new();
    m.insert(LARGE_DATE_KEY.into(), Value::Int(year));
    m.insert("__ldate_m".into(), Value::Int(month as i64));
    m.insert("__ldate_d".into(), Value::Int(day as i64));
    Value::Map(m)
}

/// Create a large-localdatetime sentinel Map value.
pub(crate) fn large_ldt_value(
    year: i64,
    month: i32,
    day: i32,
    hour: i32,
    minute: i32,
    second: i32,
    nanos: i32,
) -> Value {
    let mut m = indexmap::IndexMap::new();
    m.insert("__lldt_y".into(), Value::Int(year));
    m.insert("__lldt_m".into(), Value::Int(month as i64));
    m.insert("__lldt_d".into(), Value::Int(day as i64));
    m.insert("__lldt_h".into(), Value::Int(hour as i64));
    m.insert("__lldt_min".into(), Value::Int(minute as i64));
    m.insert("__lldt_s".into(), Value::Int(second as i64));
    m.insert("__lldt_ns".into(), Value::Int(nanos as i64));
    Value::Map(m)
}

/// Extract large-date components from a sentinel Map value.
pub(crate) fn extract_large_date(val: &Value) -> Option<(i64, i32, i32)> {
    if let Value::Map(m) = val {
        if let (Some(Value::Int(y)), Some(Value::Int(mo)), Some(Value::Int(d))) = (
            m.get(LARGE_DATE_KEY),
            m.get("__ldate_m"),
            m.get("__ldate_d"),
        ) {
            return Some((*y, *mo as i32, *d as i32));
        }
    }
    None
}

/// Extract large-localdatetime components from a sentinel Map value.
pub(crate) fn extract_large_ldt(val: &Value) -> Option<(i64, i32, i32, i32, i32, i32, i32)> {
    if let Value::Map(m) = val {
        if let Some(Value::Int(y)) = m.get("__lldt_y") {
            let get_i32 = |key: &str| {
                m.get(key)
                    .and_then(|v| {
                        if let Value::Int(i) = v {
                            Some(*i as i32)
                        } else {
                            None
                        }
                    })
                    .unwrap_or(0)
            };
            return Some((
                *y,
                get_i32("__lldt_m"),
                get_i32("__lldt_d"),
                get_i32("__lldt_h"),
                get_i32("__lldt_min"),
                get_i32("__lldt_s"),
                get_i32("__lldt_ns"),
            ));
        }
    }
    None
}

/// Compute days from a proleptic Gregorian epoch (year 0, Jan 1) for a given date.
/// Handles years of arbitrary magnitude using i128 arithmetic.
pub(crate) fn proleptic_gregorian_days(year: i64, month: i32, day: i32) -> i128 {
    let y = if month <= 2 {
        year as i128 - 1
    } else {
        year as i128
    };
    let m = if month <= 2 {
        month as i128 + 12
    } else {
        month as i128
    };
    let d = day as i128;
    365 * y + y.div_euclid(4) - y.div_euclid(100) + y.div_euclid(400) + (153 * (m - 3) + 2) / 5 + d
}

/// Compute `duration.between` for large dates using calendar arithmetic.
pub(crate) fn large_date_duration_between(
    y1: i64,
    m1: i32,
    d1: i32,
    y2: i64,
    m2: i32,
    d2: i32,
) -> CypherDuration {
    let mut year_diff = y2 - y1;
    let mut month_diff = m2 - m1;
    let mut day_diff = d2 - d1;
    if day_diff < 0 {
        month_diff -= 1;
        let (prev_y, prev_m) = if m2 == 1 { (y2 - 1, 12) } else { (y2, m2 - 1) };
        day_diff += days_in_month(prev_y, prev_m);
    }
    if month_diff < 0 {
        year_diff -= 1;
        month_diff += 12;
    }
    let total_months = year_diff * 12 + month_diff as i64;
    CypherDuration {
        months: total_months,
        days: day_diff as i64,
        nanos: 0,
    }
}

/// Days in a given month of a given year in the proleptic Gregorian calendar.
pub(crate) fn days_in_month(year: i64, month: i32) -> i32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => unreachable!("invalid month: {month}"),
    }
}

pub(crate) fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

pub(crate) type LargeLdt = (i64, i32, i32, i32, i32, i32, i32);

pub(crate) fn ldt_nanos(h: i32, min: i32, s: i32, ns: i32) -> i128 {
    h as i128 * 3_600_000_000_000
        + min as i128 * 60_000_000_000
        + s as i128 * 1_000_000_000
        + ns as i128
}

/// Compute `duration.inSeconds` between two large-year datetimes.
pub(crate) fn large_ldt_in_seconds(ldt1: LargeLdt, ldt2: LargeLdt) -> CypherDuration {
    let (y1, mo1, d1, h1, min1, s1, ns1) = ldt1;
    let (y2, mo2, d2, h2, min2, s2, ns2) = ldt2;
    let days1 = proleptic_gregorian_days(y1, mo1, d1);
    let days2 = proleptic_gregorian_days(y2, mo2, d2);
    let diff_days = days2 - days1;
    let t1_nanos = ldt_nanos(h1, min1, s1, ns1);
    let t2_nanos = ldt_nanos(h2, min2, s2, ns2);
    let total_nanos = diff_days * 86_400_000_000_000 + (t2_nanos - t1_nanos);
    CypherDuration {
        months: 0,
        days: 0,
        nanos: total_nanos,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_date_pos() {
        assert_eq!(
            parse_large_date_str("999999999-01-01").unwrap().0,
            999999999
        );
    }
    #[test]
    fn large_date_neg() {
        assert_eq!(
            parse_large_date_str("-999999999-01-01").unwrap().0,
            -999999999
        );
    }
    #[test]
    fn large_date_roundtrip() {
        let (y, m, _) = extract_large_date(&large_date_value(123456789, 6, 15)).unwrap();
        assert_eq!(y, 123456789);
        assert_eq!(m, 6);
    }
    #[test]
    fn large_ldt_roundtrip() {
        let (y, _, _, h, _, _, _) =
            extract_large_ldt(&large_ldt_value(123, 6, 15, 10, 30, 45, 999)).unwrap();
        assert_eq!(y, 123);
        assert_eq!(h, 10);
    }
    #[test]
    fn extract_non_sentinel() {
        assert!(extract_large_date(&Value::Int(42)).is_none());
    }
    #[test]
    fn proleptic_leap() {
        assert_eq!(
            proleptic_gregorian_days(2001, 1, 1) - proleptic_gregorian_days(2000, 1, 1),
            366
        );
    }
    #[test]
    fn leap_year() {
        assert!(is_leap_year(2000));
        assert!(!is_leap_year(1900));
    }
    #[test]
    fn days_feb() {
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2023, 2), 28);
    }
    #[test]
    fn large_dur_year() {
        assert_eq!(
            large_date_duration_between(2024, 1, 1, 2025, 1, 1).months,
            12
        );
    }
    #[test]
    fn large_ldt_secs() {
        assert_eq!(
            large_ldt_in_seconds((2024, 1, 1, 0, 0, 0, 0), (2024, 1, 1, 1, 0, 0, 0)).nanos,
            3_600_000_000_000
        );
    }
    #[test]
    fn ldt_nanos_zero() {
        assert_eq!(ldt_nanos(0, 0, 0, 0), 0);
    }
    #[test]
    fn ldt_nanos_hour() {
        assert_eq!(ldt_nanos(1, 0, 0, 0), 3_600_000_000_000);
    }
}
