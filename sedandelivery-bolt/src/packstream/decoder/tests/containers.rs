//! Tests for List and Map decoding.

use super::{decode, roundtrip, BoltValue};

// ── List ────────────────────────────────────────────────────────────

#[test]
fn decode_empty_list() {
    let (val, n) = decode(&[0x90]).unwrap();
    assert_eq!(val, BoltValue::List(vec![]));
    assert_eq!(n, 1);
}

#[test]
fn decode_tiny_list() {
    let data = [0x93, 0x01, 0x02, 0x03];
    let (val, n) = decode(&data).unwrap();
    assert_eq!(
        val,
        BoltValue::List(vec![
            BoltValue::Int(1),
            BoltValue::Int(2),
            BoltValue::Int(3),
        ])
    );
    assert_eq!(n, 4);
}

#[test]
fn roundtrip_list_empty() {
    roundtrip(&BoltValue::List(vec![]));
}

#[test]
fn roundtrip_list_nested() {
    let v = BoltValue::List(vec![
        BoltValue::List(vec![BoltValue::Int(1), BoltValue::Int(2)]),
        BoltValue::Null,
        BoltValue::String("test".into()),
    ]);
    roundtrip(&v);
}

#[test]
fn roundtrip_list8() {
    let items: Vec<BoltValue> = (0..16).map(BoltValue::Int).collect();
    roundtrip(&BoltValue::List(items));
}

#[test]
fn roundtrip_list16() {
    let items: Vec<BoltValue> = (0..256).map(BoltValue::Int).collect();
    roundtrip(&BoltValue::List(items));
}

// ── Map ─────────────────────────────────────────────────────────────

#[test]
fn decode_empty_map() {
    let (val, n) = decode(&[0xA0]).unwrap();
    assert_eq!(val, BoltValue::Map(vec![]));
    assert_eq!(n, 1);
}

#[test]
fn decode_tiny_map_one_entry() {
    // {"one": 1} → A1 83 6F 6E 65 01
    let data = [0xA1, 0x83, 0x6F, 0x6E, 0x65, 0x01];
    let (val, n) = decode(&data).unwrap();
    assert_eq!(val, BoltValue::Map(vec![("one".into(), BoltValue::Int(1))]));
    assert_eq!(n, 6);
}

#[test]
fn roundtrip_map() {
    let map = BoltValue::Map(vec![
        ("name".into(), BoltValue::String("Alice".into())),
        ("age".into(), BoltValue::Int(30)),
        ("active".into(), BoltValue::Bool(true)),
    ]);
    roundtrip(&map);
}

#[test]
fn roundtrip_map8() {
    let entries: Vec<(String, BoltValue)> = (0..16)
        .map(|i| (format!("key_{}", i), BoltValue::Int(i)))
        .collect();
    roundtrip(&BoltValue::Map(entries));
}
