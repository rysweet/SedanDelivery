//! Scalar / property value serialisation.

use indexmap::IndexMap;
use serde_json::{json, Map, Value as Json};

use cypher_core::types::Value;

/// Serialise a single `cypher_core::types::Value` to JSON.
///
/// `Value::Node` and `Value::Edge` produce a reference object (`{"$type": "node", "id": N}`).
/// Callers that want fully resolved data should resolve via `resolve_value`.
pub fn value_to_json(v: &Value) -> Json {
    match v {
        Value::Null => Json::Null,
        Value::Bool(b) => Json::Bool(*b),
        Value::Int(i) => Json::Number((*i).into()),
        Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(Json::Number)
            .unwrap_or_else(|| {
                tracing::debug!(value = *f, "non-finite float coerced to JSON null");
                Json::Null
            }),
        Value::Str(s) => Json::String(s.to_string()),
        Value::Date(d) => Json::String(d.to_string()),
        Value::LocalTime(t) => Json::String(t.to_string()),
        Value::Time(t, offset) => {
            let sign = if *offset < 0 { "-" } else { "+" };
            let h = offset.abs() / 3600;
            let m = (offset.abs() % 3600) / 60;
            Json::String(format!("{}{}{:02}:{:02}", t, sign, h, m))
        }
        Value::LocalDateTime(dt) => Json::String(dt.to_string()),
        Value::DateTime(dt) => Json::String(dt.to_string()),
        Value::Duration(sp) => Json::String(cypher_core::types::value::format_cypher_duration(sp)),
        Value::Bytes(b) => {
            let hex: String = b.iter().map(|byte| format!("{:02x}", byte)).collect();
            Json::String(hex)
        }
        Value::Point(p) => json!({
            "x": p.x,
            "y": p.y,
            "crs": format!("{:?}", p.crs),
        }),
        Value::List(items) => Json::Array(items.iter().map(value_to_json).collect()),
        Value::Map(m) => {
            let obj: Map<String, Json> = m
                .iter()
                .map(|(k, v)| (k.to_string(), value_to_json(v)))
                .collect();
            Json::Object(obj)
        }
        Value::Node(id) => json!({ "$type": "node", "id": id.0 }),
        Value::Edge(id) => json!({ "$type": "edge", "id": id.0 }),
        Value::Path { nodes, edges } => json!({
            "$type": "path",
            "nodes": nodes.iter().map(|n| n.0).collect::<Vec<_>>(),
            "edges": edges.iter().map(|e| e.0).collect::<Vec<_>>(),
        }),
    }
}

/// Serialise a property map (`IndexMap<String, Value>`) to a JSON object.
pub fn props_to_json(props: &IndexMap<String, Value>) -> Json {
    let obj: Map<String, Json> = props
        .iter()
        .map(|(k, v)| (k.clone(), value_to_json(v)))
        .collect();
    Json::Object(obj)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cypher_core::storage::{EdgeId, NodeId};
    use cypher_core::types::value::{Crs, CypherDuration, GeoPoint};
    use indexmap::IndexMap;
    use serde_json::json;

    #[test]
    fn value_to_json_null() {
        assert_eq!(value_to_json(&Value::Null), Json::Null);
    }

    #[test]
    fn value_to_json_bool_true() {
        assert_eq!(value_to_json(&Value::Bool(true)), Json::Bool(true));
    }

    #[test]
    fn value_to_json_bool_false() {
        assert_eq!(value_to_json(&Value::Bool(false)), Json::Bool(false));
    }

    #[test]
    fn value_to_json_int_positive() {
        assert_eq!(value_to_json(&Value::Int(42)), json!(42));
    }

    #[test]
    fn value_to_json_int_negative() {
        assert_eq!(value_to_json(&Value::Int(-7)), json!(-7));
    }

    #[test]
    fn value_to_json_int_zero() {
        assert_eq!(value_to_json(&Value::Int(0)), json!(0));
    }

    #[test]
    fn value_to_json_float() {
        assert_eq!(
            value_to_json(&Value::Float(std::f64::consts::PI)),
            json!(std::f64::consts::PI)
        );
    }

    #[test]
    fn value_to_json_float_nan_becomes_null() {
        assert_eq!(value_to_json(&Value::Float(f64::NAN)), Json::Null);
    }

    #[test]
    fn value_to_json_float_infinity_becomes_null() {
        assert_eq!(value_to_json(&Value::Float(f64::INFINITY)), Json::Null);
    }

    #[test]
    fn value_to_json_string() {
        assert_eq!(
            value_to_json(&Value::Str("hello".into())),
            Json::String("hello".to_string())
        );
    }

    #[test]
    fn value_to_json_empty_string() {
        assert_eq!(
            value_to_json(&Value::Str("".into())),
            Json::String(String::new())
        );
    }

    #[test]
    fn value_to_json_date() {
        let d = jiff::civil::Date::new(2024, 6, 15).expect("valid date");
        let result = value_to_json(&Value::Date(d));
        assert!(result.is_string());
        assert_eq!(result.as_str().unwrap(), "2024-06-15");
    }

    #[test]
    fn value_to_json_local_time() {
        let t = jiff::civil::Time::new(10, 30, 45, 0).expect("valid time");
        let result = value_to_json(&Value::LocalTime(t));
        assert!(result.is_string());
        let s = result.as_str().unwrap();
        assert!(s.contains("10"), "should contain hour: {s}");
        assert!(s.contains("30"), "should contain minute: {s}");
    }

    #[test]
    fn value_to_json_time_with_positive_offset() {
        let t = jiff::civil::Time::new(14, 0, 0, 0).expect("valid time");
        let result = value_to_json(&Value::Time(t, 7200));
        assert!(result.is_string());
        let s = result.as_str().unwrap();
        assert!(s.contains("+02:00"), "should contain offset: {s}");
    }

    #[test]
    fn value_to_json_time_with_negative_offset() {
        let t = jiff::civil::Time::new(9, 0, 0, 0).expect("valid time");
        let result = value_to_json(&Value::Time(t, -18000));
        assert!(result.is_string());
        let s = result.as_str().unwrap();
        assert!(s.contains("-05:00"), "should contain negative offset: {s}");
    }

    #[test]
    fn value_to_json_local_datetime() {
        let dt = jiff::civil::DateTime::new(2024, 1, 15, 9, 30, 0, 0).expect("valid");
        let result = value_to_json(&Value::LocalDateTime(dt));
        assert!(result.is_string());
        let s = result.as_str().unwrap();
        assert!(s.contains("2024"), "should contain year: {s}");
    }

    #[test]
    fn value_to_json_datetime_zoned() {
        let tz = jiff::tz::TimeZone::UTC;
        let zdt = jiff::civil::DateTime::new(2024, 6, 15, 12, 0, 0, 0)
            .expect("valid")
            .to_zoned(tz)
            .expect("valid");
        let result = value_to_json(&Value::DateTime(Box::new(zdt)));
        assert!(result.is_string());
    }

    #[test]
    fn value_to_json_duration() {
        let d = CypherDuration {
            months: 14,
            days: 3,
            nanos: 0,
        };
        let result = value_to_json(&Value::Duration(d));
        assert!(result.is_string());
        let s = result.as_str().unwrap();
        assert!(s.starts_with('P'), "duration should start with P: {s}");
    }

    #[test]
    fn value_to_json_bytes() {
        let result = value_to_json(&Value::Bytes(vec![0xde, 0xad, 0xbe, 0xef]));
        assert_eq!(result, Json::String("deadbeef".to_string()));
    }

    #[test]
    fn value_to_json_point() {
        let p = GeoPoint {
            x: 1.5,
            y: 2.5,
            z: None,
            crs: Crs::Wgs84,
        };
        let result = value_to_json(&Value::Point(p));
        assert!(result.is_object());
        assert_eq!(result["x"], json!(1.5));
        assert_eq!(result["y"], json!(2.5));
        assert!(result["crs"].is_string());
    }

    #[test]
    fn value_to_json_list() {
        let list = Value::List(vec![Value::Int(1), Value::Bool(true), Value::Null]);
        let result = value_to_json(&list);
        assert!(result.is_array());
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0], json!(1));
        assert_eq!(arr[1], json!(true));
        assert_eq!(arr[2], Json::Null);
    }

    #[test]
    fn value_to_json_empty_list() {
        let result = value_to_json(&Value::List(vec![]));
        assert_eq!(result, json!([]));
    }

    #[test]
    fn value_to_json_map() {
        let mut m = IndexMap::new();
        m.insert(Box::<str>::from("name"), Value::Str("Alice".into()));
        m.insert(Box::<str>::from("age"), Value::Int(30));
        let result = value_to_json(&Value::Map(m));
        assert!(result.is_object());
        assert_eq!(result["name"], json!("Alice"));
        assert_eq!(result["age"], json!(30));
    }

    #[test]
    fn value_to_json_empty_map() {
        let m: IndexMap<Box<str>, Value> = IndexMap::new();
        let result = value_to_json(&Value::Map(m));
        assert_eq!(result, json!({}));
    }

    #[test]
    fn value_to_json_node_reference() {
        let result = value_to_json(&Value::Node(NodeId(42)));
        assert_eq!(result["$type"], json!("node"));
        assert_eq!(result["id"], json!(42));
    }

    #[test]
    fn value_to_json_edge_reference() {
        let result = value_to_json(&Value::Edge(EdgeId(99)));
        assert_eq!(result["$type"], json!("edge"));
        assert_eq!(result["id"], json!(99));
    }

    #[test]
    fn value_to_json_path() {
        let result = value_to_json(&Value::Path {
            nodes: vec![NodeId(1), NodeId(2), NodeId(3)],
            edges: vec![EdgeId(10), EdgeId(20)],
        });
        assert_eq!(result["$type"], json!("path"));
        assert_eq!(result["nodes"], json!([1, 2, 3]));
        assert_eq!(result["edges"], json!([10, 20]));
    }

    #[test]
    fn props_to_json_empty() {
        let props: IndexMap<String, Value> = IndexMap::new();
        assert_eq!(props_to_json(&props), json!({}));
    }

    #[test]
    fn props_to_json_mixed_types() {
        let mut props = IndexMap::new();
        props.insert("name".to_string(), Value::Str("Alice".into()));
        props.insert("age".to_string(), Value::Int(30));
        props.insert("active".to_string(), Value::Bool(true));

        let result = props_to_json(&props);
        assert!(result.is_object());
        assert_eq!(result["name"], json!("Alice"));
        assert_eq!(result["age"], json!(30));
        assert_eq!(result["active"], json!(true));
    }

    #[test]
    fn props_to_json_nested_list() {
        let mut props = IndexMap::new();
        props.insert(
            "tags".to_string(),
            Value::List(vec![Value::Str("a".into()), Value::Str("b".into())]),
        );
        let result = props_to_json(&props);
        assert_eq!(result["tags"], json!(["a", "b"]));
    }
}
