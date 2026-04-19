//! Gremlin TCK proof through the embedded crate's public API.
//!
//! Mirrors the structure of `sedan-cypher-embedded/tests/tck/runner.rs`: every
//! query is routed through [`GremlinEngine::execute`] so a passing run is
//! direct evidence that downstream consumers of `sedan-gremlin-embedded` get
//! the same behaviour as the underlying `gremlin-core` + `cypher-core`
//! pipeline. The fixture graph is the canonical TinkerPop "modern" graph
//! reused from the `gremlin-tck` crate so this suite always exercises the
//! exact same baseline data as `cargo test -p gremlin-tck`.
//!
//! The cases below cover the same step categories enumerated in
//! `tests/gremlin-tck/src/tck_tests.rs` (V/E/has/values/count/out/in/limit/
//! order/dedup/inject) but expressed as native Gremlin traversals — not
//! Cypher rewrites — so the embedded Gremlin parser, compiler, and executor
//! are all exercised.

use std::collections::BTreeSet;

use cypher_core::types::Value;
use gremlin_tck::modern_graph::create_modern_graph;
use sedan_gremlin_embedded::{GraphStore, GremlinEngine, QueryResult};

// ── Helpers ──────────────────────────────────────────────────────────────────

fn run(query: &str) -> QueryResult {
    let (store, _ids) = create_modern_graph();
    let engine = GremlinEngine::new();
    engine
        .execute(&store as &dyn GraphStore, "modern", query)
        .unwrap_or_else(|e| panic!("embedded execute failed for {query}: {e}"))
}

/// Extract a single scalar column from every row, sorted, as strings (for
/// order-insensitive set comparisons).
fn scalar_column_sorted(result: &QueryResult, col: &str) -> Vec<String> {
    let mut out: Vec<String> = result
        .rows
        .iter()
        .map(|r| {
            let v = r
                .get(col)
                .unwrap_or_else(|| panic!("column {col} missing; columns={:?}", result.columns));
            value_to_string(v)
        })
        .collect();
    out.sort();
    out
}

fn value_to_string(v: &Value) -> String {
    match v {
        Value::Str(s) => s.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => format!("{f}"),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".into(),
        other => format!("{other:?}"),
    }
}

fn single_int(result: &QueryResult, col: &str) -> i64 {
    assert_eq!(
        result.rows.len(),
        1,
        "expected single row, got {:?}",
        result.rows
    );
    match result.rows[0].get(col) {
        Some(Value::Int(n)) => *n,
        other => panic!("expected Int in column {col}, got {other:?}"),
    }
}

// ── Count.feature equivalents ────────────────────────────────────────────────

#[test]
fn tck_g_v_count() {
    let r = run("g.V().count()");
    assert_eq!(single_int(&r, &r.columns[0]), 6);
}

#[test]
fn tck_g_e_count() {
    let r = run("g.E().count()");
    assert_eq!(single_int(&r, &r.columns[0]), 6);
}

// ── HasStep.feature equivalents ──────────────────────────────────────────────

#[test]
fn tck_has_label_person_count() {
    let r = run("g.V().hasLabel('person').count()");
    assert_eq!(single_int(&r, &r.columns[0]), 4);
}

#[test]
fn tck_has_label_software_count() {
    let r = run("g.V().hasLabel('software').count()");
    assert_eq!(single_int(&r, &r.columns[0]), 2);
}

#[test]
fn tck_has_property_name_marko() {
    let r = run("g.V().has('name','marko').values('name')");
    let col = r.columns[0].clone();
    let names = scalar_column_sorted(&r, &col);
    assert_eq!(names, vec!["marko".to_string()]);
}

// ── Values.feature equivalents ───────────────────────────────────────────────

#[test]
fn tck_values_name_all_persons() {
    let r = run("g.V().hasLabel('person').values('name')");
    let col = r.columns[0].clone();
    let names: BTreeSet<String> = scalar_column_sorted(&r, &col).into_iter().collect();
    let expected: BTreeSet<String> = ["marko", "vadas", "josh", "peter"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(names, expected);
}

#[test]
fn tck_values_name_all_software() {
    let r = run("g.V().hasLabel('software').values('name')");
    let col = r.columns[0].clone();
    let names: BTreeSet<String> = scalar_column_sorted(&r, &col).into_iter().collect();
    let expected: BTreeSet<String> = ["lop", "ripple"].iter().map(|s| s.to_string()).collect();
    assert_eq!(names, expected);
}

// ── Traversal: out / in ──────────────────────────────────────────────────────

#[test]
fn tck_out_knows_from_marko() {
    let r = run("g.V().has('name','marko').out('knows').values('name')");
    let col = r.columns[0].clone();
    let names = scalar_column_sorted(&r, &col);
    assert_eq!(names, vec!["josh".to_string(), "vadas".to_string()]);
}

#[test]
fn tck_in_created_to_lop() {
    let r = run("g.V().has('name','lop').in('created').values('name')");
    let col = r.columns[0].clone();
    let names = scalar_column_sorted(&r, &col);
    assert_eq!(
        names,
        vec!["josh".to_string(), "marko".to_string(), "peter".to_string()]
    );
}

// ── Limit / Range ────────────────────────────────────────────────────────────

#[test]
fn tck_limit_two_persons() {
    let r = run("g.V().hasLabel('person').limit(2)");
    assert_eq!(r.rows.len(), 2);
}

// ── Dedup ────────────────────────────────────────────────────────────────────

#[test]
fn tck_dedup_software_names() {
    // Every person-created edge points at lop or ripple; dedup the names.
    let r = run("g.V().hasLabel('person').out('created').values('name').dedup()");
    let col = r.columns[0].clone();
    let names: BTreeSet<String> = scalar_column_sorted(&r, &col).into_iter().collect();
    let expected: BTreeSet<String> = ["lop", "ripple"].iter().map(|s| s.to_string()).collect();
    assert_eq!(names, expected);
}

// ── Order ────────────────────────────────────────────────────────────────────

#[test]
fn tck_order_person_names() {
    let r = run("g.V().hasLabel('person').values('name').order()");
    let col = r.columns[0].clone();
    let names: Vec<String> = r
        .rows
        .iter()
        .map(|row| match row.get(&col) {
            Some(Value::Str(s)) => s.to_string(),
            other => panic!("expected Str, got {other:?}"),
        })
        .collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted, "expected ascending order, got {names:?}");
    assert_eq!(names.len(), 4);
}

// ── Inject ───────────────────────────────────────────────────────────────────

#[test]
fn tck_inject_three_constants() {
    let r = run("g.inject(1, 2, 3)");
    assert_eq!(r.rows.len(), 3);
}

// ── Edge values ──────────────────────────────────────────────────────────────

#[test]
#[allow(non_snake_case)]
fn tck_outE_knows_weights_from_marko() {
    let r = run("g.V().has('name','marko').outE('knows').values('weight')");
    let col = r.columns[0].clone();
    let mut weights: Vec<String> = r
        .rows
        .iter()
        .map(|row| value_to_string(row.get(&col).unwrap()))
        .collect();
    weights.sort();
    assert_eq!(weights, vec!["0.5".to_string(), "1".to_string()]);
}
