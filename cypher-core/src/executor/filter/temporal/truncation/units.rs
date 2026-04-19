//! Per-unit truncation logic for date and time components.

use crate::error::GqlError;

use super::super::temporal_parse_error;

/// Truncate a date to the given unit, returning (year, month, day).
pub(super) fn truncate_date_fields(
    d: jiff::civil::Date,
    unit: &str,
) -> Result<jiff::civil::Date, GqlError> {
    let y = d.year() as i64;
    match unit {
        "millennium" => {
            let ty = (y / 1000) * 1000;
            jiff::civil::Date::new(ty as i16, 1, 1).map_err(|e| temporal_parse_error("truncate", e))
        }
        "century" => {
            let ty = (y / 100) * 100;
            jiff::civil::Date::new(ty as i16, 1, 1).map_err(|e| temporal_parse_error("truncate", e))
        }
        "decade" => {
            let ty = (y / 10) * 10;
            jiff::civil::Date::new(ty as i16, 1, 1).map_err(|e| temporal_parse_error("truncate", e))
        }
        "year" => {
            jiff::civil::Date::new(d.year(), 1, 1).map_err(|e| temporal_parse_error("truncate", e))
        }
        "weekyear" => {
            // ISO week year: find the Monday of week 1 of the ISO week year.
            let iso = d.iso_week_date();
            let iso_year = iso.year();
            // Jan 4 is always in week 1 of the ISO year.
            let jan4 = jiff::civil::Date::new(iso_year, 1, 4)
                .map_err(|e| temporal_parse_error("truncate", e))?;
            let jan4_dow = jan4.weekday().to_monday_one_offset() as i64;
            // Monday of week 1
            let monday_ordinal = 4 - jan4_dow + 1;
            let base = jiff::civil::Date::new(iso_year, 1, 1)
                .map_err(|e| temporal_parse_error("truncate", e))?;
            base.checked_add(jiff::Span::new().days(monday_ordinal - 1))
                .map_err(|e| temporal_parse_error("truncate", e))
        }
        "quarter" => {
            let q = ((d.month() as i64 - 1) / 3) * 3 + 1;
            jiff::civil::Date::new(d.year(), q as i8, 1)
                .map_err(|e| temporal_parse_error("truncate", e))
        }
        "month" => jiff::civil::Date::new(d.year(), d.month(), 1)
            .map_err(|e| temporal_parse_error("truncate", e)),
        "week" => {
            // Monday of the current week (ISO weeks start on Monday)
            let dow = d.weekday().to_monday_one_offset() as i64; // 1=Mon..7=Sun
            d.checked_add(jiff::Span::new().days(1 - dow))
                .map_err(|e| temporal_parse_error("truncate", e))
        }
        "day" | "hour" | "minute" | "second" | "millisecond" | "microsecond" => Ok(d),
        other => Err(temporal_parse_error(
            "truncate",
            format!("unknown unit '{other}'"),
        )),
    }
}

/// Truncate a time to the given unit.
pub(super) fn truncate_time_fields(
    t: jiff::civil::Time,
    unit: &str,
) -> Result<jiff::civil::Time, GqlError> {
    match unit {
        "day" | "millennium" | "century" | "decade" | "year" | "weekyear" | "quarter" | "month"
        | "week" => Ok(jiff::civil::Time::midnight()),
        "hour" => jiff::civil::Time::new(t.hour(), 0, 0, 0)
            .map_err(|e| temporal_parse_error("truncate", e)),
        "minute" => jiff::civil::Time::new(t.hour(), t.minute(), 0, 0)
            .map_err(|e| temporal_parse_error("truncate", e)),
        "second" => jiff::civil::Time::new(t.hour(), t.minute(), t.second(), 0)
            .map_err(|e| temporal_parse_error("truncate", e)),
        "millisecond" => {
            let ns = (t.subsec_nanosecond() / 1_000_000) * 1_000_000;
            jiff::civil::Time::new(t.hour(), t.minute(), t.second(), ns)
                .map_err(|e| temporal_parse_error("truncate", e))
        }
        "microsecond" => {
            let ns = (t.subsec_nanosecond() / 1_000) * 1_000;
            jiff::civil::Time::new(t.hour(), t.minute(), t.second(), ns)
                .map_err(|e| temporal_parse_error("truncate", e))
        }
        other => Err(temporal_parse_error(
            "truncate",
            format!("unknown unit '{other}'"),
        )),
    }
}
