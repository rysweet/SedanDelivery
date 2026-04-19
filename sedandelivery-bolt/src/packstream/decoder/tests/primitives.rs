//! Tests for primitive scalars: Null, Bool, TinyInt, Int8/16/32/64, Float.

use super::{decode, roundtrip, BoltValue};

// ── Null ────────────────────────────────────────────────────────────

#[test]
fn decode_null() {
    let (val, n) = decode(&[0xC0]).unwrap();
    assert_eq!(val, BoltValue::Null);
    assert_eq!(n, 1);
}

#[test]
fn roundtrip_null() {
    roundtrip(&BoltValue::Null);
}

// ── Bool ────────────────────────────────────────────────────────────

#[test]
fn decode_false() {
    let (val, n) = decode(&[0xC2]).unwrap();
    assert_eq!(val, BoltValue::Bool(false));
    assert_eq!(n, 1);
}

#[test]
fn decode_true() {
    let (val, n) = decode(&[0xC3]).unwrap();
    assert_eq!(val, BoltValue::Bool(true));
    assert_eq!(n, 1);
}

#[test]
fn roundtrip_bool() {
    roundtrip(&BoltValue::Bool(true));
    roundtrip(&BoltValue::Bool(false));
}

// ── TinyInt ─────────────────────────────────────────────────────────

#[test]
fn decode_tinyint_zero() {
    let (val, n) = decode(&[0x00]).unwrap();
    assert_eq!(val, BoltValue::Int(0));
    assert_eq!(n, 1);
}

#[test]
fn decode_tinyint_1() {
    let (val, _) = decode(&[0x01]).unwrap();
    assert_eq!(val, BoltValue::Int(1));
}

#[test]
fn decode_tinyint_127() {
    let (val, _) = decode(&[0x7F]).unwrap();
    assert_eq!(val, BoltValue::Int(127));
}

#[test]
fn decode_tinyint_neg1() {
    let (val, _) = decode(&[0xFF]).unwrap();
    assert_eq!(val, BoltValue::Int(-1));
}

#[test]
fn decode_tinyint_neg16() {
    let (val, _) = decode(&[0xF0]).unwrap();
    assert_eq!(val, BoltValue::Int(-16));
}

// ── Int8 ────────────────────────────────────────────────────────────

#[test]
fn decode_int8_neg17() {
    let (val, n) = decode(&[0xC8, 0xEF]).unwrap();
    assert_eq!(val, BoltValue::Int(-17));
    assert_eq!(n, 2);
}

#[test]
fn decode_int8_neg128() {
    let (val, _) = decode(&[0xC8, 0x80]).unwrap();
    assert_eq!(val, BoltValue::Int(-128));
}

// ── Int16 ───────────────────────────────────────────────────────────

#[test]
fn decode_int16_128() {
    let (val, n) = decode(&[0xC9, 0x00, 0x80]).unwrap();
    assert_eq!(val, BoltValue::Int(128));
    assert_eq!(n, 3);
}

#[test]
fn decode_int16_neg129() {
    let (val, _) = decode(&[0xC9, 0xFF, 0x7F]).unwrap();
    assert_eq!(val, BoltValue::Int(-129));
}

#[test]
fn decode_int16_32767() {
    let (val, _) = decode(&[0xC9, 0x7F, 0xFF]).unwrap();
    assert_eq!(val, BoltValue::Int(32767));
}

#[test]
fn decode_int16_neg32768() {
    let (val, _) = decode(&[0xC9, 0x80, 0x00]).unwrap();
    assert_eq!(val, BoltValue::Int(-32768));
}

// ── Int32 ───────────────────────────────────────────────────────────

#[test]
fn decode_int32_32768() {
    let (val, n) = decode(&[0xCA, 0x00, 0x00, 0x80, 0x00]).unwrap();
    assert_eq!(val, BoltValue::Int(32768));
    assert_eq!(n, 5);
}

#[test]
fn decode_int32_i32_max() {
    let (val, _) = decode(&[0xCA, 0x7F, 0xFF, 0xFF, 0xFF]).unwrap();
    assert_eq!(val, BoltValue::Int(i32::MAX as i64));
}

#[test]
fn decode_int32_i32_min() {
    let (val, _) = decode(&[0xCA, 0x80, 0x00, 0x00, 0x00]).unwrap();
    assert_eq!(val, BoltValue::Int(i32::MIN as i64));
}

// ── Int64 ───────────────────────────────────────────────────────────

#[test]
fn decode_int64_max() {
    let mut data = vec![0xCB];
    data.extend_from_slice(&i64::MAX.to_be_bytes());
    let (val, n) = decode(&data).unwrap();
    assert_eq!(val, BoltValue::Int(i64::MAX));
    assert_eq!(n, 9);
}

#[test]
fn decode_int64_min() {
    let mut data = vec![0xCB];
    data.extend_from_slice(&i64::MIN.to_be_bytes());
    let (val, _) = decode(&data).unwrap();
    assert_eq!(val, BoltValue::Int(i64::MIN));
}

// ── Roundtrip all integer boundaries ────────────────────────────────

#[test]
fn roundtrip_int_boundaries() {
    let values: Vec<i64> = vec![
        0,
        1,
        -1,
        127,
        -16,
        -17,
        -128,
        128,
        -129,
        32767,
        -32768,
        32768,
        -32769,
        i32::MAX as i64,
        i32::MIN as i64,
        i32::MAX as i64 + 1,
        i32::MIN as i64 - 1,
        i64::MAX,
        i64::MIN,
    ];
    for v in values {
        roundtrip(&BoltValue::Int(v));
    }
}

// ── Float ───────────────────────────────────────────────────────────

#[test]
fn decode_float_1_1() {
    let data = [0xC1, 0x3F, 0xF1, 0x99, 0x99, 0x99, 0x99, 0x99, 0x9A];
    let (val, n) = decode(&data).unwrap();
    assert_eq!(n, 9);
    match val {
        BoltValue::Float(f) => assert!((f - 1.1).abs() < f64::EPSILON),
        _ => panic!("expected Float"),
    }
}

#[test]
fn roundtrip_float() {
    roundtrip(&BoltValue::Float(0.0));
    roundtrip(&BoltValue::Float(1.1));
    roundtrip(&BoltValue::Float(-999.999));
    roundtrip(&BoltValue::Float(f64::MAX));
    roundtrip(&BoltValue::Float(f64::MIN));
}
