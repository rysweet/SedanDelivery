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
        Value::Duration(sp) => Json::String(gql_core::types::value::format_cypher_duration(sp)),
        Value::Bytes(b) => {
            // Hex-encode bytes for JSON output
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
            tracing::error!(node_id = id.0, "node not found in store during serialization");
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
            tracing::error!(edge_id = id.0, "edge not found in store during serialization");
            Err(format!("edge {} not found in store", id.0))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gql_core::storage::{EdgeData, EdgeId, InMemoryGraphStore, NodeData, NodeId};
    use gql_core::types::value::{Crs, CypherDuration, GeoPoint};
    use gql_core::types::Value;
    use indexmap::IndexMap;
    use serde_json::json;
    use std::collections::HashMap;

    // ── value_to_json ────────────────────────────────────────────────────────

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
        assert_eq!(value_to_json(&Value::Float(3.14)), json!(3.14));
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
        let result = value_to_json(&Value::Time(t, 7200)); // +02:00
        assert!(result.is_string());
        let s = result.as_str().unwrap();
        assert!(s.contains("+02:00"), "should contain offset: {s}");
    }

    #[test]
    fn value_to_json_time_with_negative_offset() {
        let t = jiff::civil::Time::new(9, 0, 0, 0).expect("valid time");
        let result = value_to_json(&Value::Time(t, -18000)); // -05:00
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

    // ── props_to_json ────────────────────────────────────────────────────────

    #[test]
    fn props_to_json_empty() {
        let props: HashMap<String, Value> = HashMap::new();
        assert_eq!(props_to_json(&props), json!({}));
    }

    #[test]
    fn props_to_json_mixed_types() {
        let mut props = HashMap::new();
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
        let mut props = HashMap::new();
        props.insert(
            "tags".to_string(),
            Value::List(vec![Value::Str("a".into()), Value::Str("b".into())]),
        );
        let result = props_to_json(&props);
        assert_eq!(result["tags"], json!(["a", "b"]));
    }

    // ── node_to_json ─────────────────────────────────────────────────────────

    #[test]
    fn node_to_json_basic() {
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::Str("Bob".into()));
        let node = NodeData {
            id: NodeId(7),
            labels: vec!["Person".to_string(), "Employee".to_string()],
            properties: props,
        };
        let result = node_to_json(&node);
        assert_eq!(result["id"], json!("7"));
        assert_eq!(result["labels"], json!(["Person", "Employee"]));
        assert_eq!(result["properties"]["name"], json!("Bob"));
    }

    #[test]
    fn node_to_json_no_labels_no_props() {
        let node = NodeData {
            id: NodeId(0),
            labels: vec![],
            properties: HashMap::new(),
        };
        let result = node_to_json(&node);
        assert_eq!(result["id"], json!("0"));
        assert_eq!(result["labels"], json!([]));
        assert_eq!(result["properties"], json!({}));
    }

    // ── edge_to_json ─────────────────────────────────────────────────────────

    #[test]
    fn edge_to_json_basic() {
        let mut props = HashMap::new();
        props.insert("weight".to_string(), Value::Float(0.75));
        let edge = EdgeData {
            id: EdgeId(42),
            rel_type: "KNOWS".to_string(),
            src: NodeId(1),
            dst: NodeId(2),
            properties: props,
        };
        let result = edge_to_json(&edge);
        assert_eq!(result["id"], json!("42"));
        assert_eq!(result["type"], json!("KNOWS"));
        assert_eq!(result["source"], json!("1"));
        assert_eq!(result["target"], json!("2"));
        assert_eq!(result["properties"]["weight"], json!(0.75));
    }

    #[test]
    fn edge_to_json_no_properties() {
        let edge = EdgeData {
            id: EdgeId(1),
            rel_type: "FOLLOWS".to_string(),
            src: NodeId(10),
            dst: NodeId(20),
            properties: HashMap::new(),
        };
        let result = edge_to_json(&edge);
        assert_eq!(result["id"], json!("1"));
        assert_eq!(result["type"], json!("FOLLOWS"));
        assert_eq!(result["properties"], json!({}));
    }

    // ── row_to_json ──────────────────────────────────────────────────────────

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
                let mut p = HashMap::new();
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
        let n1 = store.create_node("g", vec![], HashMap::new()).unwrap();
        let n2 = store.create_node("g", vec![], HashMap::new()).unwrap();
        let edge_id = store
            .create_edge("g", n1, n2, "KNOWS".to_string(), HashMap::new())
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
        assert!(err.contains("9999"), "error should mention the node id: {err}");
    }

    #[test]
    fn row_to_json_resolves_path() {
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();
        let n1 = store
            .create_node("g", vec!["A".to_string()], HashMap::new())
            .unwrap();
        let n2 = store
            .create_node("g", vec!["B".to_string()], HashMap::new())
            .unwrap();
        let e = store
            .create_edge("g", n1, n2, "REL".to_string(), HashMap::new())
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
            .create_node("g", vec!["X".to_string()], HashMap::new())
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
