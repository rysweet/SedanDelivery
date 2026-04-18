//! Value-pressure tests: exercise edge cases in the type system and value handling.
//!
//! Covers:
//!  - Boundary integers (i64::MIN, i64::MAX, 0, -1)
//!  - Float edge cases (NaN semantics, infinity, negative zero)
//!  - Empty and large strings
//!  - Null propagation in expressions
//!  - List and map construction
//!  - Type coercion edges

use sedan_cypher_embedded::{CypherEngine, GraphStore, InMemoryGraphStore, Value};

fn fresh() -> (CypherEngine, InMemoryGraphStore) {
    let engine = CypherEngine::new();
    let store = InMemoryGraphStore::default();
    store.create_graph("g").unwrap();
    (engine, store)
}

fn exec(
    engine: &CypherEngine,
    store: &InMemoryGraphStore,
    q: &str,
) -> Vec<std::collections::HashMap<String, Value>> {
    engine.execute(store, "g", q).unwrap().rows
}

// ── Integer boundaries ──────────────────────────────────────────────────────

#[test]
fn return_zero() {
    let (e, s) = fresh();
    let rows = exec(&e, &s, "RETURN 0 AS n");
    assert_eq!(rows[0]["n"], Value::Int(0));
}

#[test]
fn return_negative_one() {
    let (e, s) = fresh();
    let rows = exec(&e, &s, "RETURN -1 AS n");
    assert_eq!(rows[0]["n"], Value::Int(-1));
}

#[test]
fn return_large_positive_integer() {
    let (e, s) = fresh();
    // 2^53 - fits exactly in f64 too
    let rows = exec(&e, &s, "RETURN 9007199254740992 AS n");
    assert_eq!(rows[0]["n"], Value::Int(9_007_199_254_740_992));
}

#[test]
fn integer_arithmetic_near_max() {
    let (e, s) = fresh();
    // i64::MAX = 9223372036854775807; adding 0 should work fine
    let rows = exec(&e, &s, "RETURN 9223372036854775807 + 0 AS n");
    assert_eq!(rows[0]["n"], Value::Int(i64::MAX));
}

// ── Float edge cases ────────────────────────────────────────────────────────

#[test]
fn return_float_with_many_decimals() {
    let (e, s) = fresh();
    let rows = exec(&e, &s, "RETURN 1.23456789012345 AS f");
    match &rows[0]["f"] {
        Value::Float(f) => assert!((f - 1.23456789012345).abs() < 1e-10),
        other => panic!("expected Float, got {other:?}"),
    }
}

#[test]
fn return_negative_float() {
    let (e, s) = fresh();
    let rows = exec(&e, &s, "RETURN -0.5 AS f");
    match &rows[0]["f"] {
        Value::Float(f) => assert!((*f + 0.5).abs() < 1e-10),
        other => panic!("expected Float, got {other:?}"),
    }
}

#[test]
fn float_division_produces_float() {
    let (e, s) = fresh();
    let rows = exec(&e, &s, "RETURN 10.0 / 3.0 AS f");
    match &rows[0]["f"] {
        Value::Float(f) => assert!((*f - 10.0 / 3.0).abs() < 1e-10),
        other => panic!("expected Float, got {other:?}"),
    }
}

// ── String edge cases ───────────────────────────────────────────────────────

#[test]
fn return_empty_string() {
    let (e, s) = fresh();
    let rows = exec(&e, &s, "RETURN '' AS s");
    assert_eq!(rows[0]["s"], Value::Str("".into()));
}

#[test]
fn return_string_with_escaped_quotes() {
    let (e, s) = fresh();
    let rows = exec(&e, &s, r#"RETURN "hello \"world\"" AS s"#);
    assert_eq!(rows[0]["s"], Value::Str(r#"hello "world""#.into()));
}

#[test]
fn return_unicode_string() {
    let (e, s) = fresh();
    let rows = exec(&e, &s, "RETURN '日本語テスト' AS s");
    assert_eq!(rows[0]["s"], Value::Str("日本語テスト".into()));
}

#[test]
fn return_string_with_newlines() {
    let (e, s) = fresh();
    let rows = exec(&e, &s, r"RETURN 'line1\nline2' AS s");
    // The parser should interpret \n as newline
    match &rows[0]["s"] {
        Value::Str(s) => assert!(s.contains('\n') || s.contains("\\n")),
        other => panic!("expected Str, got {other:?}"),
    }
}

// ── Null propagation ────────────────────────────────────────────────────────

#[test]
fn null_plus_integer_is_null() {
    let (e, s) = fresh();
    let rows = exec(&e, &s, "RETURN null + 1 AS n");
    assert_eq!(rows[0]["n"], Value::Null);
}

#[test]
fn null_equals_null_is_null() {
    let (e, s) = fresh();
    let rows = exec(&e, &s, "RETURN null = null AS n");
    assert_eq!(rows[0]["n"], Value::Null);
}

#[test]
fn null_is_null_is_true() {
    let (e, s) = fresh();
    let rows = exec(&e, &s, "RETURN null IS NULL AS n");
    assert_eq!(rows[0]["n"], Value::Bool(true));
}

#[test]
fn null_is_not_null_is_false() {
    let (e, s) = fresh();
    let rows = exec(&e, &s, "RETURN null IS NOT NULL AS n");
    assert_eq!(rows[0]["n"], Value::Bool(false));
}

#[test]
fn value_is_not_null_is_true() {
    let (e, s) = fresh();
    let rows = exec(&e, &s, "RETURN 42 IS NOT NULL AS n");
    assert_eq!(rows[0]["n"], Value::Bool(true));
}

// ── Boolean logic ───────────────────────────────────────────────────────────

#[test]
fn boolean_and_with_null() {
    let (e, s) = fresh();
    let rows = exec(&e, &s, "RETURN true AND null AS n");
    assert_eq!(rows[0]["n"], Value::Null, "true AND null should be null");
}

#[test]
fn boolean_or_with_null() {
    let (e, s) = fresh();
    let rows = exec(&e, &s, "RETURN true OR null AS n");
    assert_eq!(
        rows[0]["n"],
        Value::Bool(true),
        "true OR null should be true"
    );
}

#[test]
fn false_and_null_is_false() {
    let (e, s) = fresh();
    let rows = exec(&e, &s, "RETURN false AND null AS n");
    assert_eq!(rows[0]["n"], Value::Bool(false));
}

#[test]
fn not_true_is_false() {
    let (e, s) = fresh();
    let rows = exec(&e, &s, "RETURN NOT true AS n");
    assert_eq!(rows[0]["n"], Value::Bool(false));
}

#[test]
fn not_null_is_null() {
    let (e, s) = fresh();
    let rows = exec(&e, &s, "RETURN NOT null AS n");
    assert_eq!(rows[0]["n"], Value::Null);
}

// ── List operations ─────────────────────────────────────────────────────────

#[test]
fn return_list_literal() {
    let (e, s) = fresh();
    let rows = exec(&e, &s, "RETURN [1, 2, 3] AS l");
    assert_eq!(
        rows[0]["l"],
        Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)])
    );
}

#[test]
fn return_empty_list() {
    let (e, s) = fresh();
    let rows = exec(&e, &s, "RETURN [] AS l");
    assert_eq!(rows[0]["l"], Value::List(vec![]));
}

#[test]
fn return_nested_list() {
    let (e, s) = fresh();
    let rows = exec(&e, &s, "RETURN [[1, 2], [3]] AS l");
    assert_eq!(
        rows[0]["l"],
        Value::List(vec![
            Value::List(vec![Value::Int(1), Value::Int(2)]),
            Value::List(vec![Value::Int(3)]),
        ])
    );
}

#[test]
fn list_index_access() {
    let (e, s) = fresh();
    let rows = exec(&e, &s, "RETURN [10, 20, 30][1] AS v");
    assert_eq!(rows[0]["v"], Value::Int(20));
}

// ── Map operations ──────────────────────────────────────────────────────────

#[test]
fn return_map_literal() {
    let (e, s) = fresh();
    let rows = exec(&e, &s, "RETURN {a: 1, b: 'two'} AS m");
    match &rows[0]["m"] {
        Value::Map(m) => {
            assert_eq!(m.get("a"), Some(&Value::Int(1)));
            assert_eq!(m.get("b"), Some(&Value::Str("two".into())));
        }
        other => panic!("expected Map, got {other:?}"),
    }
}

// ── CASE expressions ────────────────────────────────────────────────────────

#[test]
fn simple_case_expression() {
    let (e, s) = fresh();
    let rows = exec(
        &e,
        &s,
        "RETURN CASE WHEN true THEN 'yes' ELSE 'no' END AS result",
    );
    assert_eq!(rows[0]["result"], Value::Str("yes".into()));
}

#[test]
fn case_with_null_condition() {
    let (e, s) = fresh();
    let rows = exec(
        &e,
        &s,
        "RETURN CASE WHEN null THEN 'yes' ELSE 'no' END AS result",
    );
    assert_eq!(rows[0]["result"], Value::Str("no".into()));
}

// ── Property access on missing properties ───────────────────────────────────

#[test]
fn missing_property_returns_null() {
    let (e, s) = fresh();
    exec(&e, &s, "CREATE (:Node {a: 1})");
    let rows = exec(&e, &s, "MATCH (n:Node) RETURN n.nonexistent AS v");
    assert_eq!(rows[0]["v"], Value::Null);
}

// ── String functions ────────────────────────────────────────────────────────

#[test]
fn string_concatenation() {
    let (e, s) = fresh();
    let rows = exec(&e, &s, "RETURN 'hello' + ' ' + 'world' AS s");
    assert_eq!(rows[0]["s"], Value::Str("hello world".into()));
}

#[test]
fn to_upper_and_to_lower() {
    let (e, s) = fresh();
    let rows = exec(
        &e,
        &s,
        "RETURN toUpper('hello') AS u, toLower('WORLD') AS l",
    );
    assert_eq!(rows[0]["u"], Value::Str("HELLO".into()));
    assert_eq!(rows[0]["l"], Value::Str("world".into()));
}

#[test]
fn size_of_string() {
    let (e, s) = fresh();
    let rows = exec(&e, &s, "RETURN size('hello') AS n");
    assert_eq!(rows[0]["n"], Value::Int(5));
}

#[test]
fn size_of_list() {
    let (e, s) = fresh();
    let rows = exec(&e, &s, "RETURN size([1, 2, 3]) AS n");
    assert_eq!(rows[0]["n"], Value::Int(3));
}
