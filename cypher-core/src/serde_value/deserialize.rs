//! `serde_json::Value` → `Value` decoding, including tagged-string parsing.

use indexmap::IndexMap;
use serde_json::Value as JsonValue;

use crate::error::StorageError;
use crate::storage::{EdgeId, NodeId};
use crate::types::value::{Crs, CypherDuration, GeoPoint};
use crate::types::Value;

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
        JsonValue::String(s) => decode_tagged_string(s),
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

/// Decode a JSON string, recognising the tagged-string encoding scheme
/// (e.g. `__date:`, `__point:`, `__bytes:`). Strings that do not match a
/// known tag — or whose tag payload fails to parse — fall through to
/// `Value::Str`.
fn decode_tagged_string(s: &str) -> Value {
    // Escaped plain string — must be checked first to prevent
    // user strings starting with __ from being misinterpreted as tags.
    if let Some(rest) = s.strip_prefix("__str:") {
        return Value::Str(rest.into());
    }
    // Decode tagged temporal/spatial types.
    if let Some(rest) = s.strip_prefix("__date:") {
        if let Ok(d) = rest.parse::<jiff::civil::Date>() {
            return Value::Date(d);
        }
    } else if let Some(rest) = s.strip_prefix("__localtime:") {
        if let Ok(t) = rest.parse::<jiff::civil::Time>() {
            return Value::LocalTime(t);
        }
    } else if let Some(rest) = s.strip_prefix("__localdatetime:") {
        if let Ok(dt) = rest.parse::<jiff::civil::DateTime>() {
            return Value::LocalDateTime(dt);
        }
    } else if let Some(rest) = s.strip_prefix("__datetime:") {
        if let Ok(dt) = rest.parse::<jiff::Zoned>() {
            return Value::DateTime(Box::new(dt));
        }
    } else if let Some(rest) = s.strip_prefix("__duration:") {
        if let Some(v) = parse_duration_payload(rest) {
            return v;
        }
    } else if let Some(rest) = s.strip_prefix("__point:") {
        if let Some(v) = parse_point_payload(rest) {
            return v;
        }
    } else if let Some(rest) = s.strip_prefix("__bytes:") {
        if let Some(v) = parse_bytes_payload(rest) {
            return v;
        }
    } else if let Some(rest) = s.strip_prefix("__node:") {
        if let Ok(id) = rest.parse::<u64>() {
            return Value::Node(NodeId(id));
        }
    } else if let Some(rest) = s.strip_prefix("__edge:") {
        if let Ok(id) = rest.parse::<u64>() {
            return Value::Edge(EdgeId(id));
        }
    }
    Value::Str(s.into())
}

fn parse_duration_payload(rest: &str) -> Option<Value> {
    let sp = rest.parse::<jiff::Span>().ok()?;
    let total_months = sp.get_years() as i64 * 12 + sp.get_months() as i64;
    let total_days = sp.get_weeks() as i64 * 7 + sp.get_days() as i64;
    let time_secs = sp.get_hours() as i64 * 3600 + sp.get_minutes() * 60 + sp.get_seconds();
    let total_nanos = time_secs as i128 * 1_000_000_000
        + sp.get_milliseconds() as i128 * 1_000_000
        + sp.get_microseconds() as i128 * 1_000
        + sp.get_nanoseconds() as i128;
    Some(Value::Duration(CypherDuration {
        months: total_months,
        days: total_days,
        nanos: total_nanos,
    }))
}

fn parse_point_payload(rest: &str) -> Option<Value> {
    // Format: "x,y,CrsVariant" or "x,y,z,CrsVariant"
    let parts: Vec<&str> = rest.splitn(4, ',').collect();
    if parts.len() == 3 {
        let (x, y) = (parts[0].parse::<f64>().ok()?, parts[1].parse::<f64>().ok()?);
        let crs = parse_crs(parts[2]);
        return Some(Value::Point(GeoPoint { x, y, z: None, crs }));
    } else if parts.len() == 4 {
        let x = parts[0].parse::<f64>().ok()?;
        let y = parts[1].parse::<f64>().ok()?;
        let z = parts[2].parse::<f64>().ok()?;
        let crs = parse_crs(parts[3]);
        return Some(Value::Point(GeoPoint {
            x,
            y,
            z: Some(z),
            crs,
        }));
    }
    None
}

fn parse_crs(name: &str) -> Crs {
    match name {
        "Wgs84" => Crs::Wgs84,
        "Wgs843d" => Crs::Wgs843d,
        "Cartesian" => Crs::Cartesian,
        "Cartesian3d" => Crs::Cartesian3d,
        _ => Crs::Wgs84,
    }
}

fn parse_bytes_payload(rest: &str) -> Option<Value> {
    // Decode hex pairs — odd-length strings are not valid hex byte
    // sequences, so fall through to Value::Str rather than panicking.
    if !rest.len().is_multiple_of(2) {
        return None;
    }
    let bytes: Result<Vec<u8>, _> = (0..rest.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&rest[i..i + 2], 16))
        .collect();
    bytes.ok().map(Value::Bytes)
}

/// Deserialize a JSON string into a property map.
pub fn json_to_props(s: &str) -> Result<IndexMap<String, Value>, StorageError> {
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
