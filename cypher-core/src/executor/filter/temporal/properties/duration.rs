//! Property access for `Value::Duration`.

use crate::error::{ExecutionError, GqlError};
use crate::types::{CypherDuration, Value};

pub(super) fn duration_property(d: &CypherDuration, key: &str) -> Result<Value, GqlError> {
    match key {
        "years" => Ok(Value::Int(d.months.div_euclid(12))),
        "months" => Ok(Value::Int(d.months)),
        "quarters" => Ok(Value::Int(d.months.div_euclid(3))),
        "quartersOfYear" => Ok(Value::Int(d.months.div_euclid(3).rem_euclid(4))),
        "monthsOfYear" => Ok(Value::Int(d.months.rem_euclid(12))),
        "monthsOfQuarter" => Ok(Value::Int(d.months.rem_euclid(3))),
        "weeks" => Ok(Value::Int(d.days.div_euclid(7))),
        "days" => Ok(Value::Int(d.days)),
        "daysOfWeek" => Ok(Value::Int(d.days.rem_euclid(7))),
        "hours" => Ok(Value::Int(d.nanos.div_euclid(3_600_000_000_000) as i64)),
        "minutes" => Ok(Value::Int(d.nanos.div_euclid(60_000_000_000) as i64)),
        "minutesOfHour" => Ok(Value::Int(
            (d.nanos.div_euclid(60_000_000_000).rem_euclid(60)) as i64,
        )),
        "seconds" => Ok(Value::Int(d.nanos.div_euclid(1_000_000_000) as i64)),
        "secondsOfMinute" => Ok(Value::Int(
            (d.nanos.div_euclid(1_000_000_000).rem_euclid(60)) as i64,
        )),
        "milliseconds" => Ok(Value::Int(d.nanos.div_euclid(1_000_000) as i64)),
        "millisecondsOfSecond" => Ok(Value::Int(
            (d.nanos.rem_euclid(1_000_000_000) / 1_000_000) as i64,
        )),
        "microseconds" => Ok(Value::Int(d.nanos.div_euclid(1_000) as i64)),
        "microsecondsOfSecond" => Ok(Value::Int(
            (d.nanos.rem_euclid(1_000_000_000) / 1_000) as i64,
        )),
        "nanoseconds" => {
            let nanos_i64 = i64::try_from(d.nanos).map_err(|_| {
                GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "duration nanoseconds within i64 range".to_string(),
                    actual: format!("nanoseconds {} overflow i64 range", d.nanos),
                })
            })?;
            Ok(Value::Int(nanos_i64))
        }
        "nanosecondsOfSecond" => Ok(Value::Int(d.nanos.rem_euclid(1_000_000_000) as i64)),
        _ => Ok(Value::Null),
    }
}

#[cfg(test)]
mod tests {
    use super::super::temporal_property_access;
    use crate::types::{CypherDuration, Value};

    #[test]
    fn dur_props() {
        let d = Value::Duration(CypherDuration {
            months: 27,
            days: 17,
            nanos: 3_723_456_789_012,
        });
        assert_eq!(
            temporal_property_access(&d, "years").unwrap(),
            Value::Int(2)
        );
        assert_eq!(
            temporal_property_access(&d, "weeks").unwrap(),
            Value::Int(2)
        );
    }

    #[test]
    fn duration_all_keys() {
        let d = Value::Duration(CypherDuration {
            months: 27,
            days: 17,
            nanos: 3_723_456_789_012,
        });
        for k in [
            "years",
            "months",
            "quarters",
            "quartersOfYear",
            "monthsOfYear",
            "monthsOfQuarter",
            "weeks",
            "days",
            "daysOfWeek",
            "hours",
            "minutes",
            "minutesOfHour",
            "seconds",
            "secondsOfMinute",
            "milliseconds",
            "millisecondsOfSecond",
            "microseconds",
            "microsecondsOfSecond",
            "nanoseconds",
            "nanosecondsOfSecond",
        ] {
            assert!(matches!(
                temporal_property_access(&d, k).unwrap(),
                Value::Int(_)
            ));
        }
        assert_eq!(temporal_property_access(&d, "nope").unwrap(), Value::Null);
    }

    #[test]
    fn duration_nanos_overflow_errs() {
        let d = Value::Duration(CypherDuration {
            months: 0,
            days: 0,
            nanos: i128::MAX,
        });
        assert!(temporal_property_access(&d, "nanoseconds").is_err());
    }
}
