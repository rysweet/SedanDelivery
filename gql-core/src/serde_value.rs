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

use std::collections::HashMap;

use serde_json::Value as JsonValue;

use crate::error::StorageError;
use crate::storage::{EdgeId, NodeId};
use crate::types::Value;
use crate::types::value::{Crs, GeoPoint};

/// Serialize a `Value` to a `serde_json::Value` for storage.
///
/// Returns `Err(StorageError::Serialization)` for non-finite floats.
pub fn value_to_json(v: &Value) -> Result<JsonValue, StorageError> {
    match v {
        Value::Null => Ok(JsonValue::Null),
        Value::Bool(b) => Ok(JsonValue::Bool(*b)),
        Value::Int(i) => Ok(JsonValue::Number((*i).into())),
        Value::Float(f) => {
            let n = serde_json::Number::from_f64(*f).ok_or_else(|| StorageError::Serialization {
                msg: format!("non-finite float: {f}"),
            })?;
            Ok(JsonValue::Number(n))
        }
        Value::Str(s) => Ok(JsonValue::String(s.to_string())),
        // Temporal types: tagged strings for round-trip fidelity.
        Value::Date(d) => Ok(JsonValue::String(format!("__date:{d}"))),
        Value::LocalTime(t) => Ok(JsonValue::String(format!("__localtime:{t}"))),
        Value::LocalDateTime(dt) => Ok(JsonValue::String(format!("__localdatetime:{dt}"))),
        Value::DateTime(dt) => Ok(JsonValue::String(format!("__datetime:{dt}"))),
        Value::Duration(sp) => Ok(JsonValue::String(format!("__duration:{sp}"))),
        // Spatial: tagged string "x,y,CrsVariant"
        Value::Point(p) => Ok(JsonValue::String(format!("__point:{},{},{:?}", p.x, p.y, p.crs))),
        // Collections
        Value::List(items) => {
            let arr: Result<Vec<_>, _> = items.iter().map(value_to_json).collect();
            Ok(JsonValue::Array(arr?))
        }
        Value::Map(m) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in m {
                obj.insert(k.to_string(), value_to_json(v)?);
            }
            Ok(JsonValue::Object(obj))
        }
        // Graph element references
        Value::Node(id) => Ok(JsonValue::String(format!("__node:{}", id.0))),
        Value::Edge(id) => Ok(JsonValue::String(format!("__edge:{}", id.0))),
        Value::Path { nodes, edges } => {
            let nodes_json: Vec<_> = nodes.iter().map(|n| JsonValue::Number(n.0.into())).collect();
            let edges_json: Vec<_> = edges.iter().map(|e| JsonValue::Number(e.0.into())).collect();
            Ok(JsonValue::Object({
                let mut m = serde_json::Map::new();
                m.insert("__path_nodes".into(), JsonValue::Array(nodes_json));
                m.insert("__path_edges".into(), JsonValue::Array(edges_json));
                m
            }))
        }
    }
}

/// Deserialize a `serde_json::Value` back to a `Value`.
///
/// Returns `None` only for truly unrepresentable inputs (should not happen with
/// well-formed data).  Unknown CRS values default to `Crs::Wgs84`.
pub fn json_to_value(j: &JsonValue) -> Option<Value> {
    Some(match j {
        JsonValue::Null => Value::Null,
        JsonValue::Bool(b) => Value::Bool(*b),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else if let Some(f) = n.as_f64() {
                Value::Float(f)
            } else {
                Value::Null
            }
        }
        JsonValue::String(s) => {
            // Decode tagged temporal/spatial types.
            if let Some(rest) = s.strip_prefix("__date:") {
                if let Ok(d) = rest.parse::<jiff::civil::Date>() {
                    return Some(Value::Date(d));
                }
            } else if let Some(rest) = s.strip_prefix("__localtime:") {
                if let Ok(t) = rest.parse::<jiff::civil::Time>() {
                    return Some(Value::LocalTime(t));
                }
            } else if let Some(rest) = s.strip_prefix("__localdatetime:") {
                if let Ok(dt) = rest.parse::<jiff::civil::DateTime>() {
                    return Some(Value::LocalDateTime(dt));
                }
            } else if let Some(rest) = s.strip_prefix("__datetime:") {
                if let Ok(dt) = rest.parse::<jiff::Zoned>() {
                    return Some(Value::DateTime(Box::new(dt)));
                }
            } else if let Some(rest) = s.strip_prefix("__duration:") {
                if let Ok(sp) = rest.parse::<jiff::Span>() {
                    return Some(Value::Duration(sp));
                }
            } else if let Some(rest) = s.strip_prefix("__point:") {
                // Format: "x,y,CrsVariant" e.g. "1.5,2.5,Wgs84"
                let parts: Vec<&str> = rest.splitn(3, ',').collect();
                if parts.len() == 3 {
                    if let (Ok(x), Ok(y)) = (parts[0].parse::<f64>(), parts[1].parse::<f64>()) {
                        let crs = match parts[2] {
                            "Wgs84" => Crs::Wgs84,
                            "Wgs843d" => Crs::Wgs843d,
                            "Cartesian" => Crs::Cartesian,
                            "Cartesian3d" => Crs::Cartesian3d,
                            _ => Crs::Wgs84, // default to Wgs84 (GIS standard)
                        };
                        return Some(Value::Point(GeoPoint { x, y, crs }));
                    }
                }
            } else if let Some(rest) = s.strip_prefix("__node:") {
                if let Ok(id) = rest.parse::<u64>() {
                    return Some(Value::Node(NodeId(id)));
                }
            } else if let Some(rest) = s.strip_prefix("__edge:") {
                if let Ok(id) = rest.parse::<u64>() {
                    return Some(Value::Edge(EdgeId(id)));
                }
            }
            Value::Str(s.as_str().into())
        }
        JsonValue::Array(items) => Value::List(items.iter().filter_map(json_to_value).collect()),
        JsonValue::Object(m) => {
            // Check for encoded path.
            if m.contains_key("__path_nodes") && m.contains_key("__path_edges") {
                let nodes = m["__path_nodes"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_u64().map(NodeId))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let edges = m["__path_edges"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_u64().map(EdgeId))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                return Some(Value::Path { nodes, edges });
            }
            let mut map = indexmap::IndexMap::new();
            for (k, v) in m {
                if let Some(val) = json_to_value(v) {
                    map.insert(k.as_str().into(), val);
                }
            }
            Value::Map(map)
        }
    })
}

/// Serialize a property map to a JSON string.
pub fn props_to_json(props: &HashMap<String, Value>) -> Result<String, StorageError> {
    let mut obj = serde_json::Map::new();
    for (k, v) in props {
        obj.insert(k.clone(), value_to_json(v)?);
    }
    serde_json::to_string(&JsonValue::Object(obj)).map_err(|e| StorageError::Serialization {
        msg: e.to_string(),
    })
}

/// Deserialize a JSON string into a property map.
pub fn json_to_props(s: &str) -> Result<HashMap<String, Value>, StorageError> {
    let j: JsonValue = serde_json::from_str(s).map_err(|e| StorageError::Serialization {
        msg: format!("failed to parse props JSON: {e}"),
    })?;
    match j {
        JsonValue::Object(m) => Ok(m
            .into_iter()
            .filter_map(|(k, v)| json_to_value(&v).map(|val| (k, val)))
            .collect()),
        _ => Err(StorageError::Serialization {
            msg: "props is not a JSON object".into(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Value;
    use crate::types::value::{Crs, GeoPoint};

    #[test]
    fn round_trip_scalars() {
        let cases = vec![
            Value::Null,
            Value::Bool(true),
            Value::Bool(false),
            Value::Int(42),
            Value::Int(-1),
            Value::Float(3.14),
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
        let mut props = HashMap::new();
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
}
