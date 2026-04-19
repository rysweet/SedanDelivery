//! Bolt LocalTime and Time-with-offset structure decoders.

use cypher_core::types::Value;

use super::bolt_int;
use crate::packstream::value::BoltValue;

/// LocalTime (tag 0x74): 1 field -- nanoseconds since midnight.
pub(crate) fn decode_local_time(fields: &[BoltValue]) -> Value {
    let nanos = match fields.first().and_then(bolt_int) {
        Some(n) => n,
        None => {
            tracing::warn!("bolt_decode: missing or non-integer 'nanos' field in LocalTime structure, fields={fields:?}");
            return Value::Null;
        }
    };
    let h = (nanos / 3_600_000_000_000) as i8;
    let rem = nanos % 3_600_000_000_000;
    let m = (rem / 60_000_000_000) as i8;
    let rem = rem % 60_000_000_000;
    let s = (rem / 1_000_000_000) as i8;
    let ns = (rem % 1_000_000_000) as i32;
    match jiff::civil::Time::new(h, m, s, ns) {
        Ok(t) => Value::LocalTime(t),
        Err(_) => {
            tracing::warn!("bolt_decode: out-of-range LocalTime, nanos={nanos}");
            Value::Null
        }
    }
}

/// Time with UTC offset (tag 0x54): 2 fields -- nanoseconds since midnight, offset_seconds.
/// Returns a Value::Time (time + offset) representing a zoned time with no date component.
pub(crate) fn decode_time_with_offset(fields: &[BoltValue]) -> Value {
    let nanos = match fields.first().and_then(bolt_int) {
        Some(n) => n,
        None => {
            tracing::warn!("bolt_decode: missing or non-integer 'nanos' field in Time structure, fields={fields:?}");
            return Value::Null;
        }
    };
    let offset_secs = match fields.get(1).and_then(bolt_int) {
        Some(o) => o,
        None => {
            tracing::warn!("bolt_decode: missing or non-integer 'offset_seconds' field in Time structure, fields={fields:?}");
            return Value::Null;
        }
    };

    let h = (nanos / 3_600_000_000_000) as i8;
    let rem = nanos % 3_600_000_000_000;
    let m = (rem / 60_000_000_000) as i8;
    let rem = rem % 60_000_000_000;
    let s = (rem / 1_000_000_000) as i8;
    let ns = (rem % 1_000_000_000) as i32;

    let offset_i32 = match i32::try_from(offset_secs) {
        Ok(v) => v,
        Err(_) => {
            tracing::warn!(
                "bolt_decode: offset_secs={offset_secs} overflows i32 in Time structure"
            );
            return Value::Null;
        }
    };

    match jiff::civil::Time::new(h, m, s, ns) {
        Ok(t) => Value::Time(t, offset_i32),
        Err(_) => {
            tracing::warn!(
                "bolt_decode: out-of-range Time, nanos={nanos}, offset_secs={offset_secs}"
            );
            Value::Null
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── LocalTime tests ─────────────────────────────────────────────────────

    #[test]
    fn decode_local_time_from_bolt_structure() {
        let val = decode_local_time(&[BoltValue::Int(45_000_000_000_000)]);
        let expected = jiff::civil::Time::new(12, 30, 0, 0).unwrap();
        assert_eq!(val, Value::LocalTime(expected));
    }

    #[test]
    fn decode_local_time_with_nanos() {
        let nanos: i64 =
            10 * 3_600_000_000_000 + 20 * 60_000_000_000 + 30 * 1_000_000_000 + 123_456_789;
        let val = decode_local_time(&[BoltValue::Int(nanos)]);
        let expected = jiff::civil::Time::new(10, 20, 30, 123_456_789).unwrap();
        assert_eq!(val, Value::LocalTime(expected));
    }

    #[test]
    fn decode_local_time_missing_field_returns_null() {
        assert_eq!(decode_local_time(&[]), Value::Null);
    }

    #[test]
    fn decode_local_time_non_integer_returns_null() {
        assert_eq!(decode_local_time(&[BoltValue::Bool(true)]), Value::Null);
    }

    #[test]
    fn decode_local_time_out_of_range_returns_null() {
        let nanos_25h = 25 * 3_600_000_000_000_i64;
        assert_eq!(decode_local_time(&[BoltValue::Int(nanos_25h)]), Value::Null);
    }

    // ── Time with offset tests ──────────────────────────────────────────────

    #[test]
    fn decode_time_with_offset_missing_nanos_returns_null() {
        assert_eq!(decode_time_with_offset(&[]), Value::Null);
    }

    #[test]
    fn decode_time_with_offset_missing_offset_returns_null() {
        assert_eq!(decode_time_with_offset(&[BoltValue::Int(0)]), Value::Null);
    }

    #[test]
    fn decode_time_with_offset_non_integer_offset_returns_null() {
        assert_eq!(
            decode_time_with_offset(&[BoltValue::Int(0), BoltValue::String("bad".into())]),
            Value::Null
        );
    }

    #[test]
    fn decode_time_with_offset_out_of_range_returns_null() {
        let nanos_25h = 25 * 3_600_000_000_000_i64;
        assert_eq!(
            decode_time_with_offset(&[BoltValue::Int(nanos_25h), BoltValue::Int(0)]),
            Value::Null
        );
    }

    #[test]
    fn decode_time_offset_i32_overflow_returns_null() {
        let nanos_noon = 12 * 3_600_000_000_000_i64;
        assert_eq!(
            decode_time_with_offset(&[
                BoltValue::Int(nanos_noon),
                BoltValue::Int(i64::from(i32::MAX) + 1)
            ]),
            Value::Null
        );
    }
}
