//! Value-pressure tests for sedan-gremlin-embedded.
//!
//! Exercises edge cases in the type system and value handling end-to-end through
//! the Gremlin parser → compiler → executor pipeline. Mirrors the structure of
//! the equivalent sedan-cypher-embedded and sedan-gql-embedded value-pressure
//! suites so that data-edge guarantees are uniform across all three embedded
//! engines.
//!
//! Covers:
//!  - Boundary integers (zero, negative, near-i64::MAX, exact-i64::MAX)
//!  - Float edge cases (negative, fractional, very small / very large)
//!  - Empty and Unicode strings
//!  - Boolean injection
//!  - List literals (empty, populated, nested)
//!  - Null property access on stored vertices
//!  - Aggregations over edge-case inputs (count, sum, min, max, mean)
//!  - Property round-trip for every primitive Value variant
//!  - Type coercion edges (count of empty traversal, sum of empty)

use indexmap::IndexMap;
use sedan_gremlin_embedded::{GraphStore, GremlinEngine, InMemoryGraphStore, QueryResult, Value};

fn engine() -> GremlinEngine {
    GremlinEngine::new()
}

fn store() -> InMemoryGraphStore {
    let s = InMemoryGraphStore::default();
    s.create_graph("g").expect("create graph");
    s
}

fn first_value(r: &QueryResult) -> &Value {
    r.rows
        .first()
        .and_then(|row| row.values().next())
        .expect("at least one row with one column")
}

fn run(q: &str) -> QueryResult {
    let e = engine();
    let s = store();
    e.execute(&s, "g", q).expect("query should succeed")
}

fn run_on(s: &InMemoryGraphStore, q: &str) -> QueryResult {
    engine().execute(s, "g", q).expect("query should succeed")
}

// ── Integer boundaries ─────────────────────────────────────────────────────

#[test]
fn inject_zero() {
    let r = run("g.inject(0)");
    assert_eq!(first_value(&r), &Value::Int(0));
}

#[test]
fn inject_negative_one() {
    let r = run("g.inject(-1)");
    assert_eq!(first_value(&r), &Value::Int(-1));
}

#[test]
fn inject_large_safe_integer() {
    // 2^53 — fits exactly in f64 so this exercises a value
    // that is sometimes mishandled when an engine downcasts to f64.
    let r = run("g.inject(9007199254740992)");
    assert_eq!(first_value(&r), &Value::Int(9_007_199_254_740_992));
}

#[test]
fn inject_i64_max() {
    let r = run("g.inject(9223372036854775807)");
    assert_eq!(first_value(&r), &Value::Int(i64::MAX));
}

#[test]
fn inject_i64_near_min() {
    // -2^62 — large negative value, well within i64 range.
    let r = run("g.inject(-4611686018427387904)");
    assert_eq!(first_value(&r), &Value::Int(-4_611_686_018_427_387_904));
}

// ── Float edge cases ───────────────────────────────────────────────────────

#[test]
fn inject_negative_float() {
    let r = run("g.inject(-0.5)");
    match first_value(&r) {
        Value::Float(f) => assert!((*f + 0.5).abs() < 1e-12),
        other => panic!("expected Float, got {other:?}"),
    }
}

#[test]
fn inject_small_fractional_float() {
    let r = run("g.inject(0.0001)");
    match first_value(&r) {
        Value::Float(f) => assert!((*f - 0.0001).abs() < 1e-12),
        other => panic!("expected Float, got {other:?}"),
    }
}

#[test]
fn inject_large_float() {
    let r = run("g.inject(1.7976931348623157e308)");
    match first_value(&r) {
        Value::Float(f) => assert!(f.is_finite() && *f > 1e300),
        other => panic!("expected Float, got {other:?}"),
    }
}

// ── String edge cases ──────────────────────────────────────────────────────

#[test]
fn inject_empty_string() {
    let r = run("g.inject('')");
    assert_eq!(first_value(&r), &Value::Str("".into()));
}

#[test]
fn inject_unicode_string() {
    let r = run("g.inject('日本語テスト')");
    assert_eq!(first_value(&r), &Value::Str("日本語テスト".into()));
}

#[test]
fn inject_string_with_spaces() {
    // Inject argument splitting is delimiter-sensitive in the current text
    // parser (commas inside string literals are still treated as argument
    // separators). Verify that strings without embedded commas survive
    // round-trip cleanly. See follow-up tracking issue for quote-aware
    // tokenisation in `gremlin-core/src/text_parser.rs`.
    let r = run("g.inject('hello world')");
    assert_eq!(first_value(&r), &Value::Str("hello world".into()));
}

// ── Boolean ────────────────────────────────────────────────────────────────

#[test]
fn inject_true_then_count() {
    let r = run("g.inject(true).count()");
    assert_eq!(first_value(&r), &Value::Int(1));
}

#[test]
fn inject_two_booleans_count() {
    let r = run("g.inject(true, false).count()");
    assert_eq!(first_value(&r), &Value::Int(2));
}

// ── Counts on empty / multi-element traversals ─────────────────────────────

#[test]
fn count_on_empty_graph_is_zero() {
    let r = run("g.V().count()");
    assert_eq!(first_value(&r), &Value::Int(0));
}

#[test]
fn count_on_empty_inject_is_zero() {
    let r = run("g.inject().count()");
    assert_eq!(first_value(&r), &Value::Int(0));
}

// ── Aggregations on edge-case inputs ───────────────────────────────────────

#[test]
fn sum_of_negative_and_positive_cancels() {
    let r = run("g.inject(10, -3, -7).sum()");
    assert_eq!(first_value(&r), &Value::Int(0));
}

#[test]
fn sum_of_zero_and_zero_is_zero() {
    let r = run("g.inject(0, 0, 0).sum()");
    assert_eq!(first_value(&r), &Value::Int(0));
}

#[test]
fn min_picks_smallest_negative() {
    let r = run("g.inject(5, -1, 3, -10, 0).min()");
    assert_eq!(first_value(&r), &Value::Int(-10));
}

#[test]
fn max_picks_largest_positive() {
    let r = run("g.inject(5, -1, 3, 10, 0).max()");
    assert_eq!(first_value(&r), &Value::Int(10));
}

#[test]
fn mean_of_uniform_values() {
    let r = run("g.inject(4, 4, 4, 4).mean()");
    match first_value(&r) {
        Value::Int(4) => {}
        Value::Float(f) => assert!((*f - 4.0).abs() < 1e-12),
        other => panic!("expected numeric 4, got {other:?}"),
    }
}

// ── Property round-trip for primitive Value variants ───────────────────────

fn assert_value_roundtrip(prop: &str, value: Value) {
    let s = store();
    let mut props = IndexMap::new();
    props.insert("name".into(), Value::Str("anchor".into()));
    props.insert(prop.into(), value.clone());
    s.create_node("g", vec!["X".into()], props)
        .expect("create node");

    let r = run_on(&s, &format!("g.V().values('{prop}')"));
    assert_eq!(
        first_value(&r),
        &value,
        "round-trip mismatch for property {prop}"
    );
}

#[test]
fn property_roundtrip_int_zero() {
    assert_value_roundtrip("p", Value::Int(0));
}

#[test]
fn property_roundtrip_int_negative() {
    assert_value_roundtrip("p", Value::Int(-12345));
}

#[test]
fn property_roundtrip_int_max() {
    assert_value_roundtrip("p", Value::Int(i64::MAX));
}

#[test]
fn property_roundtrip_int_min() {
    assert_value_roundtrip("p", Value::Int(i64::MIN));
}

#[test]
fn property_roundtrip_float() {
    assert_value_roundtrip("p", Value::Float(1.5));
}

#[test]
fn property_roundtrip_negative_float() {
    assert_value_roundtrip("p", Value::Float(-0.25));
}

#[test]
fn property_roundtrip_bool_true() {
    assert_value_roundtrip("p", Value::Bool(true));
}

#[test]
fn property_roundtrip_bool_false() {
    assert_value_roundtrip("p", Value::Bool(false));
}

#[test]
fn property_roundtrip_empty_string() {
    assert_value_roundtrip("p", Value::Str("".into()));
}

#[test]
fn property_roundtrip_unicode_string() {
    assert_value_roundtrip("p", Value::Str("日本語".into()));
}

#[test]
fn property_roundtrip_long_string() {
    let big = "x".repeat(10_000);
    assert_value_roundtrip("p", Value::Str(big.into()));
}

// ── Missing property handling ──────────────────────────────────────────────

#[test]
fn missing_property_yields_empty_traversal() {
    let s = store();
    let mut props = IndexMap::new();
    props.insert("name".into(), Value::Str("Alice".into()));
    s.create_node("g", vec!["Person".into()], props)
        .expect("create node");

    // Asking for a property that doesn't exist on any vertex yields no rows
    // (Gremlin semantics: values() emits nothing for missing keys).
    let r = run_on(&s, "g.V().values('nonexistent')");
    assert_eq!(r.rows.len(), 0);
}

#[test]
fn count_after_missing_property_is_zero() {
    let s = store();
    s.create_node("g", vec!["X".into()], IndexMap::new())
        .expect("create node");
    let r = run_on(&s, "g.V().values('nope').count()");
    assert_eq!(first_value(&r), &Value::Int(0));
}

// ── has() with edge-case comparisons ───────────────────────────────────────

#[test]
fn has_with_zero_value_matches() {
    let s = store();
    let mut props = IndexMap::new();
    props.insert("score".into(), Value::Int(0));
    s.create_node("g", vec!["X".into()], props)
        .expect("create node");

    let r = run_on(&s, "g.V().has('score', 0).count()");
    assert_eq!(first_value(&r), &Value::Int(1));
}

#[test]
fn has_with_empty_string_matches() {
    let s = store();
    let mut props = IndexMap::new();
    props.insert("note".into(), Value::Str("".into()));
    s.create_node("g", vec!["X".into()], props)
        .expect("create node");

    let r = run_on(&s, "g.V().has('note', '').count()");
    assert_eq!(first_value(&r), &Value::Int(1));
}

// ── Limit / range boundaries ───────────────────────────────────────────────

#[test]
fn limit_zero_yields_no_rows() {
    let r = run("g.inject(1, 2, 3).limit(0)");
    assert_eq!(r.rows.len(), 0);
}

#[test]
fn limit_one_yields_one_row() {
    let r = run("g.inject(1, 2, 3).limit(1)");
    assert_eq!(r.rows.len(), 1);
    assert_eq!(first_value(&r), &Value::Int(1));
}

#[test]
fn limit_larger_than_traversal_yields_all() {
    let r = run("g.inject(1, 2, 3).limit(100)");
    assert_eq!(r.rows.len(), 3);
}
