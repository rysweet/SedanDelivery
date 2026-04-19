//! Property access for `Value::LocalDateTime`.

use crate::error::GqlError;
use crate::types::Value;

use super::super::temporal_parse_error;

pub(super) fn local_datetime_property(
    dt: &jiff::civil::DateTime,
    key: &str,
) -> Result<Value, GqlError> {
    match key {
        "year" => Ok(Value::Int(dt.year() as i64)),
        "month" => Ok(Value::Int(dt.month() as i64)),
        "day" => Ok(Value::Int(dt.day() as i64)),
        "hour" => Ok(Value::Int(dt.hour() as i64)),
        "minute" => Ok(Value::Int(dt.minute() as i64)),
        "second" => Ok(Value::Int(dt.second() as i64)),
        "millisecond" => Ok(Value::Int((dt.subsec_nanosecond() / 1_000_000) as i64)),
        "microsecond" => Ok(Value::Int((dt.subsec_nanosecond() / 1_000) as i64)),
        "nanosecond" => Ok(Value::Int(dt.subsec_nanosecond() as i64)),
        "week" => {
            let d = dt.date();
            let iso = d.iso_week_date();
            Ok(Value::Int(iso.week() as i64))
        }
        "weekYear" => {
            let iso = dt.date().iso_week_date();
            Ok(Value::Int(iso.year() as i64))
        }
        "dayOfWeek" | "weekDay" => {
            Ok(Value::Int(dt.date().weekday().to_monday_one_offset() as i64))
        }
        "dayOfYear" | "ordinalDay" => Ok(Value::Int(dt.date().day_of_year() as i64)),
        "quarter" => Ok(Value::Int(((dt.month() as i64 - 1) / 3) + 1)),
        "dayOfQuarter" => {
            let d = dt.date();
            let q0 = ((d.month() as i64 - 1) / 3) as i32;
            let q_start = jiff::civil::Date::new(d.year(), (q0 * 3 + 1) as i8, 1)
                .map_err(|e| temporal_parse_error("localdatetime.dayOfQuarter", e))?;
            let diff = q_start
                .until(jiff::civil::DateDifference::new(d).largest(jiff::Unit::Day))
                .map_err(|e| temporal_parse_error("localdatetime.dayOfQuarter", e))?;
            Ok(Value::Int(diff.get_days() as i64 + 1))
        }
        _ => Ok(Value::Null),
    }
}
