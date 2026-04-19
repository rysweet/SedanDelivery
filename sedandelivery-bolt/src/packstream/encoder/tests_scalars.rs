//! Scalar (Null/Bool/Int/Float) encoder tests.

use super::*;

fn enc(v: &BoltValue) -> Vec<u8> {
    let mut buf = Vec::new();
    encode(v, &mut buf);
    buf
}

// ── Null ────────────────────────────────────────────────────────────

#[test]
fn encode_null() {
    assert_eq!(enc(&BoltValue::Null), vec![0xC0]);
}

// ── Bool ────────────────────────────────────────────────────────────

#[test]
fn encode_false() {
    assert_eq!(enc(&BoltValue::Bool(false)), vec![0xC2]);
}

#[test]
fn encode_true() {
    assert_eq!(enc(&BoltValue::Bool(true)), vec![0xC3]);
}

// ── Int: TinyInt ────────────────────────────────────────────────────

#[test]
fn encode_int_zero() {
    assert_eq!(enc(&BoltValue::Int(0)), vec![0x00]);
}

#[test]
fn encode_int_one() {
    assert_eq!(enc(&BoltValue::Int(1)), vec![0x01]);
}

#[test]
fn encode_int_127() {
    assert_eq!(enc(&BoltValue::Int(127)), vec![0x7F]);
}

#[test]
fn encode_int_neg1() {
    // -1 as u8 = 0xFF, within TinyInt range
    assert_eq!(enc(&BoltValue::Int(-1)), vec![0xFF]);
}

#[test]
fn encode_int_neg16() {
    // -16 as u8 = 0xF0, lower bound of TinyInt
    assert_eq!(enc(&BoltValue::Int(-16)), vec![0xF0]);
}

// ── Int: Int8 ───────────────────────────────────────────────────────

#[test]
fn encode_int_neg17() {
    // -17 = 0xEF as i8
    assert_eq!(enc(&BoltValue::Int(-17)), vec![0xC8, 0xEF]);
}

#[test]
fn encode_int_neg128() {
    // -128 = 0x80 as i8
    assert_eq!(enc(&BoltValue::Int(-128)), vec![0xC8, 0x80]);
}

// ── Int: Int16 ──────────────────────────────────────────────────────

#[test]
fn encode_int_128() {
    // 128 does not fit TinyInt (max 127), not negative so not Int8 → Int16
    assert_eq!(enc(&BoltValue::Int(128)), vec![0xC9, 0x00, 0x80]);
}

#[test]
fn encode_int_neg129() {
    // -129 does not fit Int8 (min -128) → Int16
    assert_eq!(enc(&BoltValue::Int(-129)), vec![0xC9, 0xFF, 0x7F]);
}

#[test]
fn encode_int_32767() {
    assert_eq!(enc(&BoltValue::Int(32767)), vec![0xC9, 0x7F, 0xFF]);
}

#[test]
fn encode_int_neg32768() {
    assert_eq!(enc(&BoltValue::Int(-32768)), vec![0xC9, 0x80, 0x00]);
}

// ── Int: Int32 ──────────────────────────────────────────────────────

#[test]
fn encode_int_32768() {
    assert_eq!(
        enc(&BoltValue::Int(32768)),
        vec![0xCA, 0x00, 0x00, 0x80, 0x00]
    );
}

#[test]
fn encode_int_neg32769() {
    // -32769 does not fit Int16
    let expected = (-32769_i32).to_be_bytes();
    let mut want = vec![0xCA];
    want.extend_from_slice(&expected);
    assert_eq!(enc(&BoltValue::Int(-32769)), want);
}

#[test]
fn encode_int_i32_max() {
    assert_eq!(
        enc(&BoltValue::Int(i32::MAX as i64)),
        vec![0xCA, 0x7F, 0xFF, 0xFF, 0xFF]
    );
}

#[test]
fn encode_int_i32_min() {
    assert_eq!(
        enc(&BoltValue::Int(i32::MIN as i64)),
        vec![0xCA, 0x80, 0x00, 0x00, 0x00]
    );
}

// ── Int: Int64 ──────────────────────────────────────────────────────

#[test]
fn encode_int_i64_max() {
    let mut want = vec![0xCB];
    want.extend_from_slice(&i64::MAX.to_be_bytes());
    assert_eq!(enc(&BoltValue::Int(i64::MAX)), want);
}

#[test]
fn encode_int_i64_min() {
    let mut want = vec![0xCB];
    want.extend_from_slice(&i64::MIN.to_be_bytes());
    assert_eq!(enc(&BoltValue::Int(i64::MIN)), want);
}

#[test]
fn encode_int_above_i32_max() {
    let n = i32::MAX as i64 + 1;
    let mut want = vec![0xCB];
    want.extend_from_slice(&n.to_be_bytes());
    assert_eq!(enc(&BoltValue::Int(n)), want);
}

// ── Float ───────────────────────────────────────────────────────────

#[test]
fn encode_float_positive() {
    let result = enc(&BoltValue::Float(1.1));
    assert_eq!(result.len(), 9);
    assert_eq!(result[0], 0xC1);
    let f = f64::from_be_bytes(result[1..9].try_into().unwrap());
    assert!((f - 1.1).abs() < f64::EPSILON);
}

#[test]
fn encode_float_zero() {
    let result = enc(&BoltValue::Float(0.0));
    assert_eq!(result[0], 0xC1);
    assert_eq!(result.len(), 9);
}
