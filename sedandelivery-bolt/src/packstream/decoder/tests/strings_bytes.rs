//! Tests for String and Bytes decoding.

use super::{decode, roundtrip, BoltValue};

// ── String ──────────────────────────────────────────────────────────

#[test]
fn decode_empty_string() {
    let (val, n) = decode(&[0x80]).unwrap();
    assert_eq!(val, BoltValue::String(String::new()));
    assert_eq!(n, 1);
}

#[test]
fn decode_tiny_string_hello() {
    let data = [0x85, 0x68, 0x65, 0x6C, 0x6C, 0x6F];
    let (val, n) = decode(&data).unwrap();
    assert_eq!(val, BoltValue::String("hello".into()));
    assert_eq!(n, 6);
}

#[test]
fn roundtrip_string_tiers() {
    roundtrip(&BoltValue::String(String::new()));
    roundtrip(&BoltValue::String("a".into()));
    roundtrip(&BoltValue::String("abcdefghijklmno".into())); // 15 bytes
    roundtrip(&BoltValue::String("a".repeat(16))); // String8
    roundtrip(&BoltValue::String("a".repeat(255))); // String8 max
    roundtrip(&BoltValue::String("a".repeat(256))); // String16
}

#[test]
fn roundtrip_string32() {
    roundtrip(&BoltValue::String("z".repeat(65_536)));
}

#[test]
fn decode_string_utf8() {
    let s = "cafe\u{0301}"; // "cafe" + combining accent
    roundtrip(&BoltValue::String(s.into()));
}

// ── Bytes ───────────────────────────────────────────────────────────

#[test]
fn decode_empty_bytes() {
    let (val, n) = decode(&[0xCC, 0x00]).unwrap();
    assert_eq!(val, BoltValue::Bytes(vec![]));
    assert_eq!(n, 2);
}

#[test]
fn roundtrip_bytes_tiers() {
    roundtrip(&BoltValue::Bytes(vec![]));
    roundtrip(&BoltValue::Bytes(vec![0xAB; 10]));
    roundtrip(&BoltValue::Bytes(vec![0x01; 255]));
    roundtrip(&BoltValue::Bytes(vec![0x02; 256]));
}
