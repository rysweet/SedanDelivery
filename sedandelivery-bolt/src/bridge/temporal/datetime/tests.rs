//! Tests for Bolt LocalDateTime, DateTimeZoneId and DateTimeOffset decoders.

use super::*;
use crate::packstream::value::BoltValue;
use cypher_core::types::Value;

// ── LocalDateTime tests ─────────────────────────────────────────────────

#[test]
fn decode_local_datetime_from_bolt_structure() {
    let epoch_secs: i64 = 19737 * 86_400 + 10 * 3600 + 30 * 60 + 45;
    let val = decode_local_datetime(&[BoltValue::Int(epoch_secs), BoltValue::Int(123_456_789)]);
    let expected = jiff::civil::DateTime::new(2024, 1, 15, 10, 30, 45, 123_456_789).unwrap();
    assert_eq!(val, Value::LocalDateTime(expected));
}

#[test]
fn decode_local_datetime_missing_epoch_returns_null() {
    assert_eq!(decode_local_datetime(&[]), Value::Null);
}

#[test]
fn decode_local_datetime_non_integer_returns_null() {
    assert_eq!(decode_local_datetime(&[BoltValue::Float(1.0)]), Value::Null);
}

#[test]
fn decode_local_datetime_overflow_returns_null() {
    let huge_epoch = 7_304_484_i64 * 86_400;
    assert_eq!(
        decode_local_datetime(&[BoltValue::Int(huge_epoch), BoltValue::Int(0)]),
        Value::Null
    );
}

#[test]
fn decode_local_datetime_nanos_i32_overflow_returns_null() {
    assert_eq!(
        decode_local_datetime(&[BoltValue::Int(0), BoltValue::Int(i64::from(i32::MAX) + 1)]),
        Value::Null
    );
}

// ── DateTimeZoneId tests ────────────────────────────────────────────────

#[test]
fn decode_datetime_zone_id_tag_0x69() {
    let epoch_secs: i64 = 19737 * 86_400 + 10 * 3600 + 30 * 60;
    let val = decode_datetime_zone_id(&[
        BoltValue::Int(epoch_secs),
        BoltValue::Int(0),
        BoltValue::String("UTC".to_string()),
    ]);
    if let Value::DateTime(zoned) = &val {
        assert_eq!(zoned.time_zone().iana_name(), Some("UTC"));
        assert_eq!(zoned.timestamp().as_second(), epoch_secs);
    } else {
        panic!("expected Value::DateTime, got {:?}", val);
    }
}

#[test]
fn decode_datetime_zone_id_tag_0x66() {
    let epoch_secs: i64 = 19737 * 86_400 + 10 * 3600 + 30 * 60;
    let val = decode_datetime_zone_id(&[
        BoltValue::Int(epoch_secs),
        BoltValue::Int(0),
        BoltValue::String("America/New_York".to_string()),
    ]);
    if let Value::DateTime(zoned) = &val {
        assert!(zoned.time_zone().iana_name().is_some());
    } else {
        panic!("expected Value::DateTime, got {:?}", val);
    }
}

#[test]
fn decode_datetime_zone_id_missing_epoch_returns_null() {
    assert_eq!(decode_datetime_zone_id(&[]), Value::Null);
}

#[test]
fn decode_datetime_zone_id_missing_tz_returns_null() {
    assert_eq!(
        decode_datetime_zone_id(&[BoltValue::Int(0), BoltValue::Int(0)]),
        Value::Null
    );
}

#[test]
fn decode_datetime_zone_id_non_string_tz_returns_null() {
    assert_eq!(
        decode_datetime_zone_id(&[BoltValue::Int(0), BoltValue::Int(0), BoltValue::Int(999)]),
        Value::Null
    );
}

#[test]
fn decode_datetime_zone_id_invalid_tz_returns_null() {
    assert_eq!(
        decode_datetime_zone_id(&[
            BoltValue::Int(0),
            BoltValue::Int(0),
            BoltValue::String("Not/A/Real/Timezone".into())
        ]),
        Value::Null
    );
}

#[test]
fn decode_datetime_zone_id_overflow_returns_null() {
    assert_eq!(
        decode_datetime_zone_id(&[
            BoltValue::Int(i64::MAX),
            BoltValue::Int(0),
            BoltValue::String("UTC".into())
        ]),
        Value::Null
    );
}

#[test]
fn decode_datetime_zone_id_nanos_i32_overflow_returns_null() {
    assert_eq!(
        decode_datetime_zone_id(&[
            BoltValue::Int(0),
            BoltValue::Int(i64::from(i32::MAX) + 1),
            BoltValue::String("UTC".into())
        ]),
        Value::Null
    );
}

// ── DateTimeOffset tests ────────────────────────────────────────────────

#[test]
fn decode_datetime_offset_from_bolt_structure() {
    let epoch_secs: i64 = 19737 * 86_400 + 7 * 3600 + 30 * 60;
    let val = decode_datetime_offset(&[
        BoltValue::Int(epoch_secs),
        BoltValue::Int(0),
        BoltValue::Int(10800),
    ]);
    if let Value::DateTime(zoned) = &val {
        assert_eq!(zoned.offset().seconds(), 10800);
        assert_eq!(zoned.timestamp().as_second(), epoch_secs);
    } else {
        panic!("expected Value::DateTime, got {:?}", val);
    }
}

#[test]
fn decode_datetime_offset_missing_epoch_returns_null() {
    assert_eq!(decode_datetime_offset(&[]), Value::Null);
}

#[test]
fn decode_datetime_offset_missing_offset_returns_null() {
    assert_eq!(
        decode_datetime_offset(&[BoltValue::Int(0), BoltValue::Int(0)]),
        Value::Null
    );
}

#[test]
fn decode_datetime_offset_non_integer_offset_returns_null() {
    assert_eq!(
        decode_datetime_offset(&[
            BoltValue::Int(0),
            BoltValue::Int(0),
            BoltValue::String("bad".into())
        ]),
        Value::Null
    );
}

#[test]
fn decode_datetime_offset_out_of_range_offset_returns_null() {
    assert_eq!(
        decode_datetime_offset(&[
            BoltValue::Int(0),
            BoltValue::Int(0),
            BoltValue::Int(999_999)
        ]),
        Value::Null
    );
}

#[test]
fn decode_datetime_offset_overflow_timestamp_returns_null() {
    assert_eq!(
        decode_datetime_offset(&[
            BoltValue::Int(i64::MAX),
            BoltValue::Int(0),
            BoltValue::Int(3600)
        ]),
        Value::Null
    );
}

#[test]
fn decode_datetime_offset_offset_i32_overflow_returns_null() {
    assert_eq!(
        decode_datetime_offset(&[
            BoltValue::Int(0),
            BoltValue::Int(0),
            BoltValue::Int(i64::from(i32::MAX) + 1)
        ]),
        Value::Null
    );
}

#[test]
fn decode_datetime_offset_nanos_i32_overflow_returns_null() {
    assert_eq!(
        decode_datetime_offset(&[
            BoltValue::Int(0),
            BoltValue::Int(i64::from(i32::MAX) + 1),
            BoltValue::Int(3600)
        ]),
        Value::Null
    );
}
