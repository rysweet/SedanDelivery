//! `Value` → `serde_json::Value` encoding.
//!
//! Tagged-string encoding scheme is documented at the module root.

use indexmap::IndexMap;
use serde_json::Value as JsonValue;

use crate::error::StorageError;
use crate::types::value::format_cypher_duration;
use crate::types::Value;

/// Serialize a `Value` to a `serde_json::Value` for storage.
///
/// Returns `Err(StorageError::Serialization)` for non-finite floats.
pub fn value_to_json(v: &Value) -> Result<JsonValue, StorageError> {
    match v {
        Value::Null => Ok(JsonValue::Null),
        Value::Bool(b) => Ok(JsonValue::Bool(*b)),
        Value::Int(i) => Ok(JsonValue::Number((*i).into())),
        Value::Float(f) => {
            let n =
                serde_json::Number::from_f64(*f).ok_or_else(|| StorageError::Serialization {
                    msg: format!("non-finite float: {f}"),
                })?;
            Ok(JsonValue::Number(n))
        }
        Value::Str(s) => {
            let s_ref = s.as_ref();
            if s_ref.starts_with("__") {
                Ok(JsonValue::String(format!("__str:{s_ref}")))
            } else {
                Ok(JsonValue::String(s_ref.to_string()))
            }
        }
        // Temporal types: tagged strings for round-trip fidelity.
        Value::Date(d) => Ok(JsonValue::String(format!("__date:{d}"))),
        Value::LocalTime(t) => Ok(JsonValue::String(format!("__localtime:{t}"))),
        Value::Time(t, offset) => {
            // Serialize zoned time as a tagged datetime at epoch for round-trip.
            let offset_obj = match jiff::tz::Offset::from_seconds(*offset) {
                Ok(o) => o,
                Err(_) => {
                    tracing::warn!(
                        offset_secs = offset,
                        "timezone offset out of range during JSON export, falling back to UTC"
                    );
                    jiff::tz::Offset::UTC
                }
            };
            let tz = jiff::tz::TimeZone::fixed(offset_obj);
            // safe: constant epoch date 1970-01-01 is always valid
            let epoch = jiff::civil::Date::new(1970, 1, 1).expect("valid epoch");
            let dt = epoch.to_datetime(*t);
            match dt.to_zoned(tz) {
                Ok(zoned) => Ok(JsonValue::String(format!("__datetime:{zoned}"))),
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "failed to zone Time value during JSON export, returning Null"
                    );
                    Ok(JsonValue::Null)
                }
            }
        }
        Value::LocalDateTime(dt) => Ok(JsonValue::String(format!("__localdatetime:{dt}"))),
        Value::DateTime(dt) => Ok(JsonValue::String(format!("__datetime:{dt}"))),
        Value::Duration(d) => Ok(JsonValue::String(format!(
            "__duration:{}",
            format_cypher_duration(d)
        ))),
        // Spatial: tagged string "x,y[,z],CrsVariant"
        Value::Point(p) => {
            if let Some(z) = p.z {
                Ok(JsonValue::String(format!(
                    "__point:{},{},{},{:?}",
                    p.x, p.y, z, p.crs
                )))
            } else {
                Ok(JsonValue::String(format!(
                    "__point:{},{},{:?}",
                    p.x, p.y, p.crs
                )))
            }
        }
        // Bytes: hex-encoded tagged string for round-trip fidelity.
        Value::Bytes(b) => {
            let hex: String = b.iter().map(|byte| format!("{:02x}", byte)).collect();
            Ok(JsonValue::String(format!("__bytes:{hex}")))
        }
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
            let nodes_json: Vec<_> = nodes
                .iter()
                .map(|n| JsonValue::Number(n.0.into()))
                .collect();
            let edges_json: Vec<_> = edges
                .iter()
                .map(|e| JsonValue::Number(e.0.into()))
                .collect();
            Ok(JsonValue::Object({
                let mut m = serde_json::Map::new();
                m.insert("__path_nodes".into(), JsonValue::Array(nodes_json));
                m.insert("__path_edges".into(), JsonValue::Array(edges_json));
                m
            }))
        }
    }
}

/// Serialize a property map to a JSON string.
pub fn props_to_json(props: &IndexMap<String, Value>) -> Result<String, StorageError> {
    let mut obj = serde_json::Map::new();
    for (k, v) in props {
        obj.insert(k.clone(), value_to_json(v)?);
    }
    serde_json::to_string(&JsonValue::Object(obj))
        .map_err(|e| StorageError::Serialization { msg: e.to_string() })
}
