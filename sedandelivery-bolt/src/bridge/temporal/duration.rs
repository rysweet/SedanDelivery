//! Bolt Duration structure decoder.

use cypher_core::types::value::CypherDuration;
use cypher_core::types::Value;

use super::bolt_int;
use crate::packstream::value::BoltValue;

/// Duration (tag 0x45): 4 fields -- months, days, seconds, nanoseconds.
/// All four fields are required; a missing field is an error, not silently zero.
pub(crate) fn decode_duration(fields: &[BoltValue]) -> Value {
    let months = match fields.first().and_then(bolt_int) {
        Some(v) => v,
        None => {
            tracing::error!("bolt_decode: missing or non-integer 'months' field in Duration structure, fields={fields:?}");
            return Value::Null;
        }
    };
    let days = match fields.get(1).and_then(bolt_int) {
        Some(v) => v,
        None => {
            tracing::error!("bolt_decode: missing or non-integer 'days' field in Duration structure, fields={fields:?}");
            return Value::Null;
        }
    };
    let seconds = match fields.get(2).and_then(bolt_int) {
        Some(v) => v,
        None => {
            tracing::error!("bolt_decode: missing or non-integer 'seconds' field in Duration structure, fields={fields:?}");
            return Value::Null;
        }
    };
    let nanos = match fields.get(3).and_then(bolt_int) {
        Some(v) => v,
        None => {
            tracing::error!("bolt_decode: missing or non-integer 'nanoseconds' field in Duration structure, fields={fields:?}");
            return Value::Null;
        }
    };

    Value::Duration(CypherDuration {
        months,
        days,
        nanos: (seconds as i128) * 1_000_000_000 + (nanos as i128),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_duration_from_bolt_structure() {
        let val = decode_duration(&[
            BoltValue::Int(14),
            BoltValue::Int(3),
            BoltValue::Int(100),
            BoltValue::Int(0),
        ]);
        let expected = CypherDuration {
            months: 14,
            days: 3,
            nanos: 100_000_000_000,
        };
        assert_eq!(val, Value::Duration(expected));
    }

    #[test]
    fn decode_duration_with_sub_second_nanos() {
        let val = decode_duration(&[
            BoltValue::Int(0),
            BoltValue::Int(0),
            BoltValue::Int(5),
            BoltValue::Int(500_000_000),
        ]);
        let expected = CypherDuration {
            months: 0,
            days: 0,
            nanos: 5_500_000_000,
        };
        assert_eq!(val, Value::Duration(expected));
    }

    #[test]
    fn decode_duration_missing_fields_returns_null() {
        assert_eq!(decode_duration(&[]), Value::Null);
    }

    #[test]
    fn decode_duration_partial_fields_returns_null() {
        assert_eq!(decode_duration(&[BoltValue::Int(6)]), Value::Null);
    }
}
