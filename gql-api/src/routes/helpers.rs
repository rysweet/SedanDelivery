//! Shared helper functions used across route modules.

use std::collections::HashMap;

use gql_core::storage::{EdgeId, GraphStore, NodeId};
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

/// Compute label counts and relationship-type counts for the given graph.
///
/// Label and relationship-type count maps returned by [`compute_schema_counts`].
pub(crate) type SchemaCounts = (HashMap<String, usize>, HashMap<String, usize>);

/// Returns `(label_counts, reltype_counts)` where each map associates a name
/// with the number of nodes/edges carrying that label/type.
pub(crate) fn compute_schema_counts(
    store: &dyn GraphStore,
    graph: &str,
) -> Result<SchemaCounts, ApiError> {
    // Label counts from all nodes.
    let all_node_ids = store.all_node_ids(graph).map_err(ApiError::from)?;
    let node_batch = store
        .get_nodes_batch(graph, &all_node_ids)
        .map_err(ApiError::from)?;
    let mut label_counts: HashMap<String, usize> = HashMap::new();
    for node in node_batch.into_iter().flatten() {
        for label in &node.labels {
            *label_counts.entry(label.clone()).or_insert(0) += 1;
        }
    }

    // Relationship-type counts from all edges.
    let all_edge_ids = store.all_edge_ids(graph).map_err(ApiError::from)?;
    let edge_batch = store
        .get_edges_batch(graph, &all_edge_ids)
        .map_err(ApiError::from)?;
    let mut reltype_counts: HashMap<String, usize> = HashMap::new();
    for edge in edge_batch.into_iter().flatten() {
        *reltype_counts.entry(edge.rel_type.clone()).or_insert(0) += 1;
    }

    Ok((label_counts, reltype_counts))
}
