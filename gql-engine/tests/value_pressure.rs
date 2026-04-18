//! Value-pressure tests for gql-engine.
//!
//! Exercises all Value variants through the GQL engine to ensure that
//! type handling, comparisons, and property storage work across the
//! full range of supported types.

use gql_engine::{execute, GraphStore, InMemoryGraphStore, Value};

fn fresh_store() -> InMemoryGraphStore {
    let store = InMemoryGraphStore::new();
    store.create_graph("g").unwrap();
    store
}

// ── Integer values ───────────────────────────────────────────────────────────

#[test]
fn integer_zero() {
    let store = fresh_store();
    let qr = execute("RETURN 0 AS val", &store, "g").unwrap();
    assert_eq!(qr.rows[0].get("val"), Some(&Value::Int(0)));
}

#[test]
fn integer_positive() {
    let store = fresh_store();
    let qr = execute("RETURN 42 AS val", &store, "g").unwrap();
    assert_eq!(qr.rows[0].get("val"), Some(&Value::Int(42)));
}

#[test]
fn integer_negative() {
    let store = fresh_store();
    let qr = execute("RETURN -1 AS val", &store, "g").unwrap();
    assert_eq!(qr.rows[0].get("val"), Some(&Value::Int(-1)));
}

#[test]
fn integer_large() {
    let store = fresh_store();
    let qr = execute("RETURN 9999999999 AS val", &store, "g").unwrap();
    assert_eq!(qr.rows[0].get("val"), Some(&Value::Int(9_999_999_999)));
}

// ── Float values ─────────────────────────────────────────────────────────────

#[test]
fn float_value() {
    let store = fresh_store();
    let qr = execute("RETURN 1.23 AS val", &store, "g").unwrap();
    match qr.rows[0].get("val") {
        Some(Value::Float(f)) => assert!((f - 1.23).abs() < 1e-10),
        other => panic!("expected Float, got: {other:?}"),
    }
}

#[test]
fn float_negative() {
    let store = fresh_store();
    let qr = execute("RETURN -2.5 AS val", &store, "g").unwrap();
    match qr.rows[0].get("val") {
        Some(Value::Float(f)) => assert!((f - (-2.5)).abs() < 1e-10),
        other => panic!("expected Float, got: {other:?}"),
    }
}

// ── String values ────────────────────────────────────────────────────────────

#[test]
fn string_empty() {
    let store = fresh_store();
    let qr = execute("RETURN '' AS val", &store, "g").unwrap();
    assert_eq!(qr.rows[0].get("val"), Some(&Value::Str("".into())));
}

#[test]
fn string_with_spaces() {
    let store = fresh_store();
    let qr = execute("RETURN 'hello world' AS val", &store, "g").unwrap();
    assert_eq!(
        qr.rows[0].get("val"),
        Some(&Value::Str("hello world".into()))
    );
}

#[test]
fn string_with_special_chars() {
    let store = fresh_store();
    let qr = execute("RETURN 'foo-bar_baz.qux' AS val", &store, "g").unwrap();
    assert_eq!(
        qr.rows[0].get("val"),
        Some(&Value::Str("foo-bar_baz.qux".into()))
    );
}

// ── Boolean values ───────────────────────────────────────────────────────────

#[test]
fn boolean_true() {
    let store = fresh_store();
    let qr = execute("RETURN true AS val", &store, "g").unwrap();
    assert_eq!(qr.rows[0].get("val"), Some(&Value::Bool(true)));
}

#[test]
fn boolean_false() {
    let store = fresh_store();
    let qr = execute("RETURN false AS val", &store, "g").unwrap();
    assert_eq!(qr.rows[0].get("val"), Some(&Value::Bool(false)));
}

// ── Null values ──────────────────────────────────────────────────────────────

#[test]
fn null_value() {
    let store = fresh_store();
    let qr = execute("RETURN null AS val", &store, "g").unwrap();
    assert_eq!(qr.rows[0].get("val"), Some(&Value::Null));
}

// ── List values ──────────────────────────────────────────────────────────────

#[test]
fn list_of_integers() {
    let store = fresh_store();
    let qr = execute("RETURN [1, 2, 3] AS val", &store, "g").unwrap();
    assert_eq!(
        qr.rows[0].get("val"),
        Some(&Value::List(vec![
            Value::Int(1),
            Value::Int(2),
            Value::Int(3)
        ]))
    );
}

#[test]
fn list_empty() {
    let store = fresh_store();
    let qr = execute("RETURN [] AS val", &store, "g").unwrap();
    assert_eq!(qr.rows[0].get("val"), Some(&Value::List(vec![])));
}

#[test]
fn list_of_strings() {
    let store = fresh_store();
    let qr = execute("RETURN ['a', 'b', 'c'] AS val", &store, "g").unwrap();
    assert_eq!(
        qr.rows[0].get("val"),
        Some(&Value::List(vec![
            Value::Str("a".into()),
            Value::Str("b".into()),
            Value::Str("c".into()),
        ]))
    );
}

#[test]
fn list_mixed_types() {
    let store = fresh_store();
    let qr = execute("RETURN [1, 'two', true, null] AS val", &store, "g").unwrap();
    match qr.rows[0].get("val") {
        Some(Value::List(items)) => assert_eq!(items.len(), 4),
        other => panic!("expected List, got: {other:?}"),
    }
}

// ── Arithmetic expressions ───────────────────────────────────────────────────

#[test]
fn arithmetic_addition() {
    let store = fresh_store();
    let qr = execute("RETURN 2 + 3 AS val", &store, "g").unwrap();
    assert_eq!(qr.rows[0].get("val"), Some(&Value::Int(5)));
}

#[test]
fn arithmetic_multiplication() {
    let store = fresh_store();
    let qr = execute("RETURN 4 * 5 AS val", &store, "g").unwrap();
    assert_eq!(qr.rows[0].get("val"), Some(&Value::Int(20)));
}

#[test]
fn arithmetic_division() {
    let store = fresh_store();
    let qr = execute("RETURN 10 / 3 AS val", &store, "g").unwrap();
    // Integer division in Cypher
    assert_eq!(qr.rows[0].get("val"), Some(&Value::Int(3)));
}

#[test]
fn arithmetic_modulo() {
    let store = fresh_store();
    let qr = execute("RETURN 10 % 3 AS val", &store, "g").unwrap();
    assert_eq!(qr.rows[0].get("val"), Some(&Value::Int(1)));
}

// ── GQL keyword + value type combinations ────────────────────────────────────

#[test]
fn insert_with_integer_property() {
    let store = fresh_store();
    let qr = execute("INSERT (n:N {val: 42}) RETURN n.val", &store, "g").unwrap();
    assert_eq!(qr.rows[0].get("n.val"), Some(&Value::Int(42)));
}

#[test]
fn insert_with_float_property() {
    let store = fresh_store();
    let qr = execute("INSERT (n:N {val: 1.23}) RETURN n.val", &store, "g").unwrap();
    match qr.rows[0].get("n.val") {
        Some(Value::Float(f)) => assert!((f - 1.23).abs() < 1e-10),
        other => panic!("expected Float, got: {other:?}"),
    }
}

#[test]
fn insert_with_boolean_property() {
    let store = fresh_store();
    let qr = execute("INSERT (n:N {active: true}) RETURN n.active", &store, "g").unwrap();
    assert_eq!(qr.rows[0].get("n.active"), Some(&Value::Bool(true)));
}

#[test]
fn insert_with_string_property() {
    let store = fresh_store();
    let qr = execute("INSERT (n:N {name: 'hello'}) RETURN n.name", &store, "g").unwrap();
    assert_eq!(qr.rows[0].get("n.name"), Some(&Value::Str("hello".into())));
}

#[test]
fn insert_with_list_property() {
    let store = fresh_store();
    let qr = execute("INSERT (n:N {tags: ['a', 'b']}) RETURN n.tags", &store, "g").unwrap();
    assert_eq!(
        qr.rows[0].get("n.tags"),
        Some(&Value::List(vec![
            Value::Str("a".into()),
            Value::Str("b".into()),
        ]))
    );
}

// ── FILTER with various comparison types ─────────────────────────────────────

#[test]
fn filter_integer_comparison() {
    let store = fresh_store();
    execute("CREATE (n:N {val: 10})", &store, "g").unwrap();
    execute("CREATE (n:N {val: 20})", &store, "g").unwrap();
    execute("CREATE (n:N {val: 30})", &store, "g").unwrap();

    let qr = execute("MATCH (n:N) FILTER n.val >= 20 RETURN n.val", &store, "g").unwrap();
    assert_eq!(qr.rows.len(), 2);
}

#[test]
fn filter_string_equality() {
    let store = fresh_store();
    execute("CREATE (n:N {name: 'Alice'})", &store, "g").unwrap();
    execute("CREATE (n:N {name: 'Bob'})", &store, "g").unwrap();

    let qr = execute(
        "MATCH (n:N) FILTER n.name = 'Alice' RETURN n.name",
        &store,
        "g",
    )
    .unwrap();
    assert_eq!(qr.rows.len(), 1);
    assert_eq!(qr.rows[0].get("n.name"), Some(&Value::Str("Alice".into())));
}

#[test]
fn filter_boolean_equality() {
    let store = fresh_store();
    execute("CREATE (n:N {active: true})", &store, "g").unwrap();
    execute("CREATE (n:N {active: false})", &store, "g").unwrap();

    let qr = execute(
        "MATCH (n:N) FILTER n.active = true RETURN n.active",
        &store,
        "g",
    )
    .unwrap();
    assert_eq!(qr.rows.len(), 1);
    assert_eq!(qr.rows[0].get("n.active"), Some(&Value::Bool(true)));
}

// ── FOR keyword with various list types ──────────────────────────────────────

#[test]
fn for_with_integer_list() {
    let store = fresh_store();
    let qr = execute("FOR x IN [10, 20, 30] RETURN x", &store, "g").unwrap();
    assert_eq!(qr.rows.len(), 3);
    assert_eq!(qr.rows[0].get("x"), Some(&Value::Int(10)));
}

#[test]
fn for_with_string_list() {
    let store = fresh_store();
    let qr = execute("FOR x IN ['a', 'b'] RETURN x", &store, "g").unwrap();
    assert_eq!(qr.rows.len(), 2);
    assert_eq!(qr.rows[0].get("x"), Some(&Value::Str("a".into())));
}

#[test]
fn for_with_empty_list() {
    let store = fresh_store();
    let qr = execute("FOR x IN [] RETURN x", &store, "g").unwrap();
    assert_eq!(qr.rows.len(), 0);
}

// ── Multi-property nodes ─────────────────────────────────────────────────────

#[test]
fn insert_node_with_many_properties() {
    let store = fresh_store();
    let qr = execute(
        "INSERT (n:Complex {name: 'test', age: 25, active: true, score: 9.5}) RETURN n.name, n.age, n.active, n.score",
        &store,
        "g",
    )
    .unwrap();
    assert_eq!(qr.rows.len(), 1);
    assert_eq!(qr.rows[0].get("n.name"), Some(&Value::Str("test".into())));
    assert_eq!(qr.rows[0].get("n.age"), Some(&Value::Int(25)));
    assert_eq!(qr.rows[0].get("n.active"), Some(&Value::Bool(true)));
    match qr.rows[0].get("n.score") {
        Some(Value::Float(f)) => assert!((f - 9.5).abs() < 1e-10),
        other => panic!("expected Float for score, got: {other:?}"),
    }
}

// ── NULL handling ───────────────────────────────────────────────────────────

#[test]
fn null_property_roundtrip() {
    let store = fresh_store();
    let qr = execute("INSERT (n:N {val: null}) RETURN n.val", &store, "g").unwrap();
    assert_eq!(qr.rows[0].get("n.val"), Some(&Value::Null));
}

#[test]
fn null_in_list() {
    let store = fresh_store();
    let qr = execute("RETURN [1, null, 3] AS val", &store, "g").unwrap();
    assert_eq!(
        qr.rows[0].get("val"),
        Some(&Value::List(vec![
            Value::Int(1),
            Value::Null,
            Value::Int(3),
        ]))
    );
}

#[test]
fn null_arithmetic_propagates() {
    let store = fresh_store();
    let qr = execute("RETURN null + 1 AS val", &store, "g").unwrap();
    assert_eq!(qr.rows[0].get("val"), Some(&Value::Null));
}

// ── Persistence / roundtrip ─────────────────────────────────────────────────

#[test]
fn insert_then_match_roundtrip() {
    let store = fresh_store();
    // INSERT several nodes, then verify all are retrievable
    execute("INSERT (n:Animal {species: 'Dog', legs: 4})", &store, "g").unwrap();
    execute("INSERT (n:Animal {species: 'Cat', legs: 4})", &store, "g").unwrap();
    execute("INSERT (n:Animal {species: 'Snake', legs: 0})", &store, "g").unwrap();

    let qr = execute(
        "MATCH (n:Animal) FILTER n.legs = 4 RETURN n.species",
        &store,
        "g",
    )
    .unwrap();
    assert_eq!(qr.rows.len(), 2);
}

#[test]
fn relationship_roundtrip() {
    let store = fresh_store();
    execute(
        "INSERT (a:Person {name: 'Alice'})-[:FRIEND {since: 2020}]->(b:Person {name: 'Bob'})",
        &store,
        "g",
    )
    .unwrap();

    let qr = execute(
        "MATCH (a:Person)-[r:FRIEND]->(b:Person) RETURN a.name, b.name",
        &store,
        "g",
    )
    .unwrap();
    assert_eq!(qr.rows.len(), 1);
    assert_eq!(qr.rows[0].get("a.name"), Some(&Value::Str("Alice".into())));
    assert_eq!(qr.rows[0].get("b.name"), Some(&Value::Str("Bob".into())));
}

// ── String edge cases ───────────────────────────────────────────────────────

#[test]
fn string_with_newline() {
    let store = fresh_store();
    let qr = execute("RETURN 'line1\\nline2' AS val", &store, "g").unwrap();
    match qr.rows[0].get("val") {
        Some(Value::Str(_)) => {} // Just verifying it doesn't panic
        other => panic!("expected Str, got: {other:?}"),
    }
}

#[test]
fn long_string_property() {
    let store = fresh_store();
    let long = "x".repeat(1000);
    let query = format!("INSERT (n:N {{data: '{long}'}}) RETURN n.data");
    let qr = execute(&query, &store, "g").unwrap();
    match qr.rows[0].get("n.data") {
        Some(Value::Str(s)) => assert_eq!(s.len(), 1000),
        other => panic!("expected Str, got: {other:?}"),
    }
}
