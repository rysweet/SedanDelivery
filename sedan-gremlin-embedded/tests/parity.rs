//! Parity tests for sedan-gremlin-embedded.
//!
//! Verifies that `GremlinEngine.execute()` produces identical results to using
//! the raw `gremlin-core` + `cypher-core` pipeline directly (parse → compile →
//! execute). This ensures the embedded wrapper does not silently alter
//! behaviour relative to the underlying engine. Mirrors the structure of the
//! equivalent `sedan-cypher-embedded` and `sedan-gql-embedded` parity suites
//! so that wrapper-vs-pipeline guarantees are uniform across all three
//! embedded engines.

use std::collections::HashMap;
use std::time::Duration;

use cypher_core::executor::{ExecutionBudget, QueryExecutor};
use cypher_core::storage::{GraphStore, InMemoryGraphStore};
use cypher_core::types::Value;
use gremlin_core::types::GremlinValue;
use gremlin_core::{parse_traversal, GremlinCompiler};
use indexmap::IndexMap;

use sedan_gremlin_embedded::GremlinEngine;

// ── Helpers ────────────────────────────────────────────────────────────────

fn fresh_store() -> InMemoryGraphStore {
    let store = InMemoryGraphStore::default();
    store.create_graph("g").expect("create graph");
    store
}

/// Execute via the raw pipeline: gremlin-core text parser → compiler →
/// cypher-core QueryExecutor in Gremlin mode.
fn raw_execute(store: &InMemoryGraphStore, query: &str) -> Vec<HashMap<String, Value>> {
    let no_params = |_: &str| -> Option<GremlinValue> { None };
    let steps = parse_traversal(query, &no_params).expect("raw parse");
    let mut compiler = GremlinCompiler::new();
    let operator = compiler.compile(&steps).expect("raw compile");
    let mut budget = ExecutionBudget::new(Duration::from_secs(60), usize::MAX);
    QueryExecutor::new(store as &dyn GraphStore)
        .with_gremlin_mode()
        .execute("g", &operator, &mut budget)
        .expect("raw exec")
        .rows
}

/// Execute via the GremlinEngine wrapper.
fn engine_execute(store: &InMemoryGraphStore, query: &str) -> Vec<HashMap<String, Value>> {
    let engine = GremlinEngine::new();
    engine
        .execute(store as &dyn GraphStore, "g", query)
        .expect("engine exec")
        .rows
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

fn seed_graph(store: &InMemoryGraphStore) {
    let alice = store
        .create_node(
            "g",
            vec!["Person".into()],
            IndexMap::from([
                ("name".into(), Value::Str("Alice".into())),
                ("age".into(), Value::Int(30)),
            ]),
        )
        .unwrap();
    let bob = store
        .create_node(
            "g",
            vec!["Person".into()],
            IndexMap::from([
                ("name".into(), Value::Str("Bob".into())),
                ("age".into(), Value::Int(25)),
            ]),
        )
        .unwrap();
    let charlie = store
        .create_node(
            "g",
            vec!["Person".into()],
            IndexMap::from([
                ("name".into(), Value::Str("Charlie".into())),
                ("age".into(), Value::Int(35)),
            ]),
        )
        .unwrap();
    store
        .create_edge("g", alice, bob, "knows".into(), IndexMap::new())
        .unwrap();
    store
        .create_edge("g", bob, charlie, "knows".into(), IndexMap::new())
        .unwrap();
}

// ── Source steps ───────────────────────────────────────────────────────────

#[test]
fn parity_v_returns_all_vertices() {
    let s = fresh_store();
    seed_graph(&s);
    assert_parity(&s, "g.V()");
}

#[test]
fn parity_v_count_on_seeded_graph() {
    let s = fresh_store();
    seed_graph(&s);
    assert_parity(&s, "g.V().count()");
}

#[test]
fn parity_e_count() {
    let s = fresh_store();
    seed_graph(&s);
    assert_parity(&s, "g.E().count()");
}

#[test]
fn parity_v_count_empty_graph() {
    let s = fresh_store();
    assert_parity(&s, "g.V().count()");
}

// ── Filter steps ───────────────────────────────────────────────────────────

#[test]
fn parity_has_label() {
    let s = fresh_store();
    seed_graph(&s);
    assert_parity(&s, "g.V().hasLabel('Person').count()");
}

#[test]
fn parity_has_property_value() {
    let s = fresh_store();
    seed_graph(&s);
    assert_parity(&s, "g.V().has('name', 'Alice').count()");
}

#[test]
fn parity_has_label_and_property() {
    let s = fresh_store();
    seed_graph(&s);
    assert_parity(&s, "g.V().hasLabel('Person').has('age', 30).values('name')");
}

// ── Projection steps ───────────────────────────────────────────────────────

#[test]
fn parity_values_single_property() {
    let s = fresh_store();
    seed_graph(&s);
    assert_parity(&s, "g.V().hasLabel('Person').values('name')");
}

#[test]
fn parity_value_map() {
    let s = fresh_store();
    seed_graph(&s);
    assert_parity(&s, "g.V().has('name','Alice').valueMap()");
}

#[test]
fn parity_label_step() {
    let s = fresh_store();
    seed_graph(&s);
    assert_parity(&s, "g.V().label()");
}

// ── Traversal steps ────────────────────────────────────────────────────────

#[test]
fn parity_out() {
    let s = fresh_store();
    seed_graph(&s);
    assert_parity(&s, "g.V().has('name','Alice').out('knows').values('name')");
}

#[test]
fn parity_in_step() {
    let s = fresh_store();
    seed_graph(&s);
    assert_parity(&s, "g.V().has('name','Bob').in('knows').values('name')");
}

#[test]
fn parity_both() {
    let s = fresh_store();
    seed_graph(&s);
    assert_parity(&s, "g.V().has('name','Bob').both('knows').values('name')");
}

#[test]
fn parity_out_e() {
    let s = fresh_store();
    seed_graph(&s);
    assert_parity(&s, "g.V().has('name','Alice').outE('knows').count()");
}

#[test]
fn parity_two_hop_path() {
    let s = fresh_store();
    seed_graph(&s);
    assert_parity(
        &s,
        "g.V().has('name','Alice').out('knows').out('knows').values('name')",
    );
}

// ── Aggregation steps ──────────────────────────────────────────────────────

#[test]
fn parity_inject_count() {
    let s = fresh_store();
    assert_parity(&s, "g.inject(1,2,3,4,5).count()");
}

#[test]
fn parity_inject_sum() {
    let s = fresh_store();
    assert_parity(&s, "g.inject(1,2,3,4,5).sum()");
}

#[test]
fn parity_inject_min_max() {
    let s = fresh_store();
    assert_parity(&s, "g.inject(5,1,3,8,2).min()");
}

#[test]
fn parity_values_count() {
    let s = fresh_store();
    seed_graph(&s);
    assert_parity(&s, "g.V().values('age').count()");
}

#[test]
fn parity_values_sum() {
    let s = fresh_store();
    seed_graph(&s);
    assert_parity(&s, "g.V().values('age').sum()");
}

// ── Limit / range / order ──────────────────────────────────────────────────

#[test]
fn parity_limit() {
    let s = fresh_store();
    seed_graph(&s);
    assert_parity(&s, "g.V().limit(2)");
}

#[test]
fn parity_limit_zero() {
    let s = fresh_store();
    seed_graph(&s);
    assert_parity(&s, "g.V().limit(0)");
}

#[test]
fn parity_order_by_age() {
    let s = fresh_store();
    seed_graph(&s);
    assert_parity(
        &s,
        "g.V().hasLabel('Person').order().by('age').values('name')",
    );
}

// ── Type predicates / dedup ────────────────────────────────────────────────

#[test]
fn parity_dedup_after_traversal() {
    let s = fresh_store();
    seed_graph(&s);
    assert_parity(
        &s,
        "g.V().hasLabel('Person').values('name').dedup().count()",
    );
}

// ── Empty-graph / no-match queries ─────────────────────────────────────────

#[test]
fn parity_v_count_with_no_matching_label() {
    let s = fresh_store();
    seed_graph(&s);
    assert_parity(&s, "g.V().hasLabel('Nonexistent').count()");
}

#[test]
fn parity_values_missing_property_count() {
    let s = fresh_store();
    seed_graph(&s);
    assert_parity(&s, "g.V().values('nonexistent').count()");
}
