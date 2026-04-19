//! End-to-end round-trip tests via query parameters.

use super::*;
use crate::bridge::executor::{execute_query, result_to_records};
use cypher_core::storage::InMemoryGraphStore;

#[test]
fn roundtrip_date_via_query_params() {
    let store = InMemoryGraphStore::new();
    let bolt_params: BoltMap = vec![(
        "d".to_string(),
        BoltValue::Structure {
            tag: 0x44,
            fields: vec![BoltValue::Int(19737)],
        },
    )];
    let params = bolt_map_to_value_map(&bolt_params);
    let result = execute_query(&store, "default", "RETURN $d AS d", params, None).unwrap();
    let (records, _, _, _) = result_to_records(&result, &store, "default");
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0][0],
        BoltValue::Structure {
            tag: 0x44,
            fields: vec![BoltValue::Int(19737)]
        }
    );
}

#[test]
fn roundtrip_duration_via_query_params() {
    let store = InMemoryGraphStore::new();
    let bolt_params: BoltMap = vec![(
        "d".to_string(),
        BoltValue::Structure {
            tag: 0x45,
            fields: vec![
                BoltValue::Int(14),
                BoltValue::Int(3),
                BoltValue::Int(100),
                BoltValue::Int(0),
            ],
        },
    )];
    let params = bolt_map_to_value_map(&bolt_params);
    let result = execute_query(&store, "default", "RETURN $d AS d", params, None).unwrap();
    let (records, _, _, _) = result_to_records(&result, &store, "default");
    assert_eq!(
        records[0][0],
        BoltValue::Structure {
            tag: 0x45,
            fields: vec![
                BoltValue::Int(14),
                BoltValue::Int(3),
                BoltValue::Int(100),
                BoltValue::Int(0)
            ]
        }
    );
}

#[test]
fn roundtrip_point_via_query_params() {
    let store = InMemoryGraphStore::new();
    let bolt_params: BoltMap = vec![(
        "p".to_string(),
        BoltValue::Structure {
            tag: 0x58,
            fields: vec![
                BoltValue::Int(7203),
                BoltValue::Float(1.5),
                BoltValue::Float(2.5),
            ],
        },
    )];
    let params = bolt_map_to_value_map(&bolt_params);
    let result = execute_query(&store, "default", "RETURN $p AS p", params, None).unwrap();
    let (records, _, _, _) = result_to_records(&result, &store, "default");
    assert_eq!(
        records[0][0],
        BoltValue::Structure {
            tag: 0x58,
            fields: vec![
                BoltValue::Int(7203),
                BoltValue::Float(1.5),
                BoltValue::Float(2.5)
            ]
        }
    );
}
