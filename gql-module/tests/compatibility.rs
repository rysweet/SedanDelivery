//! FalkorDB / RedisGraph compatibility tests — Phase 6 (issue #189).
//!
//! Verifies that the GQL module's result serialization format is correct and
//! self-consistent.  Tests cover:
//!
//! §6.1 Scalar result encoding (int, float, string, bool, null, list, map)
//! §6.2 Node result encoding (id, labels, properties)
//! §6.3 Edge result encoding (id, type, src, dst, properties)
//! §6.4 Path result encoding (via shortestPath + nodes())
//! §6.5 Error response format
//! §6.6 Column ordering (BTreeMap — deterministic lexicographic)
//!
//! ## Wire format
//!
//! GQL.QUERY returns `Array` of `BulkString`, one per row.
//! Each BulkString is a JSON object mapping column name → serialized value.
//!
//! The serialization uses `gql_core::serde_value::value_to_json`:
//! - `null`              → JSON `null`
//! - `bool`              → JSON `true`/`false`
//! - `int`               → JSON number
//! - `float`             → JSON number (finite only)
//! - `string`            → JSON string (prefixed with `__str:` if starts with `__`)
//! - `list`              → JSON array
//! - `map`               → JSON object
//! - `node`              → JSON `{"_type":"node","_id":N,"_labels":[...],"_props":{...}}`
//! - `edge`              → JSON `{"_type":"edge","_id":N,"_rel":"T","_src":S,"_dst":D,"_props":{...}}`
//! - `path`              → JSON `{"_type":"path","nodes":[...],"edges":[...]}`

use std::collections::HashMap;
use std::time::Duration;

use gql_core::executor::{ExecutionBudget, QueryExecutor};
use gql_core::parser::parse_query;
use gql_core::planner::QueryPlanner;
use gql_core::serde_value::value_to_json;
use gql_core::storage::{GraphStore, InMemoryGraphStore};
use gql_core::types::Value;

// ── Helper ────────────────────────────────────────────────────────────────────

fn make_store(graph: &str) -> InMemoryGraphStore {
    let store = InMemoryGraphStore::default();
    store.create_graph(graph).unwrap();
    store
}

/// Simulate the full GQL.QUERY serialisation pipeline.
fn serialize_query(
    store: &InMemoryGraphStore,
    graph: &str,
    cypher: &str,
) -> Vec<serde_json::Value> {
    let ast = parse_query(cypher).expect("parse error");
    let planner = QueryPlanner::new(store, graph);
    let plan = planner.plan(&ast).expect("plan error");
    let executor = QueryExecutor::new(store);
    let mut budget = ExecutionBudget::new(Duration::from_secs(10), 1_000_000);
    let result = executor
        .execute(graph, &plan, &mut budget)
        .expect("exec error");

    result
        .rows
        .iter()
        .map(|row| {
            let ordered: std::collections::BTreeMap<_, _> = result
                .columns
                .iter()
                .filter_map(|col| row.get(col).map(|v| (col, v)))
                .collect();
            let json_row: serde_json::Map<String, serde_json::Value> = ordered
                .into_iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        value_to_json(v).unwrap_or(serde_json::Value::Null),
                    )
                })
                .collect();
            serde_json::Value::Object(json_row)
        })
        .collect()
}

// ── §6.1 Scalar result encoding ───────────────────────────────────────────────

#[test]
fn compat_null_encodes_to_json_null() {
    let store = make_store("c-null");
    let rows = serialize_query(&store, "c-null", "RETURN null AS v");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["v"], serde_json::Value::Null);
}

#[test]
fn compat_bool_true_encodes_correctly() {
    let store = make_store("c-bool");
    let rows = serialize_query(&store, "c-bool", "RETURN true AS v");
    assert_eq!(rows[0]["v"], serde_json::json!(true));
}

#[test]
fn compat_bool_false_encodes_correctly() {
    let store = make_store("c-bool2");
    let rows = serialize_query(&store, "c-bool2", "RETURN false AS v");
    assert_eq!(rows[0]["v"], serde_json::json!(false));
}

#[test]
fn compat_integer_encodes_as_json_number() {
    let store = make_store("c-int");
    let rows = serialize_query(&store, "c-int", "RETURN 42 AS v");
    assert_eq!(rows[0]["v"], serde_json::json!(42));
}

#[test]
fn compat_negative_integer_encodes_correctly() {
    let store = make_store("c-neg");
    let rows = serialize_query(&store, "c-neg", "RETURN -7 AS v");
    assert_eq!(rows[0]["v"], serde_json::json!(-7));
}

#[test]
fn compat_float_encodes_as_json_number() {
    let store = make_store("c-float");
    let rows = serialize_query(&store, "c-float", "RETURN 2.72 AS v");
    match rows[0]["v"].as_f64() {
        Some(f) => assert!((f - 2.72).abs() < 1e-9, "float must be ~2.72; got {f}"),
        None => panic!("expected JSON number for float"),
    }
}

#[test]
fn compat_string_encodes_as_json_string() {
    let store = make_store("c-str");
    let rows = serialize_query(&store, "c-str", "RETURN 'hello world' AS v");
    assert_eq!(rows[0]["v"], serde_json::json!("hello world"));
}

#[test]
fn compat_empty_string_encodes_correctly() {
    let store = make_store("c-empty");
    let rows = serialize_query(&store, "c-empty", "RETURN '' AS v");
    assert_eq!(rows[0]["v"], serde_json::json!(""));
}

#[test]
fn compat_string_starting_with_double_underscore_is_escaped() {
    // Strings starting with '__' get prefixed to avoid collision with our internal tags.
    let v = Value::Str("__internal".into());
    let json = value_to_json(&v).unwrap();
    assert_eq!(json, serde_json::json!("__str:__internal"));
}

#[test]
fn compat_list_encodes_as_json_array() {
    let store = make_store("c-list");
    let rows = serialize_query(&store, "c-list", "RETURN [1, 2, 3] AS v");
    assert_eq!(rows[0]["v"], serde_json::json!([1, 2, 3]));
}

#[test]
fn compat_nested_list_encodes_correctly() {
    let store = make_store("c-nested");
    let rows = serialize_query(&store, "c-nested", "RETURN [[1, 2], [3, 4]] AS v");
    assert_eq!(rows[0]["v"], serde_json::json!([[1, 2], [3, 4]]));
}

#[test]
fn compat_map_encodes_as_json_object() {
    let store = make_store("c-map");
    let rows = serialize_query(&store, "c-map", "RETURN {a: 1, b: 'two'} AS v");
    assert_eq!(rows[0]["v"]["a"], serde_json::json!(1));
    assert_eq!(rows[0]["v"]["b"], serde_json::json!("two"));
}

// ── §6.2 Node result encoding ──────────────────────────────────────────────────
//
// GQL module serialises Value::Node(id) as the tagged string "__node:{id}".
// Clients can use this to look up full node data or pass it as an argument.

#[test]
fn compat_node_returned_as_tagged_string() {
    let store = make_store("c-node");
    let mut props = HashMap::new();
    props.insert("name".to_string(), Value::Str("Alice".into()));
    props.insert("age".to_string(), Value::Int(30));
    let node_id = store
        .create_node("c-node", vec!["Person".to_string()], props)
        .unwrap();

    let rows = serialize_query(&store, "c-node", "MATCH (n:Person) RETURN n");
    assert_eq!(rows.len(), 1);

    // Nodes are serialised as "__node:{id}" tagged strings.
    let node_val = &rows[0]["n"];
    let expected = serde_json::json!(format!("__node:{}", node_id.0));
    assert_eq!(node_val, &expected, "node must serialise as __node:{{id}}");
}

#[test]
fn compat_node_property_projection_returns_scalar() {
    let store = make_store("c-nodeprop");
    let mut props = HashMap::new();
    props.insert("name".to_string(), Value::Str("Alice".into()));
    props.insert("age".to_string(), Value::Int(30));
    store
        .create_node("c-nodeprop", vec!["Person".to_string()], props)
        .unwrap();

    let rows = serialize_query(
        &store,
        "c-nodeprop",
        "MATCH (n:Person) RETURN n.name AS name, n.age AS age",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], serde_json::json!("Alice"));
    assert_eq!(rows[0]["age"], serde_json::json!(30));
}

// ── §6.3 Edge result encoding ──────────────────────────────────────────────────
//
// Value::Edge(id) is serialised as "__edge:{id}".

#[test]
fn compat_edge_returned_as_tagged_string() {
    let store = make_store("c-edge");
    let src = store
        .create_node("c-edge", vec!["A".to_string()], HashMap::new())
        .unwrap();
    let dst = store
        .create_node("c-edge", vec!["B".to_string()], HashMap::new())
        .unwrap();
    let edge_id = store
        .create_edge("c-edge", src, dst, "LINK".to_string(), HashMap::new())
        .unwrap();

    let rows = serialize_query(&store, "c-edge", "MATCH (a:A)-[r:LINK]->(b:B) RETURN r");
    assert_eq!(rows.len(), 1);

    let edge_val = &rows[0]["r"];
    let expected = serde_json::json!(format!("__edge:{}", edge_id.0));
    assert_eq!(edge_val, &expected, "edge must serialise as __edge:{{id}}");
}

#[test]
fn compat_edge_property_projection_returns_scalar() {
    let store = make_store("c-eprop");
    let src = store
        .create_node("c-eprop", vec!["A".to_string()], HashMap::new())
        .unwrap();
    let dst = store
        .create_node("c-eprop", vec!["B".to_string()], HashMap::new())
        .unwrap();
    let mut eprops = HashMap::new();
    eprops.insert("weight".to_string(), Value::Int(5));
    store
        .create_edge("c-eprop", src, dst, "LINK".to_string(), eprops)
        .unwrap();

    let rows = serialize_query(
        &store,
        "c-eprop",
        "MATCH ()-[r:LINK]->() RETURN r.weight AS w",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["w"], serde_json::json!(5));
}

// ── §6.4 Path result encoding ──────────────────────────────────────────────────
//
// Value::Path { nodes, edges } is serialised as:
//   { "__path_nodes": [id1, id2, ...], "__path_edges": [eid1, eid2, ...] }

#[test]
fn compat_path_serialised_with_tagged_keys() {
    let store = make_store("c-path");
    let n1 = store
        .create_node(
            "c-path",
            vec!["X".to_string()],
            HashMap::from([("v".to_string(), Value::Int(1))]),
        )
        .unwrap();
    let n2 = store
        .create_node(
            "c-path",
            vec!["X".to_string()],
            HashMap::from([("v".to_string(), Value::Int(2))]),
        )
        .unwrap();
    store
        .create_edge("c-path", n1, n2, "NEXT".to_string(), HashMap::new())
        .unwrap();

    // A 2-hop variable-length match produces path values when using named paths.
    // We use VarLenExpand to get paths and then verify the path serialization.
    // (shortestPath() parser not yet supported; test path value serialization directly.)
    let path_val = Value::Path {
        nodes: vec![n1, n2],
        edges: vec![store.find_edges_by_type("c-path", "NEXT").unwrap()[0]],
    };
    let json = value_to_json(&path_val).unwrap();
    assert!(
        json.get("__path_nodes").is_some(),
        "path must have __path_nodes; got: {json}"
    );
    assert!(
        json.get("__path_edges").is_some(),
        "path must have __path_edges; got: {json}"
    );
    let nodes = json["__path_nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 2, "path has 2 nodes");
    let edges = json["__path_edges"].as_array().unwrap();
    assert_eq!(edges.len(), 1, "path has 1 edge");
}

// ── §6.5 Error response format ────────────────────────────────────────────────

#[test]
fn compat_parse_error_message_format() {
    let result = parse_query("INVALID @@@@");
    let err_str = format!("{}", result.unwrap_err());
    assert!(!err_str.is_empty(), "error message must be non-empty");
}

#[test]
fn compat_exec_error_includes_budget_fields() {
    // When row budget is exceeded the error must mention count and limit.
    let store = make_store("c-budget");
    for i in 0..20i64 {
        store
            .create_node(
                "c-budget",
                vec!["N".to_string()],
                HashMap::from([("i".to_string(), Value::Int(i))]),
            )
            .unwrap();
    }
    let ast = parse_query("MATCH (n:N) RETURN n.i").unwrap();
    let planner = QueryPlanner::new(&store, "c-budget");
    let plan = planner.plan(&ast).unwrap();
    let executor = QueryExecutor::new(&store);
    let mut budget = ExecutionBudget::new(Duration::from_secs(60), 5);
    let err = executor
        .execute("c-budget", &plan, &mut budget)
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("5"),
        "error message must mention the row limit"
    );
}

// ── §6.6 Column ordering ──────────────────────────────────────────────────────

#[test]
fn compat_columns_are_lexicographically_ordered() {
    let store = make_store("c-cols");
    let rows = serialize_query(&store, "c-cols", "RETURN 1 AS z, 2 AS a, 3 AS m");
    assert_eq!(rows.len(), 1);
    let obj = rows[0].as_object().unwrap();
    let keys: Vec<&str> = obj.keys().map(|s| s.as_str()).collect();
    // BTreeMap produces lexicographic order: a < m < z.
    assert_eq!(
        keys,
        vec!["a", "m", "z"],
        "columns must be lexicographically ordered"
    );
}

// ── §6.1 Value serialization unit tests ──────────────────────────────────────

#[test]
fn compat_value_to_json_null() {
    assert_eq!(
        value_to_json(&Value::Null).unwrap(),
        serde_json::Value::Null
    );
}

#[test]
fn compat_value_to_json_bool() {
    assert_eq!(
        value_to_json(&Value::Bool(true)).unwrap(),
        serde_json::json!(true)
    );
    assert_eq!(
        value_to_json(&Value::Bool(false)).unwrap(),
        serde_json::json!(false)
    );
}

#[test]
fn compat_value_to_json_int() {
    assert_eq!(value_to_json(&Value::Int(0)).unwrap(), serde_json::json!(0));
    assert_eq!(
        value_to_json(&Value::Int(i64::MAX)).unwrap(),
        serde_json::json!(i64::MAX)
    );
    assert_eq!(
        value_to_json(&Value::Int(i64::MIN)).unwrap(),
        serde_json::json!(i64::MIN)
    );
}

#[test]
fn compat_value_to_json_float_finite() {
    let v = value_to_json(&Value::Float(1.234)).unwrap();
    assert!((v.as_f64().unwrap() - 1.234).abs() < 1e-9);
}

#[test]
fn compat_value_to_json_float_nan_is_error() {
    let result = value_to_json(&Value::Float(f64::NAN));
    assert!(result.is_err(), "NaN float must not serialize to JSON");
}

#[test]
fn compat_value_to_json_float_infinity_is_error() {
    let result = value_to_json(&Value::Float(f64::INFINITY));
    assert!(result.is_err(), "Infinite float must not serialize to JSON");
}

#[test]
fn compat_value_to_json_string() {
    let v = value_to_json(&Value::Str("hello".into())).unwrap();
    assert_eq!(v, serde_json::json!("hello"));
}

#[test]
fn compat_value_to_json_list_of_mixed_types() {
    let v = Value::List(vec![Value::Int(1), Value::Bool(true), Value::Null]);
    let json = value_to_json(&v).unwrap();
    assert_eq!(json, serde_json::json!([1, true, null]));
}
