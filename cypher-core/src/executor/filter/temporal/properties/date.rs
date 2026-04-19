//! Property access for `Value::Date`.

use crate::error::GqlError;
use crate::types::Value;

use super::super::temporal_parse_error;

pub(super) fn date_property(d: &jiff::civil::Date, key: &str) -> Result<Value, GqlError> {
    match key {
        "year" => Ok(Value::Int(d.year() as i64)),
        "month" => Ok(Value::Int(d.month() as i64)),
        "day" => Ok(Value::Int(d.day() as i64)),
        "week" => {
            let iso = d.iso_week_date();
            Ok(Value::Int(iso.week() as i64))
        }
        "weekYear" => {
            let iso = d.iso_week_date();
            Ok(Value::Int(iso.year() as i64))
        }
        "dayOfWeek" | "weekDay" => Ok(Value::Int(d.weekday().to_monday_one_offset() as i64)),
        "dayOfYear" => Ok(Value::Int(d.day_of_year() as i64)),
        "quarter" => Ok(Value::Int(((d.month() as i64 - 1) / 3) + 1)),
        "ordinalDay" => Ok(Value::Int(d.day_of_year() as i64)),
        "dayOfQuarter" => {
            let q0 = ((d.month() as i64 - 1) / 3) as i32;
            let q_start = jiff::civil::Date::new(d.year(), (q0 * 3 + 1) as i8, 1)
                .map_err(|e| temporal_parse_error("date.dayOfQuarter", e))?;
            let diff = q_start
                .until(jiff::civil::DateDifference::new(*d).largest(jiff::Unit::Day))
                .map_err(|e| temporal_parse_error("date.dayOfQuarter", e))?;
            Ok(Value::Int(diff.get_days() as i64 + 1))
        }
        _ => Ok(Value::Null),
    }
}

#[cfg(test)]
mod tests {
    use super::super::temporal_property_access;
    use crate::types::Value;

    fn date(y: i16, m: i8, d: i8) -> Value {
        Value::Date(jiff::civil::Date::new(y, m, d).unwrap())
    }

    #[test]
    fn date_props() {
        let d = date(2024, 3, 15);
        assert_eq!(
            temporal_property_access(&d, "year").unwrap(),
            Value::Int(2024)
        );
        assert_eq!(temporal_property_access(&d, "day").unwrap(), Value::Int(15));
    }

    #[test]
    fn date_quarter() {
        assert_eq!(
            temporal_property_access(&date(2024, 4, 15), "quarter").unwrap(),
            Value::Int(2)
        );
    }

    #[test]
    fn date_unknown() {
        assert_eq!(
            temporal_property_access(&date(2024, 1, 1), "nope").unwrap(),
            Value::Null
        );
    }

    #[test]
    fn date_all_keys() {
        let d = date(2024, 4, 15);
        for k in [
            "year",
            "month",
            "day",
            "week",
            "weekYear",
            "dayOfWeek",
            "weekDay",
            "dayOfYear",
            "ordinalDay",
            "quarter",
            "dayOfQuarter",
        ] {
            assert!(matches!(
                temporal_property_access(&d, k).unwrap(),
                Value::Int(_)
            ));
        }
    }
}
