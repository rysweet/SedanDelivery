//! Shared helper functions used across route modules.

use std::collections::HashMap;

use gql_core::storage::{EdgeId, NodeId};
use gql_core::types::Value;

use crate::error::ApiError;

/// Parse a URL path segment as a `NodeId`.
pub fn parse_node_id(s: &str) -> Result<NodeId, ApiError> {
    s.parse::<u64>()
        .map(NodeId)
        .map_err(|_| ApiError::bad_request(format!("invalid node id: '{s}'")))
}

/// Parse a URL path segment as an `EdgeId`.
pub fn parse_edge_id(s: &str) -> Result<EdgeId, ApiError> {
    s.parse::<u64>()
        .map(EdgeId)
        .map_err(|_| ApiError::bad_request(format!("invalid edge id: '{s}'")))
}

/// Convert a `serde_json::Value` into a `gql_core::types::Value`.
///
/// JSON numbers become `Value::Int` if they are integers, `Value::Float` otherwise.
/// Nested objects become `Value::Map`, arrays become `Value::List`.
/// `null` becomes `Value::Null`.
pub fn json_to_value(v: serde_json::Value) -> Result<Value, ApiError> {
    use indexmap::IndexMap;
    match v {
        serde_json::Value::Null => Ok(Value::Null),
        serde_json::Value::Bool(b) => Ok(Value::Bool(b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(Value::Int(i))
            } else if let Some(f) = n.as_f64() {
                Ok(Value::Float(f))
            } else {
                Err(ApiError::bad_request(format!(
                    "numeric value '{n}' out of range"
                )))
            }
        }
        serde_json::Value::String(s) => Ok(Value::Str(s.into_boxed_str())),
        serde_json::Value::Array(items) => {
            let vals: Result<Vec<Value>, ApiError> = items.into_iter().map(json_to_value).collect();
            Ok(Value::List(vals?))
        }
        serde_json::Value::Object(obj) => {
            let mut m: IndexMap<Box<str>, Value> = IndexMap::new();
            for (k, val) in obj {
                m.insert(k.into_boxed_str(), json_to_value(val)?);
            }
            Ok(Value::Map(m))
        }
    }
}

/// Convert a `serde_json::Map` of property values into a `HashMap<String, Value>`.
pub fn json_map_to_props(
    map: serde_json::Map<String, serde_json::Value>,
) -> Result<HashMap<String, Value>, ApiError> {
    map.into_iter()
        .map(|(k, v)| {
            let val = json_to_value(v)?;
            Ok((k, val))
        })
        .collect()
}
