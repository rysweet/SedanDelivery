//! Tests for Structure decoding and deeply-nested heterogeneous values.

use super::{decode, roundtrip, BoltValue};

// ── Structure ───────────────────────────────────────────────────────

#[test]
fn decode_structure_no_fields() {
    let (val, n) = decode(&[0xB0, 0x01]).unwrap();
    assert_eq!(
        val,
        BoltValue::Structure {
            tag: 0x01,
            fields: vec![]
        }
    );
    assert_eq!(n, 2);
}

#[test]
fn decode_structure_with_fields() {
    // tag 0x71, one field: Int(42)
    let data = [0xB1, 0x71, 0x2A];
    let (val, n) = decode(&data).unwrap();
    assert_eq!(
        val,
        BoltValue::Structure {
            tag: 0x71,
            fields: vec![BoltValue::Int(42)]
        }
    );
    assert_eq!(n, 3);
}

#[test]
fn roundtrip_structure() {
    let s = BoltValue::Structure {
        tag: 0x4E, // Node
        fields: vec![
            BoltValue::Int(123),
            BoltValue::List(vec![BoltValue::String("Person".into())]),
            BoltValue::Map(vec![
                ("name".into(), BoltValue::String("Alice".into())),
                ("age".into(), BoltValue::Int(30)),
            ]),
        ],
    };
    roundtrip(&s);
}

// ── Nested ──────────────────────────────────────────────────────────

#[test]
fn roundtrip_deeply_nested() {
    let v = BoltValue::Map(vec![(
        "data".into(),
        BoltValue::List(vec![
            BoltValue::Map(vec![
                ("id".into(), BoltValue::Int(1)),
                (
                    "tags".into(),
                    BoltValue::List(vec![
                        BoltValue::String("a".into()),
                        BoltValue::String("b".into()),
                    ]),
                ),
            ]),
            BoltValue::Null,
            BoltValue::Bool(false),
            BoltValue::Float(2.72),
            BoltValue::Bytes(vec![0xDE, 0xAD]),
        ]),
    )]);
    roundtrip(&v);
}
