//! Data integrity tests: verify that every mutation operation maintains graph invariants.
//!
//! Phase 2 of the comprehensive end-to-end validation plan (issue #189).
//!
//! Covers:
//!  - §2.1 Referential integrity after DELETE / DETACH DELETE
//!  - §2.2 MERGE atomicity (ON CREATE SET / ON MATCH SET)
//!  - §2.3 Property round-trips for all value types
//!  - §2.4 Label add / remove / multi-label invariants
//!  - §2.5 Node and edge ID uniqueness and stability

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use gql_core::error::StorageError;
use gql_core::executor::{ExecutionBudget, QueryExecutor};
use gql_core::parser::parse_query;
use gql_core::planner::QueryPlanner;
use gql_core::storage::{GraphStore, InMemoryGraphStore, NodeId};
use gql_core::types::Value;

// ── Helpers ────────────────────────────────────────────────────────────────────

fn make_store() -> InMemoryGraphStore {
    let store = InMemoryGraphStore::default();
    store.create_graph("test").unwrap();
    store
}

/// Run any Cypher query (read or write) and return its rows.
fn run_query(store: &InMemoryGraphStore, query: &str) -> Vec<HashMap<String, Value>> {
    let ast = parse_query(query).expect("parse error");
    let planner = QueryPlanner::new(store, "test");
    let plan = planner.plan(&ast).expect("plan error");
    let executor = QueryExecutor::new(store);
    let mut budget = ExecutionBudget::new(Duration::from_secs(60), usize::MAX);
    executor
        .execute("test", &plan, &mut budget)
        .expect("exec error")
        .rows
}

// ── §2.1 Referential Integrity ─────────────────────────────────────────────────

/// After DETACH DELETE of one node, all remaining edges must have valid endpoints.
#[test]
fn test_no_dangling_edges_after_node_delete() {
    let store = make_store();
    // Alice -[:KNOWS]-> Bob -[:LIKES]-> Charlie
    run_query(
        &store,
        "CREATE (a:P {name:'Alice'})-[:KNOWS]->(b:P {name:'Bob'})-[:LIKES]->(c:P {name:'Charlie'})",
    );

    // Detach delete Alice (removes Alice + the KNOWS edge, leaves LIKES intact).
    run_query(&store, "MATCH (n:P {name:'Alice'}) DETACH DELETE n");

    // Every edge that still exists must have both endpoints present.
    let edge_ids = store.all_edge_ids("test").unwrap();
    for eid in &edge_ids {
        let edge = store
            .get_edge("test", *eid)
            .unwrap()
            .expect("edge returned by all_edge_ids must be retrievable");
        assert!(
            store.get_node("test", edge.src).unwrap().is_some(),
            "edge src {:?} must still exist after delete",
            edge.src
        );
        assert!(
            store.get_node("test", edge.dst).unwrap().is_some(),
            "edge dst {:?} must still exist after delete",
            edge.dst
        );
    }

    let stats = store.graph_stats("test").unwrap();
    assert_eq!(stats.node_count, 2, "Bob and Charlie must survive");
    assert_eq!(stats.edge_count, 1, "LIKES edge must survive");
}

/// DETACH DELETE of a hub node removes all its connected edges, but leaves leaf nodes intact.
#[test]
fn test_detach_delete_removes_all_connected_edges() {
    let store = make_store();

    // Star graph: 1 center + 5 leaf nodes.
    let center = store
        .create_node("test", vec!["Center".to_string()], HashMap::new())
        .unwrap();
    for _ in 0..5 {
        let leaf = store
            .create_node("test", vec!["Leaf".to_string()], HashMap::new())
            .unwrap();
        store
            .create_edge("test", center, leaf, "LINK".to_string(), HashMap::new())
            .unwrap();
    }

    let before = store.graph_stats("test").unwrap();
    assert_eq!(before.edge_count, 5, "should start with 5 edges");

    run_query(&store, "MATCH (c:Center) DETACH DELETE c");

    let after = store.graph_stats("test").unwrap();
    assert_eq!(after.edge_count, 0, "all edges removed with center");
    assert_eq!(after.node_count, 5, "leaf nodes survive");
}

/// Deleting a node that does not exist must return an error, not silently succeed.
#[test]
fn test_delete_nonexistent_node_returns_error() {
    let store = make_store();
    let fake_id = NodeId(99_999);
    let result = store.delete_node("test", fake_id, false);
    assert!(
        matches!(result, Err(StorageError::NodeNotFound { .. })),
        "expected NodeNotFound; got {:?}",
        result
    );
}

/// Deleting an edge via Cypher leaves both endpoint nodes intact.
#[test]
fn test_delete_edge_leaves_nodes_intact() {
    let store = make_store();
    run_query(
        &store,
        "CREATE (a:Person {name:'Alice'})-[:KNOWS]->(b:Person {name:'Bob'})",
    );

    run_query(&store, "MATCH ()-[r:KNOWS]->() DELETE r");

    let stats = store.graph_stats("test").unwrap();
    assert_eq!(stats.node_count, 2, "nodes must survive edge deletion");
    assert_eq!(stats.edge_count, 0, "edge must be gone");
}

// ── §2.2 MERGE Atomicity ──────────────────────────────────────────────────────

/// MERGE creates a node when none matches the pattern.
#[test]
fn test_merge_creates_when_not_exists() {
    let store = make_store();
    run_query(&store, "MERGE (n:Person {name:'Alice'})");
    let rows = run_query(
        &store,
        "MATCH (n:Person {name:'Alice'}) RETURN count(n) AS c",
    );
    assert_eq!(
        rows[0]["c"],
        Value::Int(1),
        "MERGE must create exactly one node"
    );
}

/// MERGE does not duplicate a node that already matches the pattern.
#[test]
fn test_merge_matches_when_exists() {
    let store = make_store();
    run_query(&store, "CREATE (:Person {name:'Alice'})");
    run_query(&store, "MERGE (n:Person {name:'Alice'})");
    let rows = run_query(
        &store,
        "MATCH (n:Person {name:'Alice'}) RETURN count(n) AS c",
    );
    assert_eq!(
        rows[0]["c"],
        Value::Int(1),
        "MERGE must not duplicate existing node"
    );
}

/// ON CREATE SET fires on creation; ON MATCH SET does not.
#[test]
fn test_merge_on_create_set_only_runs_on_create() {
    let store = make_store();
    run_query(
        &store,
        "MERGE (n:Person {name:'Alice'}) ON CREATE SET n.created = 1 ON MATCH SET n.matched = 1",
    );
    let rows = run_query(
        &store,
        "MATCH (n:Person {name:'Alice'}) RETURN n.created AS created, n.matched AS matched",
    );
    assert_eq!(rows[0]["created"], Value::Int(1), "ON CREATE SET must fire");
    assert_eq!(
        rows[0].get("matched").cloned().unwrap_or(Value::Null),
        Value::Null,
        "ON MATCH SET must not fire on create"
    );
}

/// ON MATCH SET fires when the node already exists; ON CREATE SET does not.
#[test]
fn test_merge_on_match_set_only_runs_on_match() {
    let store = make_store();
    run_query(&store, "CREATE (:Person {name:'Alice'})");
    run_query(
        &store,
        "MERGE (n:Person {name:'Alice'}) ON CREATE SET n.created = 1 ON MATCH SET n.matched = 1",
    );
    let rows = run_query(
        &store,
        "MATCH (n:Person {name:'Alice'}) RETURN n.created AS created, n.matched AS matched",
    );
    assert_eq!(rows[0]["matched"], Value::Int(1), "ON MATCH SET must fire");
    assert_eq!(
        rows[0].get("created").cloned().unwrap_or(Value::Null),
        Value::Null,
        "ON CREATE SET must not fire on match"
    );
}

/// Running MERGE twice with the same pattern is idempotent.
#[test]
fn test_merge_same_pattern_idempotent() {
    let store = make_store();
    run_query(&store, "MERGE (n:Thing {key:'x'})");
    run_query(&store, "MERGE (n:Thing {key:'x'})");
    let rows = run_query(&store, "MATCH (n:Thing {key:'x'}) RETURN count(n) AS c");
    assert_eq!(
        rows[0]["c"],
        Value::Int(1),
        "two MERGEs on same pattern must be idempotent"
    );
}

// ── §2.3 Property Round-Trips ─────────────────────────────────────────────────

/// Integer and float properties survive a write→read round-trip.
#[test]
fn test_set_then_read_integer_and_float() {
    let store = make_store();
    run_query(&store, "CREATE (:Item {qty: 42, price: 2.72})");
    let rows = run_query(
        &store,
        "MATCH (n:Item) RETURN n.qty AS qty, n.price AS price",
    );
    assert_eq!(rows[0]["qty"], Value::Int(42));
    match &rows[0]["price"] {
        Value::Float(f) => assert!(
            (f - 2.72_f64).abs() < 1e-9,
            "price should be ~2.72, got {f}"
        ),
        other => panic!("expected float for price, got {:?}", other),
    }
}

/// Boolean and string properties survive a write→read round-trip.
#[test]
fn test_set_then_read_boolean_and_string() {
    let store = make_store();
    run_query(&store, "CREATE (:Flag {active: true, label: 'hello'})");
    let rows = run_query(&store, "MATCH (n:Flag) RETURN n.active AS a, n.label AS l");
    assert_eq!(rows[0]["a"], Value::Bool(true));
    assert_eq!(rows[0]["l"], Value::Str("hello".into()));
}

/// Updating one property via SET leaves all other properties unchanged.
#[test]
fn test_set_overwrite_preserves_other_properties() {
    let store = make_store();
    run_query(&store, "CREATE (:Node {a: 1, b: 2})");
    run_query(&store, "MATCH (n:Node) SET n.a = 99");
    let rows = run_query(&store, "MATCH (n:Node) RETURN n.a AS a, n.b AS b");
    assert_eq!(rows[0]["a"], Value::Int(99), "updated property");
    assert_eq!(rows[0]["b"], Value::Int(2), "untouched property preserved");
}

/// REMOVE on a property makes it return null on subsequent reads.
#[test]
fn test_remove_property_then_read_returns_null() {
    let store = make_store();
    run_query(&store, "CREATE (:Node {a: 1, b: 2})");
    run_query(&store, "MATCH (n:Node) REMOVE n.a");
    let rows = run_query(&store, "MATCH (n:Node) RETURN n.a AS a, n.b AS b");
    assert_eq!(
        rows[0]["a"],
        Value::Null,
        "removed property must read as null"
    );
    assert_eq!(
        rows[0]["b"],
        Value::Int(2),
        "non-removed property preserved"
    );
}

/// SET n = {…} replaces ALL properties on the node.
#[test]
fn test_replace_all_properties_via_set_equals() {
    let store = make_store();
    run_query(&store, "CREATE (:Node {a: 1, b: 2, c: 3})");
    run_query(&store, "MATCH (n:Node) SET n = {x: 99}");
    let rows = run_query(&store, "MATCH (n:Node) RETURN n.a AS a, n.b AS b, n.x AS x");
    assert_eq!(rows[0]["a"], Value::Null, "old property a removed by SET =");
    assert_eq!(rows[0]["b"], Value::Null, "old property b removed by SET =");
    assert_eq!(rows[0]["x"], Value::Int(99), "new property x present");
}

/// Unicode property values survive a write→read round-trip.
#[test]
fn test_property_with_unicode_value() {
    let store = make_store();
    run_query(&store, "CREATE (:Item {name: 'Ünïcödé 日本語 🎉'})");
    let rows = run_query(&store, "MATCH (n:Item) RETURN n.name AS name");
    assert_eq!(rows[0]["name"], Value::Str("Ünïcödé 日本語 🎉".into()));
}

/// Empty-string property values survive a write→read round-trip.
#[test]
fn test_property_with_empty_string() {
    let store = make_store();
    run_query(&store, "CREATE (:Item {s: ''})");
    let rows = run_query(&store, "MATCH (n:Item) RETURN n.s AS s");
    assert_eq!(rows[0]["s"], Value::Str("".into()));
}

// ── §2.4 Label Integrity ──────────────────────────────────────────────────────

/// Adding a label via SET makes the node findable by that label.
#[test]
fn test_add_label_then_find_by_label() {
    let store = make_store();
    run_query(&store, "CREATE (:Person {name:'Alice'})");
    run_query(&store, "MATCH (n:Person {name:'Alice'}) SET n:Employee");
    let rows = run_query(&store, "MATCH (n:Employee) RETURN n.name AS name");
    assert_eq!(rows.len(), 1, "node must be found by the newly added label");
    assert_eq!(rows[0]["name"], Value::Str("Alice".into()));
}

/// Removing a label via REMOVE excludes the node from scans on that label.
#[test]
fn test_remove_label_then_find_excludes_it() {
    let store = make_store();
    run_query(&store, "CREATE (:Person:Employee {name:'Alice'})");
    run_query(
        &store,
        "MATCH (n:Employee {name:'Alice'}) REMOVE n:Employee",
    );

    let rows_emp = run_query(&store, "MATCH (n:Employee) RETURN count(n) AS c");
    assert_eq!(
        rows_emp[0]["c"],
        Value::Int(0),
        "must not appear under removed label"
    );

    let rows_person = run_query(&store, "MATCH (n:Person) RETURN count(n) AS c");
    assert_eq!(
        rows_person[0]["c"],
        Value::Int(1),
        "must still appear under retained label"
    );
}

/// A node with multiple labels is findable by any of its labels.
#[test]
fn test_multi_label_node_found_by_any_label() {
    let store = make_store();
    run_query(&store, "CREATE (:A:B:C {x: 1})");
    for label in &["A", "B", "C"] {
        let rows = run_query(&store, &format!("MATCH (n:{label}) RETURN n.x AS x"));
        assert_eq!(rows.len(), 1, "node must be found by label {label}");
        assert_eq!(rows[0]["x"], Value::Int(1));
    }
}

// ── §2.5 ID Uniqueness and Stability ─────────────────────────────────────────

/// Every created node receives a unique ID.
#[test]
fn test_node_ids_are_unique_across_creates() {
    let store = make_store();
    let n = 300;
    let mut seen = HashSet::new();
    for i in 0..n {
        let props = HashMap::from([("i".to_string(), Value::Int(i as i64))]);
        let id = store
            .create_node("test", vec!["N".to_string()], props)
            .unwrap();
        assert!(seen.insert(id), "node ID {id:?} was duplicated at i={i}");
    }
    assert_eq!(seen.len(), n, "should have {n} unique node IDs");
}

/// Every created edge receives a unique ID.
#[test]
fn test_edge_ids_are_unique_across_creates() {
    let store = make_store();
    let src = store
        .create_node("test", vec!["A".to_string()], HashMap::new())
        .unwrap();
    let dst = store
        .create_node("test", vec!["B".to_string()], HashMap::new())
        .unwrap();

    let n = 200;
    let mut seen = HashSet::new();
    for _ in 0..n {
        let id = store
            .create_edge("test", src, dst, "R".to_string(), HashMap::new())
            .unwrap();
        assert!(seen.insert(id), "edge ID {id:?} was duplicated");
    }
    assert_eq!(seen.len(), n, "should have {n} unique edge IDs");
}

/// Node IDs are stable: deleting one node does not change the IDs of other nodes.
#[test]
fn test_id_stable_after_other_deletions() {
    let store = make_store();
    // Create 3 isolated nodes.
    let id_a = store
        .create_node(
            "test",
            vec!["T".to_string()],
            HashMap::from([("i".to_string(), Value::Int(1))]),
        )
        .unwrap();
    let id_b = store
        .create_node(
            "test",
            vec!["T".to_string()],
            HashMap::from([("i".to_string(), Value::Int(2))]),
        )
        .unwrap();
    let id_c = store
        .create_node(
            "test",
            vec!["T".to_string()],
            HashMap::from([("i".to_string(), Value::Int(3))]),
        )
        .unwrap();

    // Delete the middle node.
    store.delete_node("test", id_b, false).unwrap();

    // First and third nodes must still exist with the same IDs.
    let data_a = store.get_node("test", id_a).unwrap();
    assert!(
        data_a.is_some(),
        "first node must still exist after middle is deleted"
    );
    assert_eq!(data_a.unwrap().id, id_a, "first node ID must be stable");

    let data_c = store.get_node("test", id_c).unwrap();
    assert!(
        data_c.is_some(),
        "third node must still exist after middle is deleted"
    );
    assert_eq!(data_c.unwrap().id, id_c, "third node ID must be stable");

    // Deleted node must be gone.
    assert!(
        store.get_node("test", id_b).unwrap().is_none(),
        "deleted node must no longer be retrievable"
    );
}

/// The `id()` function in a query returns the numeric node ID.
#[test]
fn test_id_function_returns_correct_id() {
    let store = make_store();
    let node_id = store
        .create_node(
            "test",
            vec!["X".to_string()],
            HashMap::from([("v".to_string(), Value::Int(7))]),
        )
        .unwrap();

    let rows = run_query(&store, "MATCH (n:X) RETURN id(n) AS node_id");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["node_id"], Value::Int(node_id.0 as i64));
}
