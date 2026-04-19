//! Regression tests for issue #1710.
//!
//! `g.V().has('value', X)` (and `'id'`, `'label'`, `'key'`) returned 0 hits
//! because the Gremlin filter property resolver intercepted the bare
//! lowercase forms as Gremlin T-tokens (`T.value`, `T.id`, ...) and returned
//! the element itself instead of looking up the user property of the same
//! name. See `cypher-core/src/executor/engine/gremlin/select_helpers.rs`.

use indexmap::IndexMap;
use sedan_gremlin_embedded::{GraphStore, GremlinEngine, InMemoryGraphStore, Value};

fn store_with_prop(prop: &str) -> InMemoryGraphStore {
    let store = InMemoryGraphStore::default();
    store.create_graph("g").unwrap();
    for i in 1..=5i64 {
        store
            .create_node(
                "g",
                vec!["N".into()],
                IndexMap::from([(prop.into(), Value::Int(i))]),
            )
            .unwrap();
    }
    store
}

fn count(store: &InMemoryGraphStore, query: &str) -> i64 {
    let result = GremlinEngine::new()
        .execute(store, "g", query)
        .expect("query should execute");
    assert_eq!(result.rows.len(), 1, "count() must produce one row");
    let row = &result.rows[0];
    let (_, v) = row.iter().next().expect("count row should have one column");
    match v {
        Value::Int(n) => *n,
        other => panic!("expected Int count, got {other:?}"),
    }
}

#[test]
fn has_value_property_filters_correctly() {
    // Original repro from #1710: property literally named "value" was shadowed.
    let store = store_with_prop("value");
    assert_eq!(count(&store, "g.V().has('value', 3).count()"), 1);
    assert_eq!(count(&store, "g.V().has('value', 99).count()"), 0);
    assert_eq!(count(&store, "g.V().has('value').count()"), 5);
}

#[test]
fn has_id_property_filters_correctly() {
    // Property literally named "id" should not be confused with T.id.
    let store = store_with_prop("id");
    assert_eq!(count(&store, "g.V().has('id', 3).count()"), 1);
    assert_eq!(count(&store, "g.V().has('id', 99).count()"), 0);
}

#[test]
fn has_label_property_filters_correctly() {
    // Property literally named "label" should not be confused with T.label
    // (which is what hasLabel and has(T.label, ...) target).
    let store = store_with_prop("label");
    assert_eq!(count(&store, "g.V().has('label', 3).count()"), 1);
    assert_eq!(count(&store, "g.V().has('label', 99).count()"), 0);
}

#[test]
fn has_key_property_filters_correctly() {
    let store = store_with_prop("key");
    assert_eq!(count(&store, "g.V().has('key', 3).count()"), 1);
    assert_eq!(count(&store, "g.V().has('key', 99).count()"), 0);
}

#[test]
fn values_returns_value_property_not_node() {
    // Sibling regression: `values('value')` previously returned the Node
    // itself rather than the property value.
    let store = store_with_prop("value");
    let result = GremlinEngine::new()
        .execute(&store, "g", "g.V().values('value')")
        .unwrap();
    let mut got: Vec<i64> = result
        .rows
        .iter()
        .map(|row| match row.iter().next().unwrap().1 {
            Value::Int(n) => *n,
            ref other => panic!("expected Int, got {other:?}"),
        })
        .collect();
    got.sort_unstable();
    assert_eq!(got, vec![1, 2, 3, 4, 5]);
}
