//! Shared helper functions used across route modules.

use std::collections::HashMap;

use gql_core::storage::{EdgeId, GraphStore, NodeId};
use gql_core::types::Value;

use crate::error::ApiError;
use crate::state::AppState;

/// Verify that a graph exists in the store; return a 404 error if not.
pub fn require_graph(state: &AppState, name: &str) -> Result<(), ApiError> {
    if !state.store.graph_exists(name).map_err(ApiError::from)? {
        return Err(ApiError::not_found(format!("graph '{name}' not found")));
    }
    Ok(())
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use gql_core::storage::{EdgeId, InMemoryGraphStore, NodeId};
    use gql_core::types::Value;
    use serde_json::json;

    // ── parse_node_id ────────────────────────────────────────────────────────

    #[test]
    fn parse_node_id_valid() {
        let result = parse_node_id("42").unwrap();
        assert_eq!(result, NodeId(42));
    }

    #[test]
    fn parse_node_id_zero() {
        let result = parse_node_id("0").unwrap();
        assert_eq!(result, NodeId(0));
    }

    #[test]
    fn parse_node_id_large() {
        let result = parse_node_id("18446744073709551615").unwrap();
        assert_eq!(result, NodeId(u64::MAX));
    }

    #[test]
    fn parse_node_id_invalid_string() {
        let result = parse_node_id("abc");
        assert!(result.is_err());
    }

    #[test]
    fn parse_node_id_negative() {
        let result = parse_node_id("-1");
        assert!(result.is_err());
    }

    #[test]
    fn parse_node_id_empty() {
        let result = parse_node_id("");
        assert!(result.is_err());
    }

    #[test]
    fn parse_node_id_float_string() {
        let result = parse_node_id("1.5");
        assert!(result.is_err());
    }

    // ── parse_edge_id ────────────────────────────────────────────────────────

    #[test]
    fn parse_edge_id_valid() {
        let result = parse_edge_id("99").unwrap();
        assert_eq!(result, EdgeId(99));
    }

    #[test]
    fn parse_edge_id_zero() {
        let result = parse_edge_id("0").unwrap();
        assert_eq!(result, EdgeId(0));
    }

    #[test]
    fn parse_edge_id_invalid() {
        let result = parse_edge_id("not-a-number");
        assert!(result.is_err());
    }

    #[test]
    fn parse_edge_id_negative() {
        let result = parse_edge_id("-42");
        assert!(result.is_err());
    }

    #[test]
    fn parse_edge_id_overflow() {
        let result = parse_edge_id("18446744073709551616");
        assert!(result.is_err());
    }

    // ── json_to_value ────────────────────────────────────────────────────────

    #[test]
    fn json_to_value_null() {
        let result = json_to_value(json!(null)).unwrap();
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn json_to_value_bool_true() {
        let result = json_to_value(json!(true)).unwrap();
        assert_eq!(result, Value::Bool(true));
    }

    #[test]
    fn json_to_value_bool_false() {
        let result = json_to_value(json!(false)).unwrap();
        assert_eq!(result, Value::Bool(false));
    }

    #[test]
    fn json_to_value_integer() {
        let result = json_to_value(json!(42)).unwrap();
        assert_eq!(result, Value::Int(42));
    }

    #[test]
    fn json_to_value_negative_integer() {
        let result = json_to_value(json!(-7)).unwrap();
        assert_eq!(result, Value::Int(-7));
    }

    #[test]
    fn json_to_value_float() {
        let result = json_to_value(json!(3.14)).unwrap();
        assert_eq!(result, Value::Float(3.14));
    }

    #[test]
    fn json_to_value_string() {
        let result = json_to_value(json!("hello")).unwrap();
        assert_eq!(result, Value::Str("hello".into()));
    }

    #[test]
    fn json_to_value_empty_string() {
        let result = json_to_value(json!("")).unwrap();
        assert_eq!(result, Value::Str("".into()));
    }

    #[test]
    fn json_to_value_array() {
        let result = json_to_value(json!([1, "two", null])).unwrap();
        match result {
            Value::List(items) => {
                assert_eq!(items.len(), 3);
                assert_eq!(items[0], Value::Int(1));
                assert_eq!(items[1], Value::Str("two".into()));
                assert_eq!(items[2], Value::Null);
            }
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn json_to_value_empty_array() {
        let result = json_to_value(json!([])).unwrap();
        match result {
            Value::List(items) => assert!(items.is_empty()),
            other => panic!("expected empty List, got {other:?}"),
        }
    }

    #[test]
    fn json_to_value_object() {
        let result = json_to_value(json!({"name": "Alice", "age": 30})).unwrap();
        match result {
            Value::Map(m) => {
                assert_eq!(m.len(), 2);
                assert_eq!(m.get("name"), Some(&Value::Str("Alice".into())));
                assert_eq!(m.get("age"), Some(&Value::Int(30)));
            }
            other => panic!("expected Map, got {other:?}"),
        }
    }

    #[test]
    fn json_to_value_nested_object() {
        let result = json_to_value(json!({"outer": {"inner": 1}})).unwrap();
        match result {
            Value::Map(m) => {
                let inner = m.get("outer").unwrap();
                match inner {
                    Value::Map(inner_m) => {
                        assert_eq!(inner_m.get("inner"), Some(&Value::Int(1)));
                    }
                    other => panic!("expected nested Map, got {other:?}"),
                }
            }
            other => panic!("expected Map, got {other:?}"),
        }
    }

    // ── json_map_to_props ────────────────────────────────────────────────────

    #[test]
    fn json_map_to_props_empty() {
        let map = serde_json::Map::new();
        let result = json_map_to_props(map).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn json_map_to_props_mixed_types() {
        let mut map = serde_json::Map::new();
        map.insert("name".to_string(), json!("Alice"));
        map.insert("age".to_string(), json!(30));
        map.insert("active".to_string(), json!(true));
        map.insert("score".to_string(), json!(9.5));

        let result = json_map_to_props(map).unwrap();
        assert_eq!(result.len(), 4);
        assert_eq!(result.get("name"), Some(&Value::Str("Alice".into())));
        assert_eq!(result.get("age"), Some(&Value::Int(30)));
        assert_eq!(result.get("active"), Some(&Value::Bool(true)));
        assert_eq!(result.get("score"), Some(&Value::Float(9.5)));
    }

    #[test]
    fn json_map_to_props_with_array_value() {
        let mut map = serde_json::Map::new();
        map.insert("tags".to_string(), json!(["a", "b", "c"]));

        let result = json_map_to_props(map).unwrap();
        match result.get("tags") {
            Some(Value::List(items)) => {
                assert_eq!(items.len(), 3);
                assert_eq!(items[0], Value::Str("a".into()));
            }
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn json_map_to_props_with_nested_object() {
        let mut map = serde_json::Map::new();
        map.insert("meta".to_string(), json!({"key": "value"}));

        let result = json_map_to_props(map).unwrap();
        match result.get("meta") {
            Some(Value::Map(m)) => {
                assert_eq!(m.get("key"), Some(&Value::Str("value".into())));
            }
            other => panic!("expected Map, got {other:?}"),
        }
    }

    #[test]
    fn json_map_to_props_with_null_value() {
        let mut map = serde_json::Map::new();
        map.insert("deleted".to_string(), json!(null));

        let result = json_map_to_props(map).unwrap();
        assert_eq!(result.get("deleted"), Some(&Value::Null));
    }

    // ── compute_schema_counts ────────────────────────────────────────────────

    #[test]
    fn compute_schema_counts_empty_graph() {
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();

        let (labels, rels) = compute_schema_counts(&store, "g").unwrap();
        assert!(labels.is_empty());
        assert!(rels.is_empty());
    }

    #[test]
    fn compute_schema_counts_with_nodes_and_edges() {
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();

        let n1 = store
            .create_node("g", vec!["Person".to_string()], HashMap::new())
            .unwrap();
        let n2 = store
            .create_node(
                "g",
                vec!["Person".to_string(), "Employee".to_string()],
                HashMap::new(),
            )
            .unwrap();
        let n3 = store
            .create_node("g", vec!["Company".to_string()], HashMap::new())
            .unwrap();

        store
            .create_edge("g", n1, n2, "KNOWS".to_string(), HashMap::new())
            .unwrap();
        store
            .create_edge("g", n2, n3, "WORKS_AT".to_string(), HashMap::new())
            .unwrap();
        store
            .create_edge("g", n1, n3, "KNOWS".to_string(), HashMap::new())
            .unwrap();

        let (labels, rels) = compute_schema_counts(&store, "g").unwrap();

        assert_eq!(labels.get("Person"), Some(&2));
        assert_eq!(labels.get("Employee"), Some(&1));
        assert_eq!(labels.get("Company"), Some(&1));

        assert_eq!(rels.get("KNOWS"), Some(&2));
        assert_eq!(rels.get("WORKS_AT"), Some(&1));
    }

    #[test]
    fn compute_schema_counts_nonexistent_graph() {
        let store = InMemoryGraphStore::new();
        let result = compute_schema_counts(&store, "nonexistent");
        assert!(result.is_err());
    }
}
