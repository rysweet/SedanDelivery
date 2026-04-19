//! ISO 8601 date string parsing including week-date and ordinal-date formats.

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
    if let Ok(d) = s.parse::<jiff::civil::Date>() {
        return Ok(d);
    }

    let s = s.trim();

    if let Some(week_part) = s
        .strip_prefix(|c: char| c.is_ascii_digit())
        .and(None::<&str>)
        .or_else(|| {
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
        let (year_str, rest) = s.split_at(4);
        let year: i64 = year_str.parse().map_err(|e| format!("{e}"))?;
        let rest = rest.strip_prefix('-').unwrap_or(rest);
        if rest.starts_with('W') {
            let rest = rest.strip_prefix('W').unwrap_or(rest);
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

    if s.len() == 4 && s.chars().all(|c| c.is_ascii_digit()) {
        let year: i16 = s.parse().map_err(|e| format!("{e}"))?;
        return jiff::civil::Date::new(year, 1, 1).map_err(|e| e.to_string());
    }

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

    if s.len() == 7 && s.as_bytes()[4] == b'-' && s[5..].chars().all(|c| c.is_ascii_digit()) {
        let year: i16 = s[..4].parse().map_err(|e| format!("{e}"))?;
        let month: i8 = s[5..].parse().map_err(|e| format!("{e}"))?;
        return jiff::civil::Date::new(year, month, 1).map_err(|e| e.to_string());
    }
    if s.len() == 6 && s.chars().all(|c| c.is_ascii_digit()) {
        let year: i16 = s[..4].parse().map_err(|e| format!("{e}"))?;
        let month: i8 = s[4..].parse().map_err(|e| format!("{e}"))?;
        return jiff::civil::Date::new(year, month, 1).map_err(|e| e.to_string());
    }

    if s.len() == 8 && s.as_bytes()[4] == b'-' && s[5..].chars().all(|c| c.is_ascii_digit()) {
        let year: i16 = s[..4].parse().map_err(|e| format!("{e}"))?;
        let ordinal: i64 = s[5..].parse().map_err(|e| format!("{e}"))?;
        let d = jiff::civil::Date::new(year, 1, 1).map_err(|e| e.to_string())?;
        return d
            .checked_add(jiff::Span::new().days(ordinal - 1))
            .map_err(|e| e.to_string());
    }
    if s.len() == 7 && s.chars().all(|c| c.is_ascii_digit()) {
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
    let jan4 = jiff::civil::Date::new(year as i16, 1, 4).map_err(|e| e.to_string())?;
    let jan4_dow = jan4.weekday().to_monday_one_offset() as i64;
    let week1_monday_ordinal = 4 - jan4_dow + 1;
    let target_ordinal = week1_monday_ordinal + (week - 1) * 7 + (day_of_week - 1);
    let d = jiff::civil::Date::new(year as i16, 1, 1).map_err(|e| e.to_string())?;
    d.checked_add(jiff::Span::new().days(target_ordinal - 1))
        .map_err(|e| e.to_string())
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
    fn week_date() {
        assert_eq!(compute_week_date_str(2015, 30, 1).unwrap().day(), 20);
    }
}
