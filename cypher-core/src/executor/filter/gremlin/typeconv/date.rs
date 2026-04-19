//! `asDate` conversion (epoch-millis → DateTime).

use crate::error::GqlError;
use crate::types::Value;

pub(crate) fn asdate(vals: Vec<Value>) -> Result<Value, GqlError> {
    // asDate() — convert epoch-millis integer to Date value
    match vals.first() {
        Some(Value::Int(millis)) => {
            let secs = millis / 1000;
            let nanos = ((millis % 1000) * 1_000_000) as i32;
            let ts = jiff::Timestamp::new(secs, nanos).unwrap_or(jiff::Timestamp::UNIX_EPOCH);
            let dt = ts.to_zoned(jiff::tz::TimeZone::UTC);
            Ok(Value::DateTime(Box::new(dt)))
        }
        Some(Value::Float(f)) => {
            let millis = *f as i64;
            let secs = millis / 1000;
            let nanos = ((millis % 1000) * 1_000_000) as i32;
            let ts = jiff::Timestamp::new(secs, nanos).unwrap_or(jiff::Timestamp::UNIX_EPOCH);
            let dt = ts.to_zoned(jiff::tz::TimeZone::UTC);
            Ok(Value::DateTime(Box::new(dt)))
        }
        Some(Value::DateTime(d)) => Ok(Value::DateTime(d.clone())),
        Some(Value::Null) => Ok(Value::Null),
        _ => Ok(Value::Null),
    }
}
