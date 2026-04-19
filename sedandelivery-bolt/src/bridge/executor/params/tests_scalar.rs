//! Tests for scalar/list/map conversion via `bolt_map_to_value_map`.

use super::*;

#[test]
fn bolt_map_to_value_map_basic() {
    let bolt_map: BoltMap = vec![
        ("name".to_string(), BoltValue::String("Alice".to_string())),
        ("age".to_string(), BoltValue::Int(30)),
        ("active".to_string(), BoltValue::Bool(true)),
    ];

    let value_map = bolt_map_to_value_map(&bolt_map);
    assert_eq!(value_map.len(), 3);
    assert_eq!(value_map["name"], Value::Str("Alice".into()));
    assert_eq!(value_map["age"], Value::Int(30));
    assert_eq!(value_map["active"], Value::Bool(true));
}

#[test]
fn bolt_map_to_value_map_nested() {
    let inner_map: BoltMap = vec![("x".to_string(), BoltValue::Int(1))];
    let bolt_map: BoltMap = vec![
        (
            "list".to_string(),
            BoltValue::List(vec![BoltValue::Int(1), BoltValue::Int(2)]),
        ),
        ("map".to_string(), BoltValue::Map(inner_map)),
        ("null".to_string(), BoltValue::Null),
    ];

    let value_map = bolt_map_to_value_map(&bolt_map);
    assert_eq!(value_map.len(), 3);
    if let Value::List(items) = &value_map["list"] {
        assert_eq!(items.len(), 2);
    } else {
        panic!("expected List");
    }
    if let Value::Map(m) = &value_map["map"] {
        assert_eq!(m.len(), 1);
    } else {
        panic!("expected Map");
    }
    assert_eq!(value_map["null"], Value::Null);
}

#[test]
fn bolt_map_to_value_map_empty() {
    let bolt_map: BoltMap = vec![];
    let value_map = bolt_map_to_value_map(&bolt_map);
    assert!(value_map.is_empty());
}

#[test]
fn bolt_bytes_converts_to_bytes() {
    let bolt_map: BoltMap = vec![("data".to_string(), BoltValue::Bytes(vec![0x01, 0x02]))];
    let value_map = bolt_map_to_value_map(&bolt_map);
    assert_eq!(value_map["data"], Value::Bytes(vec![0x01, 0x02]));
}
