//! Tests for node CRUD operations.

use indexmap::IndexMap;

use crate::storage::mem::test_helpers::make_store;
use crate::storage::{GraphStore, NodeId};
use crate::types::Value;

#[test]
fn node_create_read_delete_roundtrip() {
    let store = make_store();
    let mut props = IndexMap::new();
    props.insert("name".to_string(), Value::Str("Alice".into()));
    let id = store
        .create_node("test", vec!["Person".to_string()], props.clone())
        .unwrap();

    let node = store
        .get_node("test", id)
        .unwrap()
        .expect("node should exist");
    assert_eq!(node.id, id);
    assert_eq!(node.labels, vec!["Person".to_string()]);
    assert_eq!(node.properties["name"], Value::Str("Alice".into()));

    store.delete_node("test", id, false).unwrap();
    assert!(store.get_node("test", id).unwrap().is_none());
}

#[test]
fn delete_node_without_detach_fails_if_has_edges() {
    let store = make_store();
    let a = store.create_node("test", vec![], IndexMap::new()).unwrap();
    let b = store.create_node("test", vec![], IndexMap::new()).unwrap();
    store
        .create_edge("test", a, b, "R".to_string(), IndexMap::new())
        .unwrap();

    let result = store.delete_node("test", a, false);
    assert!(result.is_err(), "should refuse to delete connected node");
}

#[test]
fn delete_node_with_detach_removes_edges() {
    let store = make_store();
    let a = store.create_node("test", vec![], IndexMap::new()).unwrap();
    let b = store.create_node("test", vec![], IndexMap::new()).unwrap();
    let eid = store
        .create_edge("test", a, b, "R".to_string(), IndexMap::new())
        .unwrap();

    store.delete_node("test", a, true).unwrap();
    assert!(store.get_node("test", a).unwrap().is_none());
    assert!(store.get_edge("test", eid).unwrap().is_none());
    assert!(store.in_edges("test", b).unwrap().is_empty());
}

#[test]
fn detach_delete_cleans_both_directions() {
    let store = make_store();
    let a = store.create_node("test", vec![], IndexMap::new()).unwrap();
    let b = store.create_node("test", vec![], IndexMap::new()).unwrap();
    let e1 = store
        .create_edge("test", a, b, "A_TO_B".to_string(), IndexMap::new())
        .unwrap();
    let e2 = store
        .create_edge("test", b, a, "B_TO_A".to_string(), IndexMap::new())
        .unwrap();

    store.delete_node("test", a, true).unwrap();

    assert!(store.get_edge("test", e1).unwrap().is_none());
    assert!(store.get_edge("test", e2).unwrap().is_none());
    assert!(store.out_edges("test", b).unwrap().is_empty());
    assert!(store.in_edges("test", b).unwrap().is_empty());
}

#[test]
fn set_and_remove_node_properties() {
    let store = make_store();
    let id = store.create_node("test", vec![], IndexMap::new()).unwrap();

    let mut props = IndexMap::new();
    props.insert("x".to_string(), Value::Int(1));
    store.set_node_properties("test", id, props).unwrap();

    let node = store.get_node("test", id).unwrap().unwrap();
    assert_eq!(node.properties["x"], Value::Int(1));

    store.remove_node_property("test", id, "x").unwrap();
    let node = store.get_node("test", id).unwrap().unwrap();
    assert!(!node.properties.contains_key("x"));
}

#[test]
fn add_and_remove_labels() {
    let store = make_store();
    let id = store
        .create_node("test", vec!["A".to_string()], IndexMap::new())
        .unwrap();

    store
        .add_node_labels("test", id, &["B".to_string()])
        .unwrap();
    let node = store.get_node("test", id).unwrap().unwrap();
    assert!(node.labels.contains(&"B".to_string()));

    let b_nodes = store.find_nodes_by_label("test", "B").unwrap();
    assert!(b_nodes.contains(&id));

    store
        .remove_node_labels("test", id, &["A".to_string()])
        .unwrap();
    let node = store.get_node("test", id).unwrap().unwrap();
    assert!(!node.labels.contains(&"A".to_string()));
    let a_nodes = store.find_nodes_by_label("test", "A").unwrap();
    assert!(!a_nodes.contains(&id));
}

#[test]
fn batch_fetch_nodes() {
    let store = make_store();
    let a = store.create_node("test", vec![], IndexMap::new()).unwrap();
    let b = store.create_node("test", vec![], IndexMap::new()).unwrap();
    let fake = NodeId(9999);

    let results = store.get_nodes_batch("test", &[a, fake, b]).unwrap();
    assert_eq!(results.len(), 3);
    assert!(results[0].is_some());
    assert!(results[1].is_none());
    assert!(results[2].is_some());
}

#[test]
fn all_node_ids_returns_all() {
    let store = make_store();
    let a = store.create_node("test", vec![], IndexMap::new()).unwrap();
    let b = store.create_node("test", vec![], IndexMap::new()).unwrap();
    let mut ids = store.all_node_ids("test").unwrap();
    ids.sort();
    assert_eq!(ids, vec![a, b]);
}

// ── Bug #1204: Node collision detection for _sd2_id ─────────────────────

#[test]
fn create_node_with_duplicate_sd2_id_returns_error() {
    let store = make_store();
    let mut props1 = IndexMap::new();
    props1.insert("_sd2_id".to_string(), Value::Int(5));
    props1.insert("name".to_string(), Value::Str("Alice".into()));
    let nid = store
        .create_node("test", vec!["Person".to_string()], props1)
        .unwrap();
    assert_eq!(nid, NodeId(5));

    // Second create with same _sd2_id must be rejected
    let mut props2 = IndexMap::new();
    props2.insert("_sd2_id".to_string(), Value::Int(5));
    props2.insert("name".to_string(), Value::Str("Bob".into()));
    let result = store.create_node("test", vec!["Person".to_string()], props2);
    assert!(
        result.is_err(),
        "must reject duplicate _sd2_id; got {:?}",
        result
    );
}

#[test]
fn create_node_duplicate_sd2_id_does_not_corrupt_label_index() {
    let store = make_store();
    let mut props1 = IndexMap::new();
    props1.insert("_sd2_id".to_string(), Value::Int(5));
    store
        .create_node("test", vec!["Person".to_string()], props1)
        .unwrap();

    // Attempt to overwrite with different labels
    let mut props2 = IndexMap::new();
    props2.insert("_sd2_id".to_string(), Value::Int(5));
    let _ = store.create_node("test", vec!["Company".to_string()], props2);

    // Original node must still be findable under its original label
    let persons = store.find_nodes_by_label("test", "Person").unwrap();
    assert!(
        persons.contains(&NodeId(5)),
        "original node must remain indexed under Person"
    );

    // Rejected label must NOT be in the index
    let companies = store.find_nodes_by_label("test", "Company").unwrap();
    assert!(
        !companies.contains(&NodeId(5)),
        "rejected node must not be indexed under Company"
    );
}
