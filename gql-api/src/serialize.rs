//! Serialisation helpers: convert gql-core types to `serde_json::Value`.

use std::collections::HashMap;

use serde_json::{json, Map, Value as Json};

use gql_core::storage::{EdgeData, EdgeId, GraphStore, NodeData, NodeId};
use gql_core::types::Value;

// ── Scalar / property value ───────────────────────────────────────────────────

/// Serialise a single `gql_core::types::Value` to JSON.
///
/// `Value::Node` and `Value::Edge` produce a reference object (`{"$type": "node", "id": N}`).
/// Callers that want fully resolved data should resolve via `resolve_value`.
pub fn value_to_json(v: &Value) -> Json {
    match v {
        Value::Null => Json::Null,
        Value::Bool(b) => Json::Bool(*b),
        Value::Int(i) => Json::Number((*i).into()),
        Value::Float(f) => {
            serde_json::Number::from_f64(*f)
                .map(Json::Number)
                .unwrap_or(Json::Null)
        }
        Value::Str(s) => Json::String(s.to_string()),
        Value::Date(d) => Json::String(d.to_string()),
        Value::LocalTime(t) => Json::String(t.to_string()),
        Value::LocalDateTime(dt) => Json::String(dt.to_string()),
        Value::DateTime(dt) => Json::String(dt.to_string()),
        Value::Duration(sp) => Json::String(gql_core::types::value::format_cypher_duration(sp)),
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

/// Serialise a property map (`HashMap<String, Value>`) to a JSON object.
pub fn props_to_json(props: &HashMap<String, Value>) -> Json {
    let obj: Map<String, Json> = props
        .iter()
        .map(|(k, v)| (k.clone(), value_to_json(v)))
        .collect();
    Json::Object(obj)
}

// ── Node / edge wire representations ─────────────────────────────────────────

/// Wire representation of a node returned by the API.
pub fn node_to_json(node: &NodeData) -> Json {
    json!({
        "id":         node.id.0.to_string(),
        "labels":     node.labels,
        "properties": props_to_json(&node.properties),
    })
}

/// Wire representation of an edge returned by the API.
pub fn edge_to_json(edge: &EdgeData) -> Json {
    json!({
        "id":         edge.id.0.to_string(),
        "type":       edge.rel_type,
        "source":     edge.src.0.to_string(),
        "target":     edge.dst.0.to_string(),
        "properties": props_to_json(&edge.properties),
    })
}

// ── Query result row serialisation ───────────────────────────────────────────

/// Serialise a query result row, resolving `Value::Node` / `Value::Edge`
/// references against the store.
///
/// Resolved nodes become `{"type":"node","id":...,"labels":[...],"properties":{...}}`.
/// Resolved edges become `{"type":"edge","id":...,"type":...,source,target,properties}`.
/// If the store cannot resolve an id (race condition), a reference object is
/// emitted instead of returning an error.
pub fn row_to_json(
    row: &HashMap<String, Value>,
    columns: &[String],
    store: &dyn GraphStore,
    graph: &str,
) -> Json {
    let mut obj = Map::new();
    for col in columns {
        let v = row.get(col).unwrap_or(&Value::Null);
        obj.insert(col.clone(), resolve_value(v, store, graph));
    }
    Json::Object(obj)
}

/// Recursively resolve a `Value` against the store.
fn resolve_value(v: &Value, store: &dyn GraphStore, graph: &str) -> Json {
    match v {
        Value::Node(id) => resolve_node(*id, store, graph),
        Value::Edge(id) => resolve_edge(*id, store, graph),
        Value::List(items) => {
            Json::Array(items.iter().map(|i| resolve_value(i, store, graph)).collect())
        }
        Value::Map(m) => {
            let obj: Map<String, Json> = m
                .iter()
                .map(|(k, val)| (k.to_string(), resolve_value(val, store, graph)))
                .collect();
            Json::Object(obj)
        }
        Value::Path { nodes, edges } => {
            let resolved_nodes: Vec<Json> = nodes
                .iter()
                .map(|nid| resolve_node(*nid, store, graph))
                .collect();
            let resolved_edges: Vec<Json> = edges
                .iter()
                .map(|eid| resolve_edge(*eid, store, graph))
                .collect();
            json!({
                "type":  "path",
                "nodes": resolved_nodes,
                "edges": resolved_edges,
            })
        }
        other => value_to_json(other),
    }
}

fn resolve_node(id: NodeId, store: &dyn GraphStore, graph: &str) -> Json {
    match store.get_node(graph, id) {
        Ok(Some(node)) => json!({
            "type":       "node",
            "id":         node.id.0.to_string(),
            "labels":     node.labels,
            "properties": props_to_json(&node.properties),
        }),
        _ => json!({ "$type": "node", "id": id.0 }),
    }
}

fn resolve_edge(id: EdgeId, store: &dyn GraphStore, graph: &str) -> Json {
    match store.get_edge(graph, id) {
        Ok(Some(edge)) => json!({
            "type":       edge.rel_type,
            "id":         edge.id.0.to_string(),
            "source":     edge.src.0.to_string(),
            "target":     edge.dst.0.to_string(),
            "properties": props_to_json(&edge.properties),
        }),
        _ => json!({ "$type": "edge", "id": id.0 }),
    }
}
