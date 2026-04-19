//! Query result row serialisation with node/edge resolution.

use std::collections::HashMap;

use serde_json::{json, Map, Value as Json};

use cypher_core::storage::{EdgeId, GraphStore, NodeId};
use cypher_core::types::Value;

use super::value::{props_to_json, value_to_json};

/// Serialise a query result row, resolving `Value::Node` / `Value::Edge`
/// references against the store.
///
/// Resolved nodes become `{"type":"node","id":...,"labels":[...],"properties":{...}}`.
/// Resolved edges become `{"type":"edge","id":...,"type":...,source,target,properties}`.
///
/// Returns an error if any referenced node or edge cannot be resolved from the store.
pub fn row_to_json(
    row: &HashMap<String, Value>,
    columns: &[String],
    store: &dyn GraphStore,
    graph: &str,
) -> Result<Json, String> {
    let mut obj = Map::new();
    for col in columns {
        let v = row.get(col).unwrap_or(&Value::Null);
        obj.insert(col.clone(), resolve_value(v, store, graph)?);
    }
    Ok(Json::Object(obj))
}

/// Recursively resolve a `Value` against the store.
fn resolve_value(v: &Value, store: &dyn GraphStore, graph: &str) -> Result<Json, String> {
    match v {
        Value::Node(id) => resolve_node(*id, store, graph),
        Value::Edge(id) => resolve_edge(*id, store, graph),
        Value::List(items) => {
            let resolved: Result<Vec<Json>, String> = items
                .iter()
                .map(|i| resolve_value(i, store, graph))
                .collect();
            Ok(Json::Array(resolved?))
        }
        Value::Map(m) => {
            let mut obj = Map::new();
            for (k, val) in m {
                obj.insert(k.to_string(), resolve_value(val, store, graph)?);
            }
            Ok(Json::Object(obj))
        }
        Value::Path { nodes, edges } => {
            let resolved_nodes: Result<Vec<Json>, String> = nodes
                .iter()
                .map(|nid| resolve_node(*nid, store, graph))
                .collect();
            let resolved_edges: Result<Vec<Json>, String> = edges
                .iter()
                .map(|eid| resolve_edge(*eid, store, graph))
                .collect();
            Ok(json!({
                "type":  "path",
                "nodes": resolved_nodes?,
                "edges": resolved_edges?,
            }))
        }
        other => Ok(value_to_json(other)),
    }
}

fn resolve_node(id: NodeId, store: &dyn GraphStore, graph: &str) -> Result<Json, String> {
    match store.get_node(graph, id) {
        Ok(Some(node)) => Ok(json!({
            "type":       "node",
            "id":         node.id.0.to_string(),
            "labels":     node.labels,
            "properties": props_to_json(&node.properties),
        })),
        Err(e) => {
            tracing::error!(node_id = id.0, error = %e, "failed to resolve node from store");
            Err(format!("failed to resolve node {}: {e}", id.0))
        }
        Ok(None) => {
            tracing::error!(
                node_id = id.0,
                "node not found in store during serialization"
            );
            Err(format!("node {} not found in store", id.0))
        }
    }
}

fn resolve_edge(id: EdgeId, store: &dyn GraphStore, graph: &str) -> Result<Json, String> {
    match store.get_edge(graph, id) {
        Ok(Some(edge)) => Ok(json!({
            "type":       edge.rel_type,
            "id":         edge.id.0.to_string(),
            "source":     edge.src.0.to_string(),
            "target":     edge.dst.0.to_string(),
            "properties": props_to_json(&edge.properties),
        })),
        Err(e) => {
            tracing::error!(edge_id = id.0, error = %e, "failed to resolve edge from store");
            Err(format!("failed to resolve edge {}: {e}", id.0))
        }
        Ok(None) => {
            tracing::error!(
                edge_id = id.0,
                "edge not found in store during serialization"
            );
            Err(format!("edge {} not found in store", id.0))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cypher_core::storage::{InMemoryGraphStore, NodeId};
    use cypher_core::types::Value;
    use indexmap::IndexMap;
    use serde_json::json;
    use std::collections::HashMap;

    #[test]
    fn row_to_json_scalar_values() {
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();

        let mut row = HashMap::new();
        row.insert("x".to_string(), Value::Int(1));
        row.insert("y".to_string(), Value::Str("hello".into()));

        let columns = vec!["x".to_string(), "y".to_string()];
        let result = row_to_json(&row, &columns, &store, "g").unwrap();

        assert_eq!(result["x"], json!(1));
        assert_eq!(result["y"], json!("hello"));
    }

    #[test]
    fn row_to_json_missing_column_becomes_null() {
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();

        let row = HashMap::new();
        let columns = vec!["missing".to_string()];
        let result = row_to_json(&row, &columns, &store, "g").unwrap();

        assert_eq!(result["missing"], Json::Null);
    }

    #[test]
    fn row_to_json_resolves_node_reference() {
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();
        let node_id = store
            .create_node("g", vec!["Person".to_string()], {
                let mut p = IndexMap::new();
                p.insert("name".to_string(), Value::Str("Alice".into()));
                p
            })
            .unwrap();

        let mut row = HashMap::new();
        row.insert("n".to_string(), Value::Node(node_id));

        let columns = vec!["n".to_string()];
        let result = row_to_json(&row, &columns, &store, "g").unwrap();

        assert_eq!(result["n"]["type"], json!("node"));
        assert_eq!(result["n"]["labels"], json!(["Person"]));
        assert_eq!(result["n"]["properties"]["name"], json!("Alice"));
    }

    #[test]
    fn row_to_json_resolves_edge_reference() {
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();
        let n1 = store.create_node("g", vec![], IndexMap::new()).unwrap();
        let n2 = store.create_node("g", vec![], IndexMap::new()).unwrap();
        let edge_id = store
            .create_edge("g", n1, n2, "KNOWS".to_string(), IndexMap::new())
            .unwrap();

        let mut row = HashMap::new();
        row.insert("r".to_string(), Value::Edge(edge_id));

        let columns = vec!["r".to_string()];
        let result = row_to_json(&row, &columns, &store, "g").unwrap();

        assert_eq!(result["r"]["type"], json!("KNOWS"));
    }

    #[test]
    fn row_to_json_unresolvable_node_returns_error() {
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();

        let mut row = HashMap::new();
        row.insert("n".to_string(), Value::Node(NodeId(9999)));

        let columns = vec!["n".to_string()];
        let result = row_to_json(&row, &columns, &store, "g");

        assert!(result.is_err(), "unresolvable node must return an error");
        let err = result.unwrap_err();
        assert!(
            err.contains("9999"),
            "error should mention the node id: {err}"
        );
    }

    #[test]
    fn row_to_json_resolves_path() {
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();
        let n1 = store
            .create_node("g", vec!["A".to_string()], IndexMap::new())
            .unwrap();
        let n2 = store
            .create_node("g", vec!["B".to_string()], IndexMap::new())
            .unwrap();
        let e = store
            .create_edge("g", n1, n2, "REL".to_string(), IndexMap::new())
            .unwrap();

        let mut row = HashMap::new();
        row.insert(
            "p".to_string(),
            Value::Path {
                nodes: vec![n1, n2],
                edges: vec![e],
            },
        );

        let columns = vec!["p".to_string()];
        let result = row_to_json(&row, &columns, &store, "g").unwrap();

        assert_eq!(result["p"]["type"], json!("path"));
        assert!(result["p"]["nodes"].is_array());
        assert!(result["p"]["edges"].is_array());
    }

    #[test]
    fn row_to_json_resolves_nested_list() {
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();
        let nid = store
            .create_node("g", vec!["X".to_string()], IndexMap::new())
            .unwrap();

        let mut row = HashMap::new();
        row.insert(
            "items".to_string(),
            Value::List(vec![Value::Node(nid), Value::Int(42)]),
        );

        let columns = vec!["items".to_string()];
        let result = row_to_json(&row, &columns, &store, "g").unwrap();

        let arr = result["items"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["type"], json!("node"));
        assert_eq!(arr[1], json!(42));
    }
}
