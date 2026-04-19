//! Bolt Date structure decoder.

use cypher_core::types::Value;

use super::bolt_int;
use crate::packstream::value::BoltValue;

/// Date (tag 0x44): 1 field -- days since 1970-01-01.
pub(crate) fn decode_date(fields: &[BoltValue]) -> Value {
    let days = match fields.first().and_then(bolt_int) {
        Some(d) => d,
        None => {
            tracing::warn!("bolt_decode: missing or non-integer 'days' field in Date structure, fields={fields:?}");
            return Value::Null;
        }
    };
    let epoch = jiff::civil::Date::new(1970, 1, 1).expect("valid epoch");
    match epoch.checked_add(jiff::Span::new().days(days)) {
        Ok(d) => Value::Date(d),
        Err(_) => {
            tracing::warn!("bolt_decode: date overflow, days={days} out of representable range");
            Value::Null
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_date_from_bolt_structure() {
        let val = decode_date(&[BoltValue::Int(19737)]);
        let expected = jiff::civil::Date::new(2024, 1, 15).unwrap();
        assert_eq!(val, Value::Date(expected));
    }

    #[test]
    fn decode_date_epoch() {
        let val = decode_date(&[BoltValue::Int(0)]);
        let expected = jiff::civil::Date::new(1970, 1, 1).unwrap();
        assert_eq!(val, Value::Date(expected));
    }

    #[test]
    fn decode_date_missing_field_returns_null() {
        assert_eq!(decode_date(&[]), Value::Null);
    }

    #[test]
    fn decode_date_non_integer_field_returns_null() {
        assert_eq!(
            decode_date(&[BoltValue::String("not-a-number".into())]),
            Value::Null
        );
    }

    #[test]
    fn decode_date_overflow_returns_null() {
        assert_eq!(decode_date(&[BoltValue::Int(7_304_484)]), Value::Null);
    }
}
