use indexmap::IndexMap;

use crate::storage::mem::test_helpers::{make_indexed_store, make_store};
use crate::storage::{GraphStore, RangeQuery};
use crate::types::Value;

#[test]
fn index_create_list_drop() {
    let store = make_store();
    store.create_index("test", "Person", "name").unwrap();
    store.create_index("test", "Person", "age").unwrap();

    let indexes = store.list_indexes("test").unwrap();
    assert_eq!(indexes.len(), 2);

    store.drop_index("test", "Person", "name").unwrap();
    let indexes = store.list_indexes("test").unwrap();
    assert_eq!(indexes.len(), 1);
    assert_eq!(indexes[0].property, "age");
}

#[test]
fn create_index_idempotent() {
    let store = make_store();
    store.create_index("test", "Person", "name").unwrap();
    store.create_index("test", "Person", "name").unwrap();
    assert_eq!(store.list_indexes("test").unwrap().len(), 1);
}

#[test]
fn indexed_eq_query() {
    let store = make_indexed_store();
    let p1 = IndexMap::from([("age".to_string(), Value::Int(30))]);
    let a = store
        .create_node("test", vec!["Person".to_string()], p1)
        .unwrap();
    let p2 = IndexMap::from([("age".to_string(), Value::Int(25))]);
    let _b = store
        .create_node("test", vec!["Person".to_string()], p2)
        .unwrap();

    let bound = Value::Int(30);
    let result = store
        .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Eq(&bound))
        .unwrap();
    assert_eq!(result, vec![a]);
}

#[test]
fn indexed_lt_lte_gt_gte_queries() {
    let store = make_indexed_store();
    let ages = vec![10, 20, 30, 40, 50];
    let mut ids = Vec::new();
    for age in &ages {
        let p = IndexMap::from([("age".to_string(), Value::Int(*age))]);
        ids.push(
            store
                .create_node("test", vec!["Person".to_string()], p)
                .unwrap(),
        );
    }

    let bound = Value::Int(30);
    let mut lt = store
        .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Lt(&bound))
        .unwrap();
    lt.sort();
    assert_eq!(lt, vec![ids[0], ids[1]]);

    let mut lte = store
        .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Lte(&bound))
        .unwrap();
    lte.sort();
    assert_eq!(lte, vec![ids[0], ids[1], ids[2]]);

    let mut gt = store
        .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Gt(&bound))
        .unwrap();
    gt.sort();
    assert_eq!(gt, vec![ids[3], ids[4]]);

    let mut gte = store
        .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Gte(&bound))
        .unwrap();
    gte.sort();
    assert_eq!(gte, vec![ids[2], ids[3], ids[4]]);
}

#[test]
fn indexed_between_query() {
    let store = make_indexed_store();
    let ages = vec![10, 20, 30, 40, 50];
    let mut ids = Vec::new();
    for age in &ages {
        let p = IndexMap::from([("age".to_string(), Value::Int(*age))]);
        ids.push(
            store
                .create_node("test", vec!["Person".to_string()], p)
                .unwrap(),
        );
    }

    let lo = Value::Int(20);
    let hi = Value::Int(40);
    let mut result = store
        .find_nodes_by_label_and_property(
            "test",
            "Person",
            "age",
            RangeQuery::Between {
                lo: &lo,
                hi: &hi,
                inclusive: true,
            },
        )
        .unwrap();
    result.sort();
    assert_eq!(result, vec![ids[1], ids[2], ids[3]]);

    let mut result = store
        .find_nodes_by_label_and_property(
            "test",
            "Person",
            "age",
            RangeQuery::Between {
                lo: &lo,
                hi: &hi,
                inclusive: false,
            },
        )
        .unwrap();
    result.sort();
    assert_eq!(result, vec![ids[1], ids[2]]);
}

#[test]
fn indexed_starts_with_query() {
    let store = make_indexed_store();
    let names = vec!["Alice", "Albert", "Bob", "Charlie"];
    let mut ids = Vec::new();
    for name in &names {
        let p = IndexMap::from([("name".to_string(), Value::Str((*name).into()))]);
        ids.push(
            store
                .create_node("test", vec!["Person".to_string()], p)
                .unwrap(),
        );
    }

    let mut result = store
        .find_nodes_by_label_and_property("test", "Person", "name", RangeQuery::StartsWith("Al"))
        .unwrap();
    result.sort();
    assert_eq!(result, vec![ids[0], ids[1]]);
}

#[test]
fn index_backfill_existing_nodes() {
    let store = make_store();
    let p1 = IndexMap::from([("age".to_string(), Value::Int(30))]);
    let a = store
        .create_node("test", vec!["Person".to_string()], p1)
        .unwrap();
    let p2 = IndexMap::from([("age".to_string(), Value::Int(25))]);
    let _b = store
        .create_node("test", vec!["Person".to_string()], p2)
        .unwrap();

    store.create_index("test", "Person", "age").unwrap();

    let bound = Value::Int(30);
    let result = store
        .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Eq(&bound))
        .unwrap();
    assert_eq!(result, vec![a]);
}

#[test]
fn index_maintained_on_property_update() {
    let store = make_indexed_store();
    let p = IndexMap::from([("age".to_string(), Value::Int(30))]);
    let a = store
        .create_node("test", vec!["Person".to_string()], p)
        .unwrap();

    store
        .set_node_properties(
            "test",
            a,
            IndexMap::from([("age".to_string(), Value::Int(40))]),
        )
        .unwrap();

    let bound30 = Value::Int(30);
    let result = store
        .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Eq(&bound30))
        .unwrap();
    assert!(result.is_empty());

    let bound40 = Value::Int(40);
    let result = store
        .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Eq(&bound40))
        .unwrap();
    assert_eq!(result, vec![a]);
}

#[test]
fn index_maintained_on_property_replace() {
    let store = make_indexed_store();
    let p = IndexMap::from([("age".to_string(), Value::Int(30))]);
    let a = store
        .create_node("test", vec!["Person".to_string()], p)
        .unwrap();

    store
        .replace_node_properties(
            "test",
            a,
            IndexMap::from([("age".to_string(), Value::Int(50))]),
        )
        .unwrap();

    let bound30 = Value::Int(30);
    assert!(store
        .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Eq(&bound30))
        .unwrap()
        .is_empty());

    let bound50 = Value::Int(50);
    assert_eq!(
        store
            .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Eq(&bound50))
            .unwrap(),
        vec![a]
    );
}

#[test]
fn index_maintained_on_property_remove() {
    let store = make_indexed_store();
    let p = IndexMap::from([("age".to_string(), Value::Int(30))]);
    let a = store
        .create_node("test", vec!["Person".to_string()], p)
        .unwrap();

    store.remove_node_property("test", a, "age").unwrap();

    let bound = Value::Int(30);
    let result = store
        .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Eq(&bound))
        .unwrap();
    assert!(result.is_empty());
}

#[test]
fn index_maintained_on_node_delete() {
    let store = make_indexed_store();
    let p = IndexMap::from([("age".to_string(), Value::Int(30))]);
    let a = store
        .create_node("test", vec!["Person".to_string()], p)
        .unwrap();

    store.delete_node("test", a, false).unwrap();

    let bound = Value::Int(30);
    let result = store
        .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Eq(&bound))
        .unwrap();
    assert!(result.is_empty());
}

#[test]
fn drop_index_cleans_btreemap() {
    let store = make_indexed_store();
    let p = IndexMap::from([("age".to_string(), Value::Int(30))]);
    let nid = store
        .create_node("test", vec!["Person".to_string()], p)
        .unwrap();

    store.drop_index("test", "Person", "age").unwrap();

    // After dropping the index, the query falls back to label scan +
    // property filter and still returns the matching node.
    let bound = Value::Int(30);
    let result = store
        .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Eq(&bound))
        .unwrap();
    assert_eq!(
        result,
        vec![nid],
        "fallback scan should still find the node"
    );
}

#[test]
fn index_maintained_on_add_label() {
    // Create a node without the indexed label, then add it.
    // The property index should pick up the node.
    let store = make_indexed_store(); // has index on Person.age and Person.name
    let p = IndexMap::from([("age".to_string(), Value::Int(42))]);
    let a = store
        .create_node("test", vec!["Animal".to_string()], p)
        .unwrap();

    // Before adding label: index lookup should not find it.
    let bound = Value::Int(42);
    let result = store
        .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Eq(&bound))
        .unwrap();
    assert!(result.is_empty());

    // Add the Person label.
    store
        .add_node_labels("test", a, &["Person".to_string()])
        .unwrap();

    // Now the index should find it.
    let result = store
        .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Eq(&bound))
        .unwrap();
    assert_eq!(result, vec![a]);
}

#[test]
fn index_maintained_on_remove_label() {
    // Create a node with the indexed label, then remove it.
    // The property index should no longer find the node.
    let store = make_indexed_store();
    let p = IndexMap::from([("age".to_string(), Value::Int(55))]);
    let a = store
        .create_node("test", vec!["Person".to_string()], p)
        .unwrap();

    // Before removing label: index lookup should find it.
    let bound = Value::Int(55);
    let result = store
        .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Eq(&bound))
        .unwrap();
    assert_eq!(result, vec![a]);

    // Remove the Person label.
    store
        .remove_node_labels("test", a, &["Person".to_string()])
        .unwrap();

    // Now the index should no longer find it.
    let result = store
        .find_nodes_by_label_and_property("test", "Person", "age", RangeQuery::Eq(&bound))
        .unwrap();
    assert!(result.is_empty());
}

#[test]
fn index_scan_returns_correct_results_end_to_end() {
    // End-to-end: create index, insert nodes, verify that
    // find_nodes_by_label_and_property returns only matching nodes
    // and uses the index (not a fallback scan).
    let store = make_store();
    // Insert nodes BEFORE creating the index to test backfill.
    let p1 = IndexMap::from([("name".to_string(), Value::Str("Alice".into()))]);
    let alice = store
        .create_node("test", vec!["Person".to_string()], p1)
        .unwrap();
    let p2 = IndexMap::from([("name".to_string(), Value::Str("Bob".into()))]);
    let _bob = store
        .create_node("test", vec!["Person".to_string()], p2)
        .unwrap();
    let p3 = IndexMap::from([("name".to_string(), Value::Str("Alice".into()))]);
    let _other = store
        .create_node("test", vec!["Company".to_string()], p3)
        .unwrap();

    // Create index — should backfill existing Person nodes.
    store.create_index("test", "Person", "name").unwrap();

    // Exact match: only Alice with label Person.
    let bound = Value::Str("Alice".into());
    let result = store
        .find_nodes_by_label_and_property("test", "Person", "name", RangeQuery::Eq(&bound))
        .unwrap();
    assert_eq!(result, vec![alice]);

    // Insert another node AFTER creating the index.
    let p4 = IndexMap::from([("name".to_string(), Value::Str("Alice".into()))]);
    let alice2 = store
        .create_node("test", vec!["Person".to_string()], p4)
        .unwrap();

    // Now should find both Alices.
    let mut result = store
        .find_nodes_by_label_and_property("test", "Person", "name", RangeQuery::Eq(&bound))
        .unwrap();
    result.sort();
    let mut expected = vec![alice, alice2];
    expected.sort();
    assert_eq!(result, expected);
}
