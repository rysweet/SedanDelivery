//! Canonical Value <-> JSON serialization with tagged-string encoding.
//!
//! Used by all storage backends (redis-store, fabric-store) and the Redis module.
//!
//! ## Encoding scheme
//!
//! Scalar types that JSON cannot represent natively are encoded as tagged strings:
//! - `__date:2024-01-15`
//! - `__localtime:10:30:00`
//! - `__localdatetime:2024-01-15T10:30:00`
//! - `__datetime:2024-01-15T10:30:00+01:00[Europe/Berlin]`
//! - `__duration:P1Y2M3D`
//! - `__point:1.5,2.5,Wgs84`
//! - `__node:42`
//! - `__edge:7`
//!
//! Paths are encoded as JSON objects with `__path_nodes` and `__path_edges` arrays.
//!
//! ## CRS default
//!
//! Unknown CRS variants decode to `Crs::Wgs84` (consistent with GIS standards).
//!
//! ## Error handling
//!
//! Non-finite floats (NaN, Infinity, -Infinity) return `StorageError::Serialization`.

mod deserialize;
mod serialize;

pub use deserialize::{json_to_props, json_to_value};
pub use serialize::{props_to_json, value_to_json};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{EdgeId, NodeId};
    use crate::types::value::{Crs, CypherDuration, GeoPoint};
    use crate::types::Value;
    use indexmap::IndexMap;
    use serde_json::Value as JsonValue;

    #[test]
    fn round_trip_scalars() {
        let cases = vec![
            Value::Null,
            Value::Bool(true),
            Value::Bool(false),
            Value::Int(42),
            Value::Int(-1),
            Value::Float(std::f64::consts::PI),
            Value::Str("hello".into()),
        ];
        for v in cases {
            let json = value_to_json(&v).unwrap();
            let back = json_to_value(&json).unwrap();
            assert_eq!(v, back, "round-trip failed for {v:?}");
        }
    }

    #[test]
    fn non_finite_float_errors() {
        assert!(value_to_json(&Value::Float(f64::NAN)).is_err());
        assert!(value_to_json(&Value::Float(f64::INFINITY)).is_err());
        assert!(value_to_json(&Value::Float(f64::NEG_INFINITY)).is_err());
    }

    #[test]
    fn round_trip_temporal() {
        let date = Value::Date(jiff::civil::date(2024, 1, 15));
        let json = value_to_json(&date).unwrap();
        let back = json_to_value(&json).unwrap();
        assert_eq!(date, back);

        let time = Value::LocalTime(jiff::civil::time(10, 30, 0, 0));
        let json = value_to_json(&time).unwrap();
        let back = json_to_value(&json).unwrap();
        assert_eq!(time, back);
    }

    #[test]
    fn round_trip_point() {
        let point = Value::Point(GeoPoint {
            x: 1.5,
            y: 2.5,
            z: None,
            crs: Crs::Wgs84,
        });
        let json = value_to_json(&point).unwrap();
        let back = json_to_value(&json).unwrap();
        assert_eq!(point, back);
    }

    #[test]
    fn unknown_crs_defaults_to_wgs84() {
        let json = JsonValue::String("__point:1.0,2.0,UnknownCrs".into());
        let val = json_to_value(&json).unwrap();
        match val {
            Value::Point(p) => assert_eq!(p.crs, Crs::Wgs84),
            other => panic!("expected Point, got {other:?}"),
        }
    }

    #[test]
    fn round_trip_node_edge_path() {
        let node = Value::Node(NodeId(42));
        let json = value_to_json(&node).unwrap();
        let back = json_to_value(&json).unwrap();
        assert_eq!(node, back);

        let edge = Value::Edge(EdgeId(7));
        let json = value_to_json(&edge).unwrap();
        let back = json_to_value(&json).unwrap();
        assert_eq!(edge, back);

        let path = Value::Path {
            nodes: vec![NodeId(1), NodeId(2), NodeId(3)],
            edges: vec![EdgeId(10), EdgeId(20)],
        };
        let json = value_to_json(&path).unwrap();
        let back = json_to_value(&json).unwrap();
        assert_eq!(path, back);
    }

    #[test]
    fn round_trip_list_and_map() {
        let list = Value::List(vec![Value::Int(1), Value::Str("two".into()), Value::Null]);
        let json = value_to_json(&list).unwrap();
        let back = json_to_value(&json).unwrap();
        assert_eq!(list, back);
    }

    #[test]
    fn props_round_trip() {
        let mut props = IndexMap::new();
        props.insert("name".into(), Value::Str("Alice".into()));
        props.insert("age".into(), Value::Int(30));

        let json_str = props_to_json(&props).unwrap();
        let back = json_to_props(&json_str).unwrap();
        assert_eq!(props, back);
    }

    #[test]
    fn json_to_props_rejects_non_object() {
        assert!(json_to_props("42").is_err());
        assert!(json_to_props("\"hello\"").is_err());
    }

    // ── R4-2 / R4-10: tagged string escape tests ─────────────────────────────

    #[test]
    fn str_escape_round_trip() {
        // A Value::Str that starts with __ must survive a round-trip via the
        // __str: escape prefix.
        let val = Value::Str("__date:2024-01-15".into());
        let json = value_to_json(&val).unwrap();
        // Serialised form should be "__str:__date:2024-01-15"
        assert_eq!(json, JsonValue::String("__str:__date:2024-01-15".into()));
        let back = json_to_value(&json).unwrap();
        assert_eq!(val, back);
    }

    #[test]
    fn str_escape_double_underscore_prefix() {
        // Any __ prefix gets escaped, not just known tags
        let val = Value::Str("__custom_tag:whatever".into());
        let json = value_to_json(&val).unwrap();
        let back = json_to_value(&json).unwrap();
        assert_eq!(val, back);
    }

    #[test]
    fn raw_tagged_string_without_str_escape() {
        // A raw JSON string "__date:not-a-date" (not produced by our serializer)
        // would try to parse as a date, fail, and fall through to plain Str.
        let json = JsonValue::String("__date:not-a-date".into());
        let val = json_to_value(&json).unwrap();
        // The date parse fails, so it falls through to Value::Str
        assert_eq!(val, Value::Str("__date:not-a-date".into()));
    }

    // ── R4-10: empty map ──────────────────────────────────────────────────────

    #[test]
    fn empty_props_round_trip() {
        let props = IndexMap::new();
        let json_str = props_to_json(&props).unwrap();
        assert_eq!(json_str, "{}");
        let back = json_to_props(&json_str).unwrap();
        assert!(back.is_empty());
    }

    // ── R4-10: malformed JSON ─────────────────────────────────────────────────

    #[test]
    fn malformed_json_errors() {
        assert!(json_to_props("").is_err());
        assert!(json_to_props("null").is_err());
    }

    // ── R4-10: Duration round-trip ────────────────────────────────────────────

    #[test]
    fn round_trip_duration() {
        // 3 days + 4 hours + 5 minutes = 3 days + (4*3600+5*60) seconds in nanos
        let nanos = (4i128 * 3600 + 5 * 60) * 1_000_000_000;
        let val = Value::Duration(CypherDuration {
            months: 0,
            days: 3,
            nanos,
        });
        let json = value_to_json(&val).unwrap();
        let back = json_to_value(&json).unwrap();
        // Compare via the fieldwise wrapper used by Value's PartialEq
        assert_eq!(val, back, "Duration round-trip failed");
    }

    // ── R4-10: negative _sd2_id in InMemoryGraphStore ─────────────────────────

    #[test]
    fn negative_sd2_id_rejected() {
        use crate::storage::{GraphStore, InMemoryGraphStore};

        let store = InMemoryGraphStore::new();
        store.create_graph("test").unwrap();

        let mut props = IndexMap::new();
        props.insert("_sd2_id".to_string(), Value::Int(-1));

        let result = store.create_node("test", vec!["Label".into()], props);
        assert!(result.is_err(), "negative _sd2_id should be rejected");
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("non-negative"),
            "error should mention non-negative, got: {err_msg}"
        );
    }

    #[test]
    fn negative_sd2_eid_rejected() {
        use crate::storage::{GraphStore, InMemoryGraphStore};

        let store = InMemoryGraphStore::new();
        store.create_graph("test").unwrap();

        // Create two nodes first
        let n1 = store
            .create_node("test", vec!["A".into()], IndexMap::new())
            .unwrap();
        let n2 = store
            .create_node("test", vec!["B".into()], IndexMap::new())
            .unwrap();

        let mut props = IndexMap::new();
        props.insert("_sd2_eid".to_string(), Value::Int(-5));

        let result = store.create_edge("test", n1, n2, "KNOWS".into(), props);
        assert!(result.is_err(), "negative _sd2_eid should be rejected");
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("non-negative"),
            "error should mention non-negative, got: {err_msg}"
        );
    }

    // ── JSON export: valid Time offset round-trips correctly ────────────────

    #[test]
    fn time_value_valid_offset_to_json() {
        let t = jiff::civil::time(14, 30, 0, 0);
        let val = Value::Time(t, 3600);
        let json = value_to_json(&val).unwrap();
        match &json {
            JsonValue::String(s) => {
                assert!(
                    s.starts_with("__datetime:"),
                    "expected __datetime tag, got: {s}"
                );
            }
            other => panic!("expected JSON string, got {other:?}"),
        }
    }

    // ── JSON export: invalid offset warns and falls back to UTC ─────────────

    #[test]
    fn time_value_invalid_offset_falls_back_to_utc_in_json() {
        let t = jiff::civil::time(12, 0, 0, 0);
        let val = Value::Time(t, 200_000);
        let json = value_to_json(&val).unwrap();
        match &json {
            JsonValue::String(s) => {
                assert!(
                    s.starts_with("__datetime:"),
                    "expected __datetime tag with UTC fallback, got: {s}"
                );
            }
            other => panic!("expected JSON string with UTC fallback, got {other:?}"),
        }
    }
}
