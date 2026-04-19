//! Property access for `Value::DateTime` (zoned datetime).

use crate::error::GqlError;
use crate::types::Value;

use super::super::temporal_parse_error;

pub(super) fn datetime_property(zdt: &jiff::Zoned, key: &str) -> Result<Value, GqlError> {
    match key {
        "year" => Ok(Value::Int(zdt.year() as i64)),
        "month" => Ok(Value::Int(zdt.month() as i64)),
        "day" => Ok(Value::Int(zdt.day() as i64)),
        "hour" => Ok(Value::Int(zdt.hour() as i64)),
        "minute" => Ok(Value::Int(zdt.minute() as i64)),
        "second" => Ok(Value::Int(zdt.second() as i64)),
        "millisecond" => Ok(Value::Int((zdt.subsec_nanosecond() / 1_000_000) as i64)),
        "microsecond" => Ok(Value::Int((zdt.subsec_nanosecond() / 1_000) as i64)),
        "nanosecond" => Ok(Value::Int(zdt.subsec_nanosecond() as i64)),
        "timezone" => Ok(Value::Str(timezone_label(zdt).into_boxed_str())),
        "offset" => Ok(Value::Str(offset_label(zdt).into_boxed_str())),
        "offsetMinutes" => {
            let off = zdt.offset();
            Ok(Value::Int((off.seconds() / 60) as i64))
        }
        "offsetSeconds" => {
            let off = zdt.offset();
            Ok(Value::Int(off.seconds() as i64))
        }
        "week" => {
            let d = jiff::civil::Date::from(zdt);
            let iso = d.iso_week_date();
            Ok(Value::Int(iso.week() as i64))
        }
        "weekYear" => {
            let d = jiff::civil::Date::from(zdt);
            let iso = d.iso_week_date();
            Ok(Value::Int(iso.year() as i64))
        }
        "dayOfWeek" | "weekDay" => {
            let d = jiff::civil::Date::from(zdt);
            Ok(Value::Int(d.weekday().to_monday_one_offset() as i64))
        }
        "dayOfYear" | "ordinalDay" => {
            let d = jiff::civil::Date::from(zdt);
            Ok(Value::Int(d.day_of_year() as i64))
        }
        "quarter" => Ok(Value::Int(((zdt.month() as i64 - 1) / 3) + 1)),
        "dayOfQuarter" => {
            let d = jiff::civil::Date::from(zdt);
            let q0 = ((d.month() as i64 - 1) / 3) as i32;
            let q_start = jiff::civil::Date::new(d.year(), (q0 * 3 + 1) as i8, 1)
                .map_err(|e| temporal_parse_error("datetime.dayOfQuarter", e))?;
            let diff = q_start
                .until(jiff::civil::DateDifference::new(d).largest(jiff::Unit::Day))
                .map_err(|e| temporal_parse_error("datetime.dayOfQuarter", e))?;
            Ok(Value::Int(diff.get_days() as i64 + 1))
        }
        "epochMillis" => {
            let ts = zdt.timestamp();
            Ok(Value::Int(ts.as_millisecond()))
        }
        "epochSeconds" => {
            let ts = zdt.timestamp();
            Ok(Value::Int(ts.as_second()))
        }
        _ => Ok(Value::Null),
    }
}

fn offset_label(zdt: &jiff::Zoned) -> String {
    let off = zdt.offset();
    let total_secs = off.seconds();
    let h = total_secs / 3600;
    let m = (total_secs.abs() % 3600) / 60;
    format!("{:+03}:{:02}", h, m)
}

fn timezone_label(zdt: &jiff::Zoned) -> String {
    if let Some(iana) = zdt.time_zone().iana_name() {
        // The IANA "Factory" timezone is valid in jiff but not in
        // common timezone libraries (pytz). Treat it as a fixed
        // offset to avoid interop issues.
        if iana == "Factory" {
            offset_label(zdt)
        } else {
            iana.to_string()
        }
    } else {
        offset_label(zdt)
    }
}
