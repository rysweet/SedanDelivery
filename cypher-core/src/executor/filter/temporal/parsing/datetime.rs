//! ISO 8601 datetime parsing with optional offset/timezone handling.

use super::date::parse_date_str;
use super::time::{expand_compact_time, strip_tz_from_time_str};
use super::timezone::extract_offset_from_string;

pub(crate) fn parse_datetime_with_offset(s: &str) -> Result<jiff::Zoned, String> {
    if let Ok(ts) = s.parse::<jiff::Timestamp>() {
        let tz = extract_offset_from_string(s).unwrap_or(jiff::tz::TimeZone::UTC);
        return Ok(ts.to_zoned(tz));
    }
    let s_trim = s.trim();
    if let Some(t_idx) = s_trim.find(['T', 't']) {
        let date_str = &s_trim[..t_idx];
        let after_t = &s_trim[t_idx + 1..];

        let date = parse_date_str(date_str)?;

        let (time_str_base, tz) = if let Some(bracket_start) = after_t.rfind('[') {
            if let Some(bracket_end) = after_t.rfind(']') {
                if bracket_end > bracket_start {
                    let iana_name = &after_t[bracket_start + 1..bracket_end];
                    let tz =
                        super::super::parse_timezone(iana_name).unwrap_or(jiff::tz::TimeZone::UTC);
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
    if let Ok(dt) = s.parse::<jiff::civil::DateTime>() {
        return dt
            .to_zoned(jiff::tz::TimeZone::UTC)
            .map_err(|e| e.to_string());
    }
    Err(format!("cannot parse datetime from '{s}'"))
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
