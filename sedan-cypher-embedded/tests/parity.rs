//! Parity tests: verify that CypherEngine.execute() produces identical results
//! to using the raw cypher-core pipeline (parse → plan → execute) directly.
//!
//! This ensures the sedan-cypher-embedded wrapper doesn't silently alter
//! behavior compared to the underlying engine.

use std::collections::HashMap;
use std::time::Duration;

use cypher_core::executor::{ExecutionBudget, QueryExecutor};
use cypher_core::parser::parse_query;
use cypher_core::planner::QueryPlanner;
use cypher_core::storage::{GraphStore, InMemoryGraphStore};
use cypher_core::types::Value;

use sedan_cypher_embedded::CypherEngine;

// ── Helpers ──────────────────────────────────────────────────────────────────

fn fresh_store() -> InMemoryGraphStore {
    let store = InMemoryGraphStore::default();
    store.create_graph("g").expect("create graph");
    store
}

/// Execute via raw cypher-core pipeline.
fn raw_execute(store: &InMemoryGraphStore, query: &str) -> Vec<HashMap<String, Value>> {
    let ast = parse_query(query).expect("raw parse");
    let planner = QueryPlanner::new(store, "g");
    let plan = planner.plan(&ast).expect("raw plan");
    let mut budget = ExecutionBudget::new(Duration::from_secs(60), usize::MAX);
    QueryExecutor::new(store)
        .execute("g", &plan, &mut budget)
        .expect("raw exec")
        .rows
}

/// Execute via CypherEngine wrapper.
fn engine_execute(store: &InMemoryGraphStore, query: &str) -> Vec<HashMap<String, Value>> {
    let engine = CypherEngine::new();
    engine.execute(store, "g", query).expect("engine exec").rows
}

/// Compare row sets from both approaches (order-sensitive).
fn assert_parity(store: &InMemoryGraphStore, query: &str) {
    let raw_rows = raw_execute(store, query);
    let engine_rows = engine_execute(store, query);
    assert_eq!(
        raw_rows.len(),
        engine_rows.len(),
        "row count mismatch for query: {query}\nraw: {raw_rows:?}\nengine: {engine_rows:?}"
    );
    for (i, (raw, eng)) in raw_rows.iter().zip(engine_rows.iter()).enumerate() {
        assert_eq!(
            raw, eng,
            "row {i} mismatch for query: {query}\nraw: {raw:?}\nengine: {eng:?}"
        );
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[test]
fn parity_return_literal() {
    let store = fresh_store();
    assert_parity(&store, "RETURN 42 AS n");
}

#[test]
fn parity_return_string() {
    let store = fresh_store();
    assert_parity(&store, "RETURN 'hello world' AS s");
}

#[test]
fn parity_return_expression() {
    let store = fresh_store();
    assert_parity(&store, "RETURN 2 + 3 * 4 AS result");
}

#[test]
fn parity_create_and_match() {
    let store = fresh_store();

    // Both pipelines use the same store, so seed it once via raw
    raw_execute(&store, "CREATE (:Person {name: 'Alice', age: 30})");

    assert_parity(
        &store,
        "MATCH (p:Person) RETURN p.name AS name, p.age AS age",
    );
}

#[test]
fn parity_aggregation() {
    let store = fresh_store();
    raw_execute(&store, "CREATE (:N {v: 1}), (:N {v: 2}), (:N {v: 3})");
    assert_parity(&store, "MATCH (n:N) RETURN count(n) AS c, sum(n.v) AS s");
}

#[test]
fn parity_with_clause() {
    let store = fresh_store();
    raw_execute(&store, "CREATE (:N {v: 10}), (:N {v: 20}), (:N {v: 30})");
    assert_parity(
        &store,
        "MATCH (n:N) WITH n.v AS val WHERE val > 15 RETURN val ORDER BY val",
    );
}

#[test]
fn parity_relationship_traversal() {
    let store = fresh_store();
    raw_execute(
        &store,
        "CREATE (a:Person {name: 'Alice'})-[:KNOWS]->(b:Person {name: 'Bob'})",
    );
    assert_parity(
        &store,
        "MATCH (a:Person)-[:KNOWS]->(b:Person) RETURN a.name AS from, b.name AS to",
    );
}

#[test]
fn parity_error_on_invalid_syntax() {
    let store = fresh_store();
    let raw_err = parse_query("GARBAGE!!!").is_err();
    let engine_err = CypherEngine::new()
        .execute(&store, "g", "GARBAGE!!!")
        .is_err();
    assert_eq!(
        raw_err, engine_err,
        "both pipelines should error on invalid syntax"
    );
}

#[test]
fn parity_unwind() {
    let store = fresh_store();
    assert_parity(&store, "UNWIND [1, 2, 3] AS x RETURN x");
}

#[test]
fn parity_distinct() {
    let store = fresh_store();
    raw_execute(&store, "CREATE (:T {v: 1}), (:T {v: 1}), (:T {v: 2})");
    assert_parity(&store, "MATCH (t:T) RETURN DISTINCT t.v AS v ORDER BY v");
}
