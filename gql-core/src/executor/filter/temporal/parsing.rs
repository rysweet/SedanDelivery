//! ISO 8601 parsing, compact datetime format parsing, proleptic Gregorian calendar helpers,
//! and large-year date sentinel handling.

use crate::error::{ExecutionError, GqlError};
use crate::types::{CypherDuration, Value};

// ── Large-year date handling ──────────────────────────────────────────────────
// For years outside jiff's supported range (roughly ±9999), we store
// large dates as special Map values with sentinel keys.

pub(crate) const LARGE_DATE_KEY: &str = "__ldate_y";

/// Parse a date string with potentially extreme years (e.g. -999999999-01-01).
/// Returns (year, month, day) on success, None if the string doesn't match.
pub(crate) fn parse_large_date_str(s: &str) -> Option<(i64, i32, i32)> {
    let s = s.trim();
    // Match: optional +/- sign, then digits, then -MM-DD
    let (sign, rest) = if let Some(r) = s.strip_prefix('-') {
        (-1i64, r)
    } else if let Some(r) = s.strip_prefix('+') {
        (1i64, r)
    } else {
        (1i64, s)
    };
    // Find the two '-' separators for YYYY-MM-DD
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
    // Gregorian formula
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
    // Compute calendar diff: years, months, days
    let mut year_diff = y2 - y1;
    let mut month_diff = m2 - m1;
    let mut day_diff = d2 - d1;
    // Normalize: if day_diff < 0, borrow 1 month
    if day_diff < 0 {
        month_diff -= 1;
        // Add days in the previous month of y2/m2
        let (prev_y, prev_m) = if m2 == 1 { (y2 - 1, 12) } else { (y2, m2 - 1) };
        day_diff += days_in_month(prev_y, prev_m);
    }
    // Normalize: if month_diff < 0, borrow 1 year
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
        _ => 30,
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

/// Parse an ISO 8601 date string, handling all abbreviated formats that jiff may not support.
/// Formats supported beyond jiff's default:
///   - `YYYY`           (year only → Jan 1)
///   - `YYYY-MM`        (year-month → day 1)
///   - `YYYYMM`         (compact year-month)
///   - `YYYY-Www`       (ISO week date → Monday)
///   - `YYYY-Www-D`     (ISO week date with day-of-week)
///   - `YYYYWww`        (compact ISO week)
///   - `YYYYWwwD`       (compact ISO week + day)
///   - `YYYY-DDD`       (ordinal date)
///   - `YYYYDDD`        (compact ordinal date)
pub(crate) fn parse_date_str(s: &str) -> Result<jiff::civil::Date, String> {
    // First try jiff's standard parser (handles YYYY-MM-DD and RFC3339 variants)
    if let Ok(d) = s.parse::<jiff::civil::Date>() {
        return Ok(d);
    }

    let s = s.trim();

    // YYYY-Www or YYYY-Www-D (ISO week date)
    if let Some(week_part) = s
        .strip_prefix(|c: char| c.is_ascii_digit())
        .and(None::<&str>)
        .or_else(|| {
            // Extract YYYY-W portion
            if s.len() >= 8 {
                let (_year_part, rest) = s.split_at(4);
                let rest = rest.strip_prefix('-').unwrap_or(rest);
                if rest.starts_with('W') {
                    Some(rest)
                } else {
                    None
                }
            } else {
                None
            }
        })
    {
        let _ = week_part;
        // Handle YYYY-Www-D and YYYY-Www and YYYYWwwD and YYYYWww
        let (year_str, rest) = s.split_at(4);
        let year: i64 = year_str.parse().map_err(|e| format!("{e}"))?;
        let rest = rest.strip_prefix('-').unwrap_or(rest);
        if rest.starts_with('W') {
            let rest = rest.strip_prefix('W').unwrap_or(rest); // skip 'W'
            let (week_str, rest) = if rest.len() >= 2 {
                rest.split_at(2)
            } else {
                (rest, "")
            };
            let week: i64 = week_str.parse().map_err(|e| format!("{e}"))?;
            let rest = rest.strip_prefix('-').unwrap_or(rest);
            let day_of_week = if rest.is_empty() {
                1 // Monday
            } else {
                rest[..1].parse::<i64>().map_err(|e| format!("{e}"))?
            };
            return compute_week_date_str(year, week, day_of_week);
        }
    }

    // Redo parsing more systematically
    if s.len() == 4 && s.chars().all(|c| c.is_ascii_digit()) {
        // YYYY → Jan 1
        let year: i16 = s.parse().map_err(|e| format!("{e}"))?;
        return jiff::civil::Date::new(year, 1, 1).map_err(|e| e.to_string());
    }

    // Check for YYYY-Www or YYYY-Www-D or YYYYWwwD or YYYYWww
    if s.contains('W') {
        let (year_str, rest) = s.split_at(4);
        let year: i64 = year_str.parse().map_err(|e| format!("{e}"))?;
        let rest = rest.strip_prefix('-').unwrap_or(rest);
        if let Some(rest) = rest.strip_prefix('W') {
            let (week_str, rest) = if rest.len() >= 2 {
                rest.split_at(2)
            } else {
                (rest, "")
            };
            let week: i64 = week_str.parse().map_err(|e| format!("{e}"))?;
            let rest = rest.strip_prefix('-').unwrap_or(rest);
            let day_of_week = if rest.is_empty() {
                1
            } else {
                rest[..1].parse::<i64>().map_err(|e| format!("{e}"))?
            };
            return compute_week_date_str(year, week, day_of_week);
        }
    }

    // YYYY-MM (with separator, no day) or YYYYMM (without separator)
    if s.len() == 7 && s.as_bytes()[4] == b'-' && s[5..].chars().all(|c| c.is_ascii_digit()) {
        // YYYY-MM
        let year: i16 = s[..4].parse().map_err(|e| format!("{e}"))?;
        let month: i8 = s[5..].parse().map_err(|e| format!("{e}"))?;
        return jiff::civil::Date::new(year, month, 1).map_err(|e| e.to_string());
    }
    if s.len() == 6 && s.chars().all(|c| c.is_ascii_digit()) {
        // YYYYMM
        let year: i16 = s[..4].parse().map_err(|e| format!("{e}"))?;
        let month: i8 = s[4..].parse().map_err(|e| format!("{e}"))?;
        return jiff::civil::Date::new(year, month, 1).map_err(|e| e.to_string());
    }

    // YYYY-DDD (ordinal date with separator) or YYYYDDD (compact)
    if s.len() == 8 && s.as_bytes()[4] == b'-' && s[5..].chars().all(|c| c.is_ascii_digit()) {
        // YYYY-DDD
        let year: i16 = s[..4].parse().map_err(|e| format!("{e}"))?;
        let ordinal: i64 = s[5..].parse().map_err(|e| format!("{e}"))?;
        let d = jiff::civil::Date::new(year, 1, 1).map_err(|e| e.to_string())?;
        return d
            .checked_add(jiff::Span::new().days(ordinal - 1))
            .map_err(|e| e.to_string());
    }
    if s.len() == 7 && s.chars().all(|c| c.is_ascii_digit()) {
        // YYYYDDD
        let year: i16 = s[..4].parse().map_err(|e| format!("{e}"))?;
        let ordinal: i64 = s[4..].parse().map_err(|e| format!("{e}"))?;
        let d = jiff::civil::Date::new(year, 1, 1).map_err(|e| e.to_string())?;
        return d
            .checked_add(jiff::Span::new().days(ordinal - 1))
            .map_err(|e| e.to_string());
    }

    Err(format!("cannot parse date from '{s}'"))
}

pub(crate) fn compute_week_date_str(
    year: i64,
    week: i64,
    day_of_week: i64,
) -> Result<jiff::civil::Date, String> {
    // Find the Monday of week 1 of the given year using ISO 8601 algorithm
    let jan4 = jiff::civil::Date::new(year as i16, 1, 4).map_err(|e| e.to_string())?;
    let jan4_dow = jan4.weekday().to_monday_one_offset() as i64;
    let week1_monday_ordinal = 4 - jan4_dow + 1;
    let target_ordinal = week1_monday_ordinal + (week - 1) * 7 + (day_of_week - 1);
    let d = jiff::civil::Date::new(year as i16, 1, 1).map_err(|e| e.to_string())?;
    d.checked_add(jiff::Span::new().days(target_ordinal - 1))
        .map_err(|e| e.to_string())
}

pub(crate) fn parse_datetime_with_offset(s: &str) -> Result<jiff::Zoned, String> {
    // Try parsing as a Timestamp first (handles standard "2015-07-21T21:40:32+01:00" etc.)
    if let Ok(ts) = s.parse::<jiff::Timestamp>() {
        let tz = extract_offset_from_string(s).unwrap_or(jiff::tz::TimeZone::UTC);
        return Ok(ts.to_zoned(tz));
    }
    // Split at 'T' to handle date and time separately (supports week, ordinal, compact formats)
    let s_trim = s.trim();
    if let Some(t_idx) = s_trim.find(['T', 't']) {
        let date_str = &s_trim[..t_idx];
        let after_t = &s_trim[t_idx + 1..];

        let date = parse_date_str(date_str)?;

        // Check for IANA timezone name in brackets at end: e.g. "21:40:32.142[Europe/Stockholm]"
        // or with offset: "21:40:32.142+02:00[Europe/Stockholm]"
        let (time_str_base, tz) = if let Some(bracket_start) = after_t.rfind('[') {
            if let Some(bracket_end) = after_t.rfind(']') {
                if bracket_end > bracket_start {
                    let iana_name = &after_t[bracket_start + 1..bracket_end];
                    let tz = super::parse_timezone(iana_name).unwrap_or(jiff::tz::TimeZone::UTC);
                    // Strip the [IANA] part; the time_str may still have a numeric offset
                    (&after_t[..bracket_start], tz)
                } else {
                    (
                        after_t,
                        extract_offset_from_string(after_t).unwrap_or(jiff::tz::TimeZone::UTC),
                    )
                }
            } else {
                (
                    after_t,
                    extract_offset_from_string(after_t).unwrap_or(jiff::tz::TimeZone::UTC),
                )
            }
        } else {
            (
                after_t,
                extract_offset_from_string(after_t).unwrap_or(jiff::tz::TimeZone::UTC),
            )
        };

        let time_no_tz = strip_tz_from_time_str(time_str_base);
        let time_expanded = expand_compact_time(time_no_tz);
        let time = time_expanded
            .parse::<jiff::civil::Time>()
            .map_err(|e| format!("cannot parse time '{time_expanded}': {e}"))?;

        let dt = date.to_datetime(time);
        return dt.to_zoned(tz).map_err(|e| e.to_string());
    }
    // Last resort: try parsing as civil DateTime and assume UTC
    if let Ok(dt) = s.parse::<jiff::civil::DateTime>() {
        return dt
            .to_zoned(jiff::tz::TimeZone::UTC)
            .map_err(|e| e.to_string());
    }
    Err(format!("cannot parse datetime from '{s}'"))
}

pub(crate) fn strip_tz_from_time_str(s: &str) -> &str {
    if s.ends_with('Z') || s.ends_with('z') {
        return &s[..s.len() - 1];
    }
    // Look for +/- that starts the offset (not in the time digits)
    // The offset can be: +HH:MM, -HH:MM, +HHMM, -HHMM, +HH, -HH
    // We search from position 1 onward to skip a potential leading '-' sign issue
    // Start from position 1 to avoid matching a leading minus in negative times
    let bytes = s.as_bytes();
    for i in (1..s.len()).rev() {
        let b = bytes[i];
        if b == b'+' || b == b'-' {
            // Check that what follows looks like an offset
            let after = &s[i..];
            let rest = &after[1..]; // after the sign
                                    // Valid offset formats: HH, HHMM, HH:MM (2, 4, or 5 chars)
            let rest_stripped = rest.replace(':', "");
            if (rest_stripped.len() == 2 || rest_stripped.len() == 4)
                && rest_stripped.chars().all(|c| c.is_ascii_digit())
            {
                return &s[..i];
            }
        }
    }
    s
}

/// Extract a timezone offset from the tail of an ISO 8601 datetime string.
/// Looks for patterns like "+01:00", "-05:30", "+0100", "Z" at the end.
pub(crate) fn extract_offset_from_string(s: &str) -> Option<jiff::tz::TimeZone> {
    let s = s.trim();
    if s.ends_with('Z') || s.ends_with('z') {
        return Some(jiff::tz::TimeZone::UTC);
    }
    // Look for +/-HH:MM or +/-HHMM at the end
    // Find last '+' or '-' that could be offset (not in date part)
    // The offset should be within the last 6 chars
    let tail = if s.len() > 6 { &s[s.len() - 6..] } else { s };
    let offset_start = tail.rfind('+').or_else(|| tail.rfind('-'));
    if let Some(pos) = offset_start {
        let offset_str = &tail[pos..];
        if let Ok(tz) = super::parse_timezone(offset_str) {
            return Some(tz);
        }
    }
    None
}

/// Parse an ISO 8601 datetime string, handling week-date, ordinal-date, and compact formats.
pub(crate) fn parse_localdatetime_str(s: &str) -> Result<jiff::civil::DateTime, String> {
    if let Ok(dt) = s.parse::<jiff::civil::DateTime>() {
        return Ok(dt);
    }
    if let Ok(zdt) = s.parse::<jiff::Zoned>() {
        return Ok(jiff::civil::DateTime::from(&zdt));
    }
    let s = s.trim();
    let t_pos = s.find(['T', 't']);
    if let Some(t_idx) = t_pos {
        let date_str = &s[..t_idx];
        let rest = &s[t_idx + 1..];
        let date = parse_date_str(date_str)?;
        let time_str = rest.trim_end_matches('Z');
        let time_str = if let Some(plus_pos) = time_str.rfind('+') {
            if plus_pos > 0 {
                &time_str[..plus_pos]
            } else {
                time_str
            }
        } else if let Some(minus_pos) = time_str[1..].rfind('-') {
            &time_str[..minus_pos + 1]
        } else {
            time_str
        };
        let time_str = expand_compact_time(time_str);
        let time = time_str
            .parse::<jiff::civil::Time>()
            .map_err(|e| format!("cannot parse time '{time_str}': {e}"))?;
        Ok(jiff::civil::DateTime::from_parts(date, time))
    } else {
        Err(format!("cannot parse datetime from '{s}'"))
    }
}

/// Expand compact time notation `HHMMSS` to `HH:MM:SS`
pub(crate) fn expand_compact_time(s: &str) -> String {
    if s.len() >= 6 && !s.contains(':') {
        let hh = &s[0..2];
        let mm = &s[2..4];
        let ss = &s[4..];
        if let Some(dot_pos) = ss.find('.') {
            let (secs, frac) = ss.split_at(dot_pos);
            format!("{hh}:{mm}:{secs}{frac}")
        } else {
            format!("{hh}:{mm}:{ss}")
        }
    } else if s.len() == 4 && !s.contains(':') {
        let hh = &s[0..2];
        let mm = &s[2..4];
        format!("{hh}:{mm}:00")
    } else {
        s.to_string()
    }
}

/// Strip timezone offset from a time string like "21:40:32+01:00" → "21:40:32"
pub(crate) fn strip_tz_offset_from_time(s: &str) -> &str {
    // Strip trailing 'Z' (Zulu/UTC)
    if s.ends_with('Z') || s.ends_with('z') {
        return &s[..s.len() - 1];
    }
    // Find last '+' or '-' that separates time from offset
    // Time part is before this. Handle compact format too (e.g. "214032-0100")
    if let Some(pos) = s.rfind('+') {
        if pos > 0 {
            return &s[..pos];
        }
    }
    if let Some(pos) = s.rfind('-') {
        // Make sure the '-' is not at position 0 or part of the time itself
        // A time string like "21:40:32" doesn't have '-' after position 0
        if pos > 2 {
            return &s[..pos];
        }
    }
    s
}

/// Parse a duration string, supporting decimal ISO 8601 durations (e.g. P1.5Y, PT1.5H)
/// and cascading overflow (e.g. P12Y5M14DT16H12M70S → 70s cascades to 1m10s).
pub(crate) fn parse_duration_str(s: &str) -> Result<CypherDuration, GqlError> {
    // Return error for date-like strings (e.g., "2012-02-02")
    if !s.starts_with('P') && !s.starts_with('p') {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "ISO 8601 duration string starting with 'P'".to_string(),
            actual: s.to_string(),
        }));
    }
    // Try jiff first: handles calendar format (P2012-02-02T14:37:21.545) and basic forms.
    if let Ok(sp) = s.parse::<jiff::Span>() {
        let total_months = sp.get_years() as i64 * 12 + sp.get_months() as i64;
        let total_days = sp.get_weeks() as i64 * 7 + sp.get_days() as i64;
        let time_secs = sp.get_hours() as i64 * 3600 + sp.get_minutes() * 60 + sp.get_seconds();
        let ms = sp.get_milliseconds();
        let us = sp.get_microseconds();
        let ns_sub = sp.get_nanoseconds();
        let total_nanos = time_secs as i128 * 1_000_000_000
            + ms as i128 * 1_000_000
            + us as i128 * 1_000
            + ns_sub as i128;
        return Ok(CypherDuration {
            months: total_months,
            days: total_days,
            nanos: total_nanos,
        });
    }
    // Fall back to custom parser for decimal values and edge cases like 'PT'.
    parse_decimal_iso_duration(s)
}

/// Parse an ISO 8601 duration string that may contain decimal values.
///
/// Examples: `P1.5Y`, `PT1.5H`, `P1Y2M3.5D`.
/// Cascades fractional parts to the next smaller unit.
pub(crate) fn parse_decimal_iso_duration(s: &str) -> Result<CypherDuration, GqlError> {
    let s = s.trim();
    let s = s.strip_prefix('P').ok_or_else(|| {
        GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "ISO 8601 duration starting with 'P'".to_string(),
            actual: s.to_string(),
        })
    })?;

    // Split at 'T' to separate date and time components.
    let (date_str, time_str) = match s.find('T') {
        Some(pos) => (&s[..pos], &s[pos + 1..]),
        None => (s, ""),
    };

    // Handle P<YYYY-MM-DD>T<HH:MM:SS.sss> calendar datetime format.
    // Detect: date_str contains '-' and looks like a calendar date (YYYY-MM-DD).
    let is_calendar_date =
        date_str.contains('-') && !date_str.ends_with(|c: char| c.is_ascii_alphabetic());
    if is_calendar_date {
        // Parse YYYY-MM-DD as year/month/day components directly.
        let parts: Vec<&str> = date_str.splitn(3, '-').collect();
        if parts.len() == 3 {
            let years: f64 = parts[0].parse().unwrap_or(0.0);
            let months: f64 = parts[1].parse().unwrap_or(0.0);
            let days: f64 = parts[2].parse().unwrap_or(0.0);
            // Parse time part: HH:MM:SS.sss
            let (t_hours, t_mins, t_secs) = if !time_str.is_empty() {
                let t_parts: Vec<&str> = time_str.splitn(3, ':').collect();
                let h: f64 = t_parts.first().and_then(|s| s.parse().ok()).unwrap_or(0.0);
                let m: f64 = t_parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0.0);
                let s: f64 = t_parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0.0);
                (h, m, s)
            } else {
                (0.0, 0.0, 0.0)
            };
            let int_years = years as i64;
            let frac_years = years - int_years as f64;
            let total_month_float = frac_years * 12.0 + months;
            let int_months_raw = total_month_float as i64;
            let frac_months = total_month_float - int_months_raw as f64;
            let total_months = int_years * 12 + int_months_raw;
            let total_days_float = frac_months * (2_629_746.0 / 86_400.0) + days;
            let int_days = total_days_float.trunc() as i64;
            let frac_days = total_days_float - int_days as f64;
            let total_time_secs = frac_days * 86_400.0 + t_hours * 3_600.0 + t_mins * 60.0 + t_secs;
            let total_nanos = (total_time_secs * 1_000_000_000.0).round() as i128;
            return Ok(CypherDuration {
                months: total_months,
                days: int_days,
                nanos: total_nanos,
            });
        }
    }

    // Parse components of the form: <number><designator> where number may be decimal.
    fn parse_components(s: &str, designators: &[u8]) -> Result<Vec<(f64, char)>, GqlError> {
        let mut result = Vec::new();
        let mut remaining = s;
        while !remaining.is_empty() {
            let des_pos = remaining
                .bytes()
                .position(|b| designators.contains(&b))
                .ok_or_else(|| {
                    GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: format!(
                            "duration unit designator (one of {})",
                            designators.iter().map(|&b| b as char).collect::<String>()
                        ),
                        actual: remaining.to_string(),
                    })
                })?;
            let num_str = &remaining[..des_pos];
            let designator = remaining.as_bytes()[des_pos] as char;
            let value: f64 = num_str.parse().map_err(|_| {
                GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "numeric duration value".to_string(),
                    actual: num_str.to_string(),
                })
            })?;
            result.push((value, designator));
            remaining = &remaining[des_pos + 1..];
        }
        Ok(result)
    }

    let date_comps = if date_str.is_empty() {
        vec![]
    } else {
        parse_components(date_str, b"YMWD")?
    };
    let time_comps = if time_str.is_empty() {
        vec![]
    } else {
        parse_components(time_str, b"HMS")?
    };

    let mut years = 0.0f64;
    let mut months = 0.0f64;
    let mut weeks = 0.0f64;
    let mut days = 0.0f64;
    for (v, d) in date_comps {
        match d {
            'Y' => years += v,
            'M' => months += v,
            'W' => weeks += v,
            'D' => days += v,
            _ => {}
        }
    }

    let mut hours = 0.0f64;
    let mut minutes = 0.0f64;
    let mut seconds = 0.0f64;
    for (v, d) in time_comps {
        match d {
            'H' => hours += v,
            'M' => minutes += v,
            'S' => seconds += v,
            _ => {}
        }
    }

    // Phase 1: years + months → total months
    let int_years = years as i64;
    let frac_years = years - int_years as f64;
    let total_month_float = frac_years * 12.0 + months;
    let int_months_raw = total_month_float as i64;
    let frac_months = total_month_float - int_months_raw as f64;
    let total_months = int_years * 12 + int_months_raw;

    // Phase 2: fractional months + weeks + days → total days
    let total_days_float = frac_months * (2_629_746.0 / 86_400.0) + weeks * 7.0 + days;
    let int_days = total_days_float as i64;
    let frac_days = total_days_float - int_days as f64;

    // Phase 3: fractional days + hours + minutes + seconds → nanos
    let total_time_secs = frac_days * 86_400.0 + hours * 3_600.0 + minutes * 60.0 + seconds;
    let total_nanos = (total_time_secs * 1_000_000_000.0).round() as i128;

    Ok(CypherDuration {
        months: total_months,
        days: int_days,
        nanos: total_nanos,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse_standard_date() {
        let d = parse_date_str("2024-03-15").unwrap();
        assert_eq!(d.year(), 2024);
        assert_eq!(d.day(), 15);
    }
    #[test]
    fn parse_year_only() {
        assert_eq!(parse_date_str("2024").unwrap().month(), 1);
    }
    #[test]
    fn parse_year_month() {
        assert_eq!(parse_date_str("2024-03").unwrap().month(), 3);
    }
    #[test]
    fn parse_iso_week() {
        assert_eq!(parse_date_str("2015-W30-2").unwrap().day(), 21);
    }
    #[test]
    fn parse_ordinal() {
        assert_eq!(parse_date_str("2015-202").unwrap().day(), 21);
    }
    #[test]
    fn parse_date_invalid() {
        assert!(parse_date_str("not-a-date").is_err());
    }
    #[test]
    fn expand_hhmmss() {
        assert_eq!(expand_compact_time("214032"), "21:40:32");
    }
    #[test]
    fn expand_hhmm() {
        assert_eq!(expand_compact_time("2140"), "21:40:00");
    }
    #[test]
    fn expand_frac() {
        assert_eq!(expand_compact_time("214032.142"), "21:40:32.142");
    }
    #[test]
    fn strip_tz_z() {
        assert_eq!(strip_tz_from_time_str("21:40:32Z"), "21:40:32");
    }
    #[test]
    fn extract_z() {
        assert_eq!(
            extract_offset_from_string("T12:00:00Z").unwrap(),
            jiff::tz::TimeZone::UTC
        );
    }
    #[test]
    fn extract_none() {
        assert!(extract_offset_from_string("T12:00:00").is_none());
    }
    #[test]
    fn parse_dt() {
        assert_eq!(
            parse_datetime_with_offset("2015-07-21T21:40:32+01:00")
                .unwrap()
                .year(),
            2015
        );
    }
    #[test]
    fn parse_dt_invalid() {
        assert!(parse_datetime_with_offset("nope").is_err());
    }
    #[test]
    fn parse_ldt() {
        assert_eq!(
            parse_localdatetime_str("2024-03-15T10:30:00")
                .unwrap()
                .year(),
            2024
        );
    }
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
    fn dur_simple() {
        let d = parse_duration_str("P1Y2M3D").unwrap();
        assert_eq!(d.months, 14);
        assert_eq!(d.days, 3);
    }
    #[test]
    fn dur_time() {
        assert_eq!(
            parse_duration_str("PT1H30M").unwrap().nanos,
            5400i128 * 1_000_000_000
        );
    }
    #[test]
    fn dur_decimal() {
        assert_eq!(parse_duration_str("P1.5Y").unwrap().months, 18);
    }
    #[test]
    fn dur_invalid() {
        assert!(parse_duration_str("2012-02-02").is_err());
    }
    #[test]
    fn strip_offset_z() {
        assert_eq!(strip_tz_offset_from_time("21:40:32Z"), "21:40:32");
    }
    #[test]
    fn strip_offset_plus() {
        assert_eq!(strip_tz_offset_from_time("21:40:32+01:00"), "21:40:32");
    }
    #[test]
    fn week_date() {
        assert_eq!(compute_week_date_str(2015, 30, 1).unwrap().day(), 20);
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
