//! Tests for [`super::CypherEngine`].

use super::*;
use cypher_core::storage::InMemoryGraphStore;

fn test_store() -> InMemoryGraphStore {
    let store = InMemoryGraphStore::default();
    store.create_graph("test").expect("create graph");
    store
}

#[test]
fn return_literal() {
    let engine = CypherEngine::new();
    let store = test_store();
    let result = engine.execute(&store, "test", "RETURN 42 AS n").unwrap();
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0].get("n"), Some(&Value::Int(42)));
}

#[test]
fn create_and_match() {
    let engine = CypherEngine::new();
    let store = test_store();

    engine
        .execute(
            &store,
            "test",
            "CREATE (p:Person {name: 'Bob'}) RETURN p.name",
        )
        .unwrap();

    let result = engine
        .execute(&store, "test", "MATCH (p:Person) RETURN p.name")
        .unwrap();
    assert_eq!(result.rows.len(), 1);
    assert_eq!(
        result.rows[0].get("p.name"),
        Some(&Value::Str("Bob".into()))
    );
}

#[test]
fn invalid_cypher_returns_error() {
    let engine = CypherEngine::new();
    let store = test_store();
    assert!(engine.execute(&store, "test", "GARBAGE!!!").is_err());
}

// ── Default impl ────────────────────────────────────────────────────────

#[test]
fn default_impl_creates_usable_engine() {
    let engine: CypherEngine = Default::default();
    let store = test_store();
    let result = engine.execute(&store, "test", "RETURN 7 AS n").unwrap();
    assert_eq!(result.rows[0]["n"], Value::Int(7));
}

#[test]
fn default_impl_matches_new() {
    // Both paths must construct an engine that can execute queries.
    let a = CypherEngine::default();
    let b = CypherEngine::new();
    let store = test_store();
    assert_eq!(
        a.execute(&store, "test", "RETURN 1 AS n").unwrap().rows[0]["n"],
        Value::Int(1)
    );
    assert_eq!(
        b.execute(&store, "test", "RETURN 1 AS n").unwrap().rows[0]["n"],
        Value::Int(1)
    );
}

// ── with_cache_capacity ─────────────────────────────────────────────────

#[test]
fn with_cache_capacity_zero_disables_caching() {
    let engine = CypherEngine::with_cache_capacity(0);
    let store = test_store();
    for _ in 0..5 {
        let r = engine.execute(&store, "test", "RETURN 1 AS n").unwrap();
        assert_eq!(r.rows[0]["n"], Value::Int(1));
    }
}

#[test]
fn with_cache_capacity_small() {
    let engine = CypherEngine::with_cache_capacity(2);
    let store = test_store();
    for i in 0..6 {
        let q = format!("RETURN {i} AS n");
        let r = engine.execute(&store, "test", &q).unwrap();
        assert_eq!(r.rows[0]["n"], Value::Int(i as i64));
    }
}

#[test]
fn with_cache_capacity_large_repeats_hit_cache() {
    let engine = CypherEngine::with_cache_capacity(64);
    let store = test_store();
    let q = "RETURN 'cached' AS msg";
    let r1 = engine.execute(&store, "test", q).unwrap();
    let r2 = engine.execute(&store, "test", q).unwrap();
    assert_eq!(r1.rows[0]["msg"], Value::Str("cached".into()));
    assert_eq!(r2.rows[0]["msg"], Value::Str("cached".into()));
}

// ── clear_cache ─────────────────────────────────────────────────────────

#[test]
fn clear_cache_after_execute_still_works() {
    let engine = CypherEngine::new();
    let store = test_store();
    engine.execute(&store, "test", "RETURN 1 AS n").unwrap();
    engine.execute(&store, "test", "RETURN 2 AS n").unwrap();
    engine.clear_cache();
    let r = engine.execute(&store, "test", "RETURN 3 AS n").unwrap();
    assert_eq!(r.rows[0]["n"], Value::Int(3));
    let r2 = engine.execute(&store, "test", "RETURN 1 AS n").unwrap();
    assert_eq!(r2.rows[0]["n"], Value::Int(1));
}

#[test]
fn clear_cache_on_empty_cache_is_noop() {
    let engine = CypherEngine::new();
    engine.clear_cache();
    let store = test_store();
    let r = engine.execute(&store, "test", "RETURN 99 AS n").unwrap();
    assert_eq!(r.rows[0]["n"], Value::Int(99));
}

#[test]
fn clear_cache_on_zero_capacity_engine() {
    let engine = CypherEngine::with_cache_capacity(0);
    engine.clear_cache();
    let store = test_store();
    let r = engine.execute(&store, "test", "RETURN 1 AS n").unwrap();
    assert_eq!(r.rows[0]["n"], Value::Int(1));
}

// ── execute_with_parameters ─────────────────────────────────────────────

#[test]
fn execute_with_parameters_empty_params() {
    let engine = CypherEngine::new();
    let store = test_store();
    let params = HashMap::new();
    let registry = ProcedureRegistry::new();
    let r = engine
        .execute_with_parameters(&store, "test", "RETURN 1 AS n", &params, &registry)
        .unwrap();
    assert_eq!(r.rows[0]["n"], Value::Int(1));
}

#[test]
fn execute_with_parameters_single_param() {
    let engine = CypherEngine::new();
    let store = test_store();
    let mut params = HashMap::new();
    params.insert("x".to_string(), Value::Int(42));
    let registry = ProcedureRegistry::new();
    let r = engine
        .execute_with_parameters(&store, "test", "RETURN $x AS v", &params, &registry)
        .unwrap();
    assert_eq!(r.rows[0]["v"], Value::Int(42));
}

#[test]
fn execute_with_parameters_multiple_params() {
    let engine = CypherEngine::new();
    let store = test_store();
    let mut params = HashMap::new();
    params.insert("a".to_string(), Value::Int(10));
    params.insert("b".to_string(), Value::Int(32));
    let registry = ProcedureRegistry::new();
    let r = engine
        .execute_with_parameters(&store, "test", "RETURN $a + $b AS s", &params, &registry)
        .unwrap();
    assert_eq!(r.rows[0]["s"], Value::Int(42));
}

#[test]
fn execute_with_parameters_string_param() {
    let engine = CypherEngine::new();
    let store = test_store();
    let mut params = HashMap::new();
    params.insert("name".to_string(), Value::Str("Alice".into()));
    let registry = ProcedureRegistry::new();
    let r = engine
        .execute_with_parameters(&store, "test", "RETURN $name AS who", &params, &registry)
        .unwrap();
    assert_eq!(r.rows[0]["who"], Value::Str("Alice".into()));
}

#[test]
fn execute_with_parameters_in_where_clause() {
    let engine = CypherEngine::new();
    let store = test_store();
    engine
        .execute(
            &store,
            "test",
            "CREATE (:Person {name: 'Alice', age: 30}), (:Person {name: 'Bob', age: 25})",
        )
        .unwrap();
    let mut params = HashMap::new();
    params.insert("min_age".to_string(), Value::Int(28));
    let registry = ProcedureRegistry::new();
    let r = engine
        .execute_with_parameters(
            &store,
            "test",
            "MATCH (p:Person) WHERE p.age >= $min_age RETURN p.name AS name",
            &params,
            &registry,
        )
        .unwrap();
    assert_eq!(r.rows.len(), 1);
    assert_eq!(r.rows[0]["name"], Value::Str("Alice".into()));
}

#[test]
fn execute_with_parameters_invalid_query_errors() {
    let engine = CypherEngine::new();
    let store = test_store();
    let params = HashMap::new();
    let registry = ProcedureRegistry::new();
    let result =
        engine.execute_with_parameters(&store, "test", "TOTALLY BROKEN", &params, &registry);
    assert!(matches!(result, Err(GqlError::Parse(_))));
}

#[test]
fn execute_with_parameters_does_not_pollute_cache() {
    let engine = CypherEngine::new();
    let store = test_store();
    let mut params = HashMap::new();
    params.insert("x".to_string(), Value::Int(1));
    let registry = ProcedureRegistry::new();
    engine
        .execute_with_parameters(&store, "test", "RETURN $x AS n", &params, &registry)
        .unwrap();
    let r = engine.execute(&store, "test", "RETURN 5 AS n").unwrap();
    assert_eq!(r.rows[0]["n"], Value::Int(5));
}

// ── error variant exercise ──────────────────────────────────────────────

#[test]
fn execute_parse_error_variant() {
    let engine = CypherEngine::new();
    let store = test_store();
    let err = engine
        .execute(&store, "test", "MATCH ((( BROKEN")
        .unwrap_err();
    assert!(matches!(err, GqlError::Parse(_)));
}
