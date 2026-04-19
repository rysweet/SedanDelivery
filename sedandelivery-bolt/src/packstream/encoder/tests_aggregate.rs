//! Aggregate (String/Bytes/List/Map/Structure/spec) encoder tests.

use super::*;

fn enc(v: &BoltValue) -> Vec<u8> {
    let mut buf = Vec::new();
    encode(v, &mut buf);
    buf
}

// ── String ──────────────────────────────────────────────────────────

#[test]
fn encode_empty_string() {
    // TinyString with len 0: marker = 0x80
    assert_eq!(enc(&BoltValue::String(String::new())), vec![0x80]);
}

#[test]
fn encode_tiny_string() {
    let s = "A";
    let result = enc(&BoltValue::String(s.into()));
    assert_eq!(result, vec![0x81, b'A']);
}

#[test]
fn encode_tiny_string_max() {
    let s = "abcdefghijklmno"; // exactly 15 bytes
    let result = enc(&BoltValue::String(s.into()));
    assert_eq!(result[0], 0x8F);
    assert_eq!(&result[1..], s.as_bytes());
}

#[test]
fn encode_string8() {
    // 16 bytes → String8
    let s: String = "a".repeat(16);
    let result = enc(&BoltValue::String(s.clone()));
    assert_eq!(result[0], 0xD0);
    assert_eq!(result[1], 16);
    assert_eq!(&result[2..], s.as_bytes());
}

#[test]
fn encode_string8_max() {
    let s: String = "x".repeat(255);
    let result = enc(&BoltValue::String(s.clone()));
    assert_eq!(result[0], 0xD0);
    assert_eq!(result[1], 255);
    assert_eq!(result.len(), 2 + 255);
}

#[test]
fn encode_string16() {
    let s: String = "y".repeat(256);
    let result = enc(&BoltValue::String(s.clone()));
    assert_eq!(result[0], 0xD1);
    let len = u16::from_be_bytes([result[1], result[2]]);
    assert_eq!(len, 256);
    assert_eq!(result.len(), 3 + 256);
}

#[test]
fn encode_string32() {
    let s: String = "z".repeat(65_536);
    let result = enc(&BoltValue::String(s.clone()));
    assert_eq!(result[0], 0xD2);
    let len = u32::from_be_bytes([result[1], result[2], result[3], result[4]]);
    assert_eq!(len, 65_536);
    assert_eq!(result.len(), 5 + 65_536);
}

// ── Bytes ───────────────────────────────────────────────────────────

#[test]
fn encode_empty_bytes() {
    assert_eq!(enc(&BoltValue::Bytes(vec![])), vec![0xCC, 0x00]);
}

#[test]
fn encode_bytes8() {
    let data = vec![0xAB; 10];
    let result = enc(&BoltValue::Bytes(data.clone()));
    assert_eq!(result[0], 0xCC);
    assert_eq!(result[1], 10);
    assert_eq!(&result[2..], &data[..]);
}

#[test]
fn encode_bytes16() {
    let data = vec![0x01; 256];
    let result = enc(&BoltValue::Bytes(data.clone()));
    assert_eq!(result[0], 0xCD);
    let len = u16::from_be_bytes([result[1], result[2]]);
    assert_eq!(len, 256);
}

// ── List ────────────────────────────────────────────────────────────

#[test]
fn encode_empty_list() {
    assert_eq!(enc(&BoltValue::List(vec![])), vec![0x90]);
}

#[test]
fn encode_tiny_list() {
    let list = vec![BoltValue::Int(1), BoltValue::Int(2), BoltValue::Int(3)];
    let result = enc(&BoltValue::List(list));
    // 0x93, then 0x01, 0x02, 0x03
    assert_eq!(result, vec![0x93, 0x01, 0x02, 0x03]);
}

#[test]
fn encode_list8() {
    // 16 items → List8
    let list: Vec<BoltValue> = (0..16).map(BoltValue::Int).collect();
    let result = enc(&BoltValue::List(list));
    assert_eq!(result[0], 0xD4);
    assert_eq!(result[1], 16);
}

// ── Map ─────────────────────────────────────────────────────────────

#[test]
fn encode_empty_map() {
    assert_eq!(enc(&BoltValue::Map(vec![])), vec![0xA0]);
}

#[test]
fn encode_tiny_map() {
    let map = vec![("one".into(), BoltValue::Int(1))];
    let result = enc(&BoltValue::Map(map));
    // 0xA1, then key "one" (0x83 + b"one"), then value 1 (0x01)
    assert_eq!(result, vec![0xA1, 0x83, b'o', b'n', b'e', 0x01]);
}

// ── Structure ───────────────────────────────────────────────────────

#[test]
fn encode_structure_no_fields() {
    let s = BoltValue::Structure {
        tag: 0x01,
        fields: vec![],
    };
    assert_eq!(enc(&s), vec![0xB0, 0x01]);
}

#[test]
fn encode_structure_with_fields() {
    let s = BoltValue::Structure {
        tag: 0x71,
        fields: vec![BoltValue::Int(42)],
    };
    let result = enc(&s);
    assert_eq!(result, vec![0xB1, 0x71, 0x2A]);
}

#[test]
fn encode_structure_caps_at_15_fields() {
    // 16 fields should NOT panic; the encoder should cap at 15.
    let fields: Vec<BoltValue> = (0..16).map(BoltValue::Int).collect();
    let s = BoltValue::Structure { tag: 0x01, fields };
    let result = enc(&s);
    // Marker should indicate 15 fields (0xBF = 0xB0 | 15)
    assert_eq!(result[0], 0xBF);
    assert_eq!(result[1], 0x01); // tag
                                 // Only 15 encoded field values should follow (fields 0..15, each a TinyInt)
    assert_eq!(result.len(), 2 + 15);
}

// ── Roundtrip sanity (uses decoder in decoder.rs tests) ─────────────

#[test]
fn encode_nested_list_and_map() {
    let val = BoltValue::List(vec![
        BoltValue::Map(vec![
            ("a".into(), BoltValue::Int(1)),
            ("b".into(), BoltValue::Bool(true)),
        ]),
        BoltValue::Null,
    ]);
    let result = enc(&val);
    // Should start with 0x92 (list of 2)
    assert_eq!(result[0], 0x92);
    // Second-to-last byte before Null should be Null marker
    assert_eq!(*result.last().unwrap(), 0xC0);
}

// ── Spec hex examples ───────────────────────────────────────────────

#[test]
fn spec_example_int_1() {
    // PackStream spec: integer 1 → 0x01
    assert_eq!(enc(&BoltValue::Int(1)), vec![0x01]);
}

#[test]
fn spec_example_int_42() {
    // PackStream spec: integer 42 → 0x2A
    assert_eq!(enc(&BoltValue::Int(42)), vec![0x2A]);
}

#[test]
fn spec_example_int_neg7() {
    // -7 as TinyInt: 0xF9
    assert_eq!(enc(&BoltValue::Int(-7)), vec![0xF9]);
}

#[test]
fn spec_example_int_neg16() {
    // -16 as TinyInt: 0xF0
    assert_eq!(enc(&BoltValue::Int(-16)), vec![0xF0]);
}

#[test]
fn spec_example_string_hello() {
    // "hello" → 0x85 68 65 6C 6C 6F
    assert_eq!(
        enc(&BoltValue::String("hello".into())),
        vec![0x85, 0x68, 0x65, 0x6C, 0x6C, 0x6F]
    );
}

#[test]
fn spec_example_list_1_2_3() {
    // [1,2,3] → 0x93 01 02 03
    let v = BoltValue::List(vec![
        BoltValue::Int(1),
        BoltValue::Int(2),
        BoltValue::Int(3),
    ]);
    assert_eq!(enc(&v), vec![0x93, 0x01, 0x02, 0x03]);
}

#[test]
fn spec_example_float_1_1() {
    // 1.1 → 0xC1 3F F1 99 99 99 99 99 9A
    assert_eq!(
        enc(&BoltValue::Float(1.1)),
        vec![0xC1, 0x3F, 0xF1, 0x99, 0x99, 0x99, 0x99, 0x99, 0x9A]
    );
}

#[test]
fn spec_example_map_one_1() {
    // {"one": 1} → A1 83 6F 6E 65 01
    let v = BoltValue::Map(vec![("one".into(), BoltValue::Int(1))]);
    assert_eq!(enc(&v), vec![0xA1, 0x83, 0x6F, 0x6E, 0x65, 0x01]);
}
