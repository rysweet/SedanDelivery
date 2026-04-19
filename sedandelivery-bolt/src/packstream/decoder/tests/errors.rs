//! Error-condition tests, spec hex examples, and partial-consumption checks.

use super::{decode, BoltValue, DecodeError};

// ── Error conditions ────────────────────────────────────────────────

#[test]
fn decode_empty_input() {
    let err = decode(&[]).unwrap_err();
    assert!(matches!(err, DecodeError::UnexpectedEof { .. }));
}

#[test]
fn decode_truncated_int16() {
    // Int16 marker but only 1 payload byte
    let err = decode(&[0xC9, 0x00]).unwrap_err();
    assert!(matches!(err, DecodeError::UnexpectedEof { .. }));
}

#[test]
fn decode_truncated_float() {
    let err = decode(&[0xC1, 0x00, 0x00]).unwrap_err();
    assert!(matches!(err, DecodeError::UnexpectedEof { .. }));
}

#[test]
fn decode_truncated_string() {
    // TinyString of length 5 but only 2 bytes of payload
    let err = decode(&[0x85, b'h', b'i']).unwrap_err();
    assert!(matches!(err, DecodeError::UnexpectedEof { .. }));
}

#[test]
fn decode_invalid_utf8_string() {
    // TinyString of length 2 with invalid UTF-8
    let err = decode(&[0x82, 0xFF, 0xFE]).unwrap_err();
    assert!(matches!(err, DecodeError::Utf8Error(_)));
}

#[test]
fn decode_non_string_map_key() {
    // Map with 1 entry, key is Int(1) instead of String
    let err = decode(&[0xA1, 0x01, 0x02]).unwrap_err();
    assert!(matches!(err, DecodeError::NonStringMapKey(_)));
}

#[test]
fn decode_invalid_marker() {
    // 0xCF is reserved/unused
    let err = decode(&[0xCF]).unwrap_err();
    assert!(matches!(err, DecodeError::InvalidMarker(0xCF)));
}

// ── Spec hex examples ───────────────────────────────────────────────

#[test]
fn spec_hex_null() {
    let (val, _) = decode(&[0xC0]).unwrap();
    assert_eq!(val, BoltValue::Null);
}

#[test]
fn spec_hex_false() {
    let (val, _) = decode(&[0xC2]).unwrap();
    assert_eq!(val, BoltValue::Bool(false));
}

#[test]
fn spec_hex_true() {
    let (val, _) = decode(&[0xC3]).unwrap();
    assert_eq!(val, BoltValue::Bool(true));
}

#[test]
fn spec_hex_int_1() {
    let (val, _) = decode(&[0x01]).unwrap();
    assert_eq!(val, BoltValue::Int(1));
}

#[test]
fn spec_hex_int_42() {
    let (val, _) = decode(&[0x2A]).unwrap();
    assert_eq!(val, BoltValue::Int(42));
}

#[test]
fn spec_hex_float_1_1() {
    let (val, _) = decode(&[0xC1, 0x3F, 0xF1, 0x99, 0x99, 0x99, 0x99, 0x99, 0x9A]).unwrap();
    match val {
        BoltValue::Float(f) => assert!((f - 1.1).abs() < f64::EPSILON),
        _ => panic!("expected Float"),
    }
}

#[test]
fn spec_hex_string_hello() {
    let (val, _) = decode(&[0x85, 0x68, 0x65, 0x6C, 0x6C, 0x6F]).unwrap();
    assert_eq!(val, BoltValue::String("hello".into()));
}

#[test]
fn spec_hex_list_1_2_3() {
    let (val, _) = decode(&[0x93, 0x01, 0x02, 0x03]).unwrap();
    assert_eq!(
        val,
        BoltValue::List(vec![
            BoltValue::Int(1),
            BoltValue::Int(2),
            BoltValue::Int(3),
        ])
    );
}

#[test]
fn spec_hex_map_one_1() {
    let (val, _) = decode(&[0xA1, 0x83, 0x6F, 0x6E, 0x65, 0x01]).unwrap();
    assert_eq!(val, BoltValue::Map(vec![("one".into(), BoltValue::Int(1))]));
}

// ── Extra bytes after value are not consumed ────────────────────────

#[test]
fn decode_consumes_only_needed_bytes() {
    let data = [0x01, 0x02, 0x03]; // Int(1) followed by extra bytes
    let (val, n) = decode(&data).unwrap();
    assert_eq!(val, BoltValue::Int(1));
    assert_eq!(n, 1); // only 1 byte consumed
}
