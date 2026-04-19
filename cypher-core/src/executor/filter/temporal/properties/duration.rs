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
