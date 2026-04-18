//! Unit tests for sedan-cypher-embedded's CypherEngine API.
//!
//! Covers:
//!  - Default trait implementation
//!  - Public re-export completeness
//!  - Error variant propagation through CypherEngine::execute
//!  - API ergonomics (single-call workflow)

use sedan_cypher_embedded::{
    CypherEngine, GqlError, GraphStore, InMemoryGraphStore, MutationStats, QueryResult, Row, Value,
};

// ── Helpers ──────────────────────────────────────────────────────────────────

fn fresh_engine_and_store() -> (CypherEngine, InMemoryGraphStore) {
    let engine = CypherEngine::new();
    let store = InMemoryGraphStore::default();
    store.create_graph("g").expect("create graph");
    (engine, store)
}

// ── Default impl ─────────────────────────────────────────────────────────────

#[test]
fn default_creates_usable_engine() {
    let engine: CypherEngine = Default::default();
    let store = InMemoryGraphStore::default();
    store.create_graph("g").unwrap();
    let result = engine.execute(&store, "g", "RETURN 1 AS n").unwrap();
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0]["n"], Value::Int(1));
}

#[test]
fn new_and_default_are_equivalent() {
    let _a = CypherEngine::new();
    let _b: CypherEngine = Default::default();
    // Both are unit structs; this test ensures both constructors exist and compile.
}

// ── Re-export completeness ───────────────────────────────────────────────────

/// All public types listed in the crate root must be importable.
/// This test exercises the type system at compile time.
#[test]
fn re_exports_are_importable() {
    fn assert_types_exist(
        _engine: CypherEngine,
        _store: InMemoryGraphStore,
        _err: Option<GqlError>,
        _val: Value,
        _result: Option<QueryResult>,
        _row: Option<Row>,
        _stats: Option<MutationStats>,
    ) {
    }

    let engine = CypherEngine::new();
    let store = InMemoryGraphStore::default();
    assert_types_exist(engine, store, None, Value::Null, None, None, None);
}

// ── Error propagation ────────────────────────────────────────────────────────

#[test]
fn parse_error_propagated() {
    let (engine, store) = fresh_engine_and_store();
    let err = engine.execute(&store, "g", "NOT VALID CYPHER").unwrap_err();
    assert!(
        matches!(err, GqlError::Parse(_)),
        "syntax error must produce GqlError::Parse, got: {err}"
    );
}

#[test]
fn empty_query_propagated() {
    let (engine, store) = fresh_engine_and_store();
    let err = engine.execute(&store, "g", "").unwrap_err();
    assert!(
        matches!(err, GqlError::Parse(_)),
        "empty query must produce GqlError::Parse, got: {err}"
    );
}

#[test]
fn missing_graph_propagated() {
    let engine = CypherEngine::new();
    let store = InMemoryGraphStore::default();
    // Don't create any graph.
    let err = engine
        .execute(&store, "nonexistent", "RETURN 1")
        .unwrap_err();
    assert!(
        matches!(err, GqlError::Storage(_)),
        "missing graph must produce GqlError::Storage, got: {err}"
    );
}

#[test]
fn planning_error_propagated_for_undefined_variable() {
    let (engine, store) = fresh_engine_and_store();
    // Reference a variable not introduced in MATCH
    let err = engine
        .execute(&store, "g", "RETURN nonexistent_var")
        .unwrap_err();
    assert!(
        matches!(err, GqlError::Plan(_)),
        "undefined variable must produce GqlError::Plan, got: {err}"
    );
}

// ── Return value shapes ──────────────────────────────────────────────────────

#[test]
fn return_single_literal_int() {
    let (engine, store) = fresh_engine_and_store();
    let result = engine.execute(&store, "g", "RETURN 42 AS n").unwrap();
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0]["n"], Value::Int(42));
}

#[test]
fn return_single_literal_string() {
    let (engine, store) = fresh_engine_and_store();
    let result = engine.execute(&store, "g", "RETURN 'hello' AS s").unwrap();
    assert_eq!(result.rows[0]["s"], Value::Str("hello".into()));
}

#[test]
fn return_single_literal_float() {
    let (engine, store) = fresh_engine_and_store();
    let result = engine.execute(&store, "g", "RETURN 1.5 AS f").unwrap();
    match &result.rows[0]["f"] {
        Value::Float(f) => assert!((f - 1.5).abs() < 1e-9),
        other => panic!("expected Float, got {other:?}"),
    }
}

#[test]
fn return_single_literal_bool() {
    let (engine, store) = fresh_engine_and_store();
    let result = engine.execute(&store, "g", "RETURN true AS b").unwrap();
    assert_eq!(result.rows[0]["b"], Value::Bool(true));
}

#[test]
fn return_null() {
    let (engine, store) = fresh_engine_and_store();
    let result = engine.execute(&store, "g", "RETURN null AS n").unwrap();
    assert_eq!(result.rows[0]["n"], Value::Null);
}

#[test]
fn return_multiple_columns() {
    let (engine, store) = fresh_engine_and_store();
    let result = engine
        .execute(&store, "g", "RETURN 1 AS a, 'two' AS b, true AS c")
        .unwrap();
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0]["a"], Value::Int(1));
    assert_eq!(result.rows[0]["b"], Value::Str("two".into()));
    assert_eq!(result.rows[0]["c"], Value::Bool(true));
}

// NOTE: CRUD workflows, store isolation, and GQL-syntax rejection are tested
// in surface_matrix.rs and public_api_contract.rs. Not duplicated here.
