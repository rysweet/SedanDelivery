//! Value-pressure tests for sedan-gql-embedded.
//!
//! Exercises all Value variants through the SedanGql public API to ensure
//! type handling works correctly at the embedding layer.

use sedan_gql_embedded::{GraphStore, InMemoryGraphStore, SedanGql, Value};

fn setup() -> (SedanGql, InMemoryGraphStore) {
    let store = InMemoryGraphStore::default();
    store.create_graph("g").unwrap();
    (SedanGql::new(), store)
}

// ── Integer values ───────────────────────────────────────────────────────────

#[test]
fn integer_zero() {
    let (e, s) = setup();
    let qr = e.execute(&s, "g", "RETURN 0 AS val").unwrap();
    assert_eq!(qr.rows[0].get("val"), Some(&Value::Int(0)));
}

#[test]
fn integer_large_positive() {
    let (e, s) = setup();
    let qr = e.execute(&s, "g", "RETURN 2147483647 AS val").unwrap();
    assert_eq!(qr.rows[0].get("val"), Some(&Value::Int(2_147_483_647)));
}

#[test]
fn integer_negative() {
    let (e, s) = setup();
    let qr = e.execute(&s, "g", "RETURN -999 AS val").unwrap();
    assert_eq!(qr.rows[0].get("val"), Some(&Value::Int(-999)));
}

// ── Float values ─────────────────────────────────────────────────────────────

#[test]
fn float_positive() {
    let (e, s) = setup();
    let qr = e.execute(&s, "g", "RETURN 1.23 AS val").unwrap();
    match qr.rows[0].get("val") {
        Some(Value::Float(f)) => assert!((f - 1.23).abs() < 1e-10),
        other => panic!("expected Float, got: {other:?}"),
    }
}

#[test]
fn float_zero() {
    let (e, s) = setup();
    let qr = e.execute(&s, "g", "RETURN 0.0 AS val").unwrap();
    match qr.rows[0].get("val") {
        Some(Value::Float(f)) => assert!(f.abs() < 1e-10),
        other => panic!("expected Float, got: {other:?}"),
    }
}

// ── String values ────────────────────────────────────────────────────────────

#[test]
fn string_empty() {
    let (e, s) = setup();
    let qr = e.execute(&s, "g", "RETURN '' AS val").unwrap();
    assert_eq!(qr.rows[0].get("val"), Some(&Value::Str("".into())));
}

#[test]
fn string_unicode_ascii_subset() {
    let (e, s) = setup();
    let qr = e
        .execute(&s, "g", "RETURN 'hello world 123' AS val")
        .unwrap();
    assert_eq!(
        qr.rows[0].get("val"),
        Some(&Value::Str("hello world 123".into()))
    );
}

#[test]
fn string_with_escaped_quotes() {
    let (e, s) = setup();
    let qr = e.execute(&s, "g", r#"RETURN "hello" AS val"#).unwrap();
    assert_eq!(qr.rows[0].get("val"), Some(&Value::Str("hello".into())));
}

// ── Boolean values ───────────────────────────────────────────────────────────

#[test]
fn boolean_true() {
    let (e, s) = setup();
    let qr = e.execute(&s, "g", "RETURN true AS val").unwrap();
    assert_eq!(qr.rows[0].get("val"), Some(&Value::Bool(true)));
}

#[test]
fn boolean_false() {
    let (e, s) = setup();
    let qr = e.execute(&s, "g", "RETURN false AS val").unwrap();
    assert_eq!(qr.rows[0].get("val"), Some(&Value::Bool(false)));
}

// ── Null ─────────────────────────────────────────────────────────────────────

#[test]
fn null_value() {
    let (e, s) = setup();
    let qr = e.execute(&s, "g", "RETURN null AS val").unwrap();
    assert_eq!(qr.rows[0].get("val"), Some(&Value::Null));
}

// ── List values ──────────────────────────────────────────────────────────────

#[test]
fn list_empty() {
    let (e, s) = setup();
    let qr = e.execute(&s, "g", "RETURN [] AS val").unwrap();
    assert_eq!(qr.rows[0].get("val"), Some(&Value::List(vec![])));
}

#[test]
fn list_of_integers() {
    let (e, s) = setup();
    let qr = e.execute(&s, "g", "RETURN [1, 2, 3] AS val").unwrap();
    assert_eq!(
        qr.rows[0].get("val"),
        Some(&Value::List(vec![
            Value::Int(1),
            Value::Int(2),
            Value::Int(3),
        ]))
    );
}

#[test]
fn list_of_strings() {
    let (e, s) = setup();
    let qr = e.execute(&s, "g", "RETURN ['a', 'b', 'c'] AS val").unwrap();
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
    let (e, s) = setup();
    let qr = e
        .execute(&s, "g", "RETURN [1, 'two', true, null] AS val")
        .unwrap();
    match qr.rows[0].get("val") {
        Some(Value::List(items)) => assert_eq!(items.len(), 4),
        other => panic!("expected List, got: {other:?}"),
    }
}

// ── GQL INSERT with various property types ───────────────────────────────────

#[test]
fn insert_integer_property() {
    let (e, s) = setup();
    let qr = e
        .execute(&s, "g", "INSERT (n:N {val: 42}) RETURN n.val")
        .unwrap();
    assert_eq!(qr.rows[0].get("n.val"), Some(&Value::Int(42)));
}

#[test]
fn insert_float_property() {
    let (e, s) = setup();
    let qr = e
        .execute(&s, "g", "INSERT (n:N {val: 4.56}) RETURN n.val")
        .unwrap();
    match qr.rows[0].get("n.val") {
        Some(Value::Float(f)) => assert!((f - 4.56).abs() < 1e-10),
        other => panic!("expected Float, got: {other:?}"),
    }
}

#[test]
fn insert_boolean_property() {
    let (e, s) = setup();
    let qr = e
        .execute(&s, "g", "INSERT (n:N {active: true}) RETURN n.active")
        .unwrap();
    assert_eq!(qr.rows[0].get("n.active"), Some(&Value::Bool(true)));
}

#[test]
fn insert_string_property() {
    let (e, s) = setup();
    let qr = e
        .execute(&s, "g", "INSERT (n:N {name: 'test'}) RETURN n.name")
        .unwrap();
    assert_eq!(qr.rows[0].get("n.name"), Some(&Value::Str("test".into())));
}

#[test]
fn insert_list_property() {
    let (e, s) = setup();
    let qr = e
        .execute(&s, "g", "INSERT (n:N {tags: ['a', 'b']}) RETURN n.tags")
        .unwrap();
    assert_eq!(
        qr.rows[0].get("n.tags"),
        Some(&Value::List(vec![
            Value::Str("a".into()),
            Value::Str("b".into()),
        ]))
    );
}

// ── GQL FILTER with type-specific comparisons ────────────────────────────────

#[test]
fn filter_integer_gt() {
    let (e, s) = setup();
    e.execute(&s, "g", "CREATE (n:N {val: 10})").unwrap();
    e.execute(&s, "g", "CREATE (n:N {val: 20})").unwrap();

    let qr = e
        .execute(&s, "g", "MATCH (n:N) FILTER n.val > 15 RETURN n.val")
        .unwrap();
    assert_eq!(qr.rows.len(), 1);
    assert_eq!(qr.rows[0].get("n.val"), Some(&Value::Int(20)));
}

#[test]
fn filter_string_eq() {
    let (e, s) = setup();
    e.execute(&s, "g", "CREATE (n:N {name: 'Alice'})").unwrap();
    e.execute(&s, "g", "CREATE (n:N {name: 'Bob'})").unwrap();

    let qr = e
        .execute(&s, "g", "MATCH (n:N) FILTER n.name = 'Alice' RETURN n.name")
        .unwrap();
    assert_eq!(qr.rows.len(), 1);
    assert_eq!(qr.rows[0].get("n.name"), Some(&Value::Str("Alice".into())));
}

#[test]
fn filter_boolean_eq() {
    let (e, s) = setup();
    e.execute(&s, "g", "CREATE (n:N {active: true})").unwrap();
    e.execute(&s, "g", "CREATE (n:N {active: false})").unwrap();

    let qr = e
        .execute(
            &s,
            "g",
            "MATCH (n:N) FILTER n.active = true RETURN n.active",
        )
        .unwrap();
    assert_eq!(qr.rows.len(), 1);
    assert_eq!(qr.rows[0].get("n.active"), Some(&Value::Bool(true)));
}

// ── FOR with various list types ──────────────────────────────────────────────

#[test]
fn for_integer_list() {
    let (e, s) = setup();
    let qr = e
        .execute(&s, "g", "FOR x IN [10, 20, 30] RETURN x")
        .unwrap();
    assert_eq!(qr.rows.len(), 3);
    assert_eq!(qr.rows[0].get("x"), Some(&Value::Int(10)));
}

#[test]
fn for_empty_list() {
    let (e, s) = setup();
    let qr = e.execute(&s, "g", "FOR x IN [] RETURN x").unwrap();
    assert_eq!(qr.rows.len(), 0);
}

// ── Multi-property stress ────────────────────────────────────────────────────

#[test]
fn node_with_many_properties() {
    let (e, s) = setup();
    let qr = e
        .execute(
            &s,
            "g",
            "INSERT (n:Complex {a: 1, b: 'two', c: true, d: 1.23, e: null}) RETURN n.a, n.b, n.c, n.d, n.e",
        )
        .unwrap();
    assert_eq!(qr.rows.len(), 1);
    assert_eq!(qr.rows[0].get("n.a"), Some(&Value::Int(1)));
    assert_eq!(qr.rows[0].get("n.b"), Some(&Value::Str("two".into())));
    assert_eq!(qr.rows[0].get("n.c"), Some(&Value::Bool(true)));
    assert_eq!(qr.rows[0].get("n.e"), Some(&Value::Null));
}

// ── NULL handling via embedded API ──────────────────────────────────────────

#[test]
fn null_property_roundtrip() {
    let (e, s) = setup();
    let qr = e
        .execute(&s, "g", "INSERT (n:N {val: null}) RETURN n.val")
        .unwrap();
    assert_eq!(qr.rows[0].get("n.val"), Some(&Value::Null));
}

#[test]
fn null_arithmetic_propagates() {
    let (e, s) = setup();
    let qr = e.execute(&s, "g", "RETURN null + 1 AS val").unwrap();
    assert_eq!(qr.rows[0].get("val"), Some(&Value::Null));
}

// ── Persistence roundtrip via embedded API ──────────────────────────────────

#[test]
fn insert_then_match_roundtrip() {
    let (e, s) = setup();
    e.execute(&s, "g", "INSERT (n:Animal {species: 'Dog', legs: 4})")
        .unwrap();
    e.execute(&s, "g", "INSERT (n:Animal {species: 'Cat', legs: 4})")
        .unwrap();
    e.execute(&s, "g", "INSERT (n:Animal {species: 'Snake', legs: 0})")
        .unwrap();

    let qr = e
        .execute(
            &s,
            "g",
            "MATCH (n:Animal) FILTER n.legs = 4 RETURN n.species",
        )
        .unwrap();
    assert_eq!(qr.rows.len(), 2);
}

// ── execute_with_context via embedded API ───────────────────────────────────

#[test]
fn parameterised_insert() {
    use sedan_gql_embedded::ProcedureRegistry;
    use std::collections::HashMap;

    let (e, s) = setup();
    let mut params = HashMap::new();
    params.insert("name".into(), Value::Str("Param Alice".into()));

    let qr = e
        .execute_with_context(
            &s,
            "g",
            "INSERT (n:Person {name: $name}) RETURN n.name",
            &ProcedureRegistry::default(),
            params,
        )
        .unwrap();
    assert_eq!(
        qr.rows[0].get("n.name"),
        Some(&Value::Str("Param Alice".into()))
    );
}
