//! ISO 8601 duration parsing, including decimal-value cascading and calendar format.

use crate::error::{ExecutionError, GqlError};
use crate::types::CypherDuration;

/// Parse a duration string, supporting decimal ISO 8601 durations (e.g. P1.5Y, PT1.5H)
/// and cascading overflow (e.g. P12Y5M14DT16H12M70S → 70s cascades to 1m10s).
pub(crate) fn parse_duration_str(s: &str) -> Result<CypherDuration, GqlError> {
    if !s.starts_with('P') && !s.starts_with('p') {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "ISO 8601 duration string starting with 'P'".to_string(),
            actual: s.to_string(),
        }));
    }
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

    let (date_str, time_str) = match s.find('T') {
        Some(pos) => (&s[..pos], &s[pos + 1..]),
        None => (s, ""),
    };

    let is_calendar_date =
        date_str.contains('-') && !date_str.ends_with(|c: char| c.is_ascii_alphabetic());
    if is_calendar_date {
        let parts: Vec<&str> = date_str.splitn(3, '-').collect();
        if parts.len() == 3 {
            let parse_cal = |s: &str, name: &str| -> Result<f64, GqlError> {
                s.parse::<f64>().map_err(|_| {
                    GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: format!("numeric {name} value in calendar-format duration"),
                        actual: s.to_string(),
                    })
                })
            };
            let years: f64 = parse_cal(parts[0], "years")?;
            let months: f64 = parse_cal(parts[1], "months")?;
            let days: f64 = parse_cal(parts[2], "days")?;
            let (t_hours, t_mins, t_secs) = if !time_str.is_empty() {
                let t_parts: Vec<&str> = time_str.splitn(3, ':').collect();
                let h: f64 = t_parts
                    .first()
                    .map(|s| parse_cal(s, "hours"))
                    .transpose()?
                    .unwrap_or(0.0);
                let m: f64 = t_parts
                    .get(1)
                    .map(|s| parse_cal(s, "minutes"))
                    .transpose()?
                    .unwrap_or(0.0);
                let s: f64 = t_parts
                    .get(2)
                    .map(|s| parse_cal(s, "seconds"))
                    .transpose()?
                    .unwrap_or(0.0);
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
            _ => {
                tracing::debug!("unknown duration designator: {}", d);
            }
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
            _ => {
                tracing::debug!("unknown duration designator: {}", d);
            }
        }
    }

    let int_years = years as i64;
    let frac_years = years - int_years as f64;
    let total_month_float = frac_years * 12.0 + months;
    let int_months_raw = total_month_float as i64;
    let frac_months = total_month_float - int_months_raw as f64;
    let total_months = int_years * 12 + int_months_raw;

    let total_days_float = frac_months * (2_629_746.0 / 86_400.0) + weeks * 7.0 + days;
    let int_days = total_days_float as i64;
    let frac_days = total_days_float - int_days as f64;

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
    fn calendar_duration_valid() {
        let d = parse_decimal_iso_duration("P2012-02-02T14:37:21.545").unwrap();
        assert_eq!(d.months, 2012 * 12 + 2);
        assert_eq!(d.days, 2);
        let expected_nanos: f64 = (14.0 * 3_600.0 + 37.0 * 60.0 + 21.545) * 1_000_000_000.0;
        assert_eq!(d.nanos, expected_nanos.round() as i128);
    }

    #[test]
    fn calendar_duration_invalid_year_component() {
        let result = parse_decimal_iso_duration("P -02-02T14:37:21");
        assert!(result.is_err(), "non-numeric year should be rejected");
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("years"),
            "error should mention years, got: {err_msg}"
        );
    }

    #[test]
    fn calendar_duration_invalid_month_component() {
        let result = parse_decimal_iso_duration("P2012-!!-02T14:37:21");
        assert!(result.is_err(), "non-numeric month should be rejected");
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("months"),
            "error should mention months, got: {err_msg}"
        );
    }

    #[test]
    fn calendar_duration_invalid_day_component() {
        let result = parse_decimal_iso_duration("P2012-02-??T14:37:21");
        assert!(result.is_err(), "non-numeric day should be rejected");
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("days"),
            "error should mention days, got: {err_msg}"
        );
    }

    #[test]
    fn calendar_duration_invalid_time_hours() {
        let result = parse_decimal_iso_duration("P2012-02-02T!!:37:21");
        assert!(result.is_err(), "non-numeric hours should be rejected");
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("hours"),
            "error should mention hours, got: {err_msg}"
        );
    }

    #[test]
    fn calendar_duration_invalid_time_minutes() {
        let result = parse_decimal_iso_duration("P2012-02-02T14:##:21");
        assert!(result.is_err(), "non-numeric minutes should be rejected");
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("minutes"),
            "error should mention minutes, got: {err_msg}"
        );
    }

    #[test]
    fn calendar_duration_invalid_time_seconds() {
        let result = parse_decimal_iso_duration("P2012-02-02T14:37:$$");
        assert!(result.is_err(), "non-numeric seconds should be rejected");
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("seconds"),
            "error should mention seconds, got: {err_msg}"
        );
    }
}
