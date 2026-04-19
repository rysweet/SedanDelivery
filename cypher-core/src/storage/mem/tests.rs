//! Integration tests for the in-memory `GraphStore` implementation, covering
//! snapshot isolation, graph-level CRUD, and uniqueness constraints.

use super::test_helpers::make_store;
use super::*;
use indexmap::IndexMap;

#[test]
fn graph_not_found_errors() {
    let store = InMemoryGraphStore::new();
    let result = store.get_node("no_such_graph", NodeId(1));
    assert!(matches!(result, Err(StorageError::GraphNotFound { .. })));

    let result = store.find_nodes_by_label("no_such_graph", "Person");
    assert!(matches!(result, Err(StorageError::GraphNotFound { .. })));
}

#[test]
fn duplicate_graph_creation_fails() {
    let store = InMemoryGraphStore::new();
    store.create_graph("mygraph").unwrap();
    let result = store.create_graph("mygraph");
    assert!(matches!(
        result,
        Err(StorageError::GraphAlreadyExists { .. })
    ));
}

#[test]
fn invalid_graph_name_on_create() {
    let store = InMemoryGraphStore::new();
    assert!(store.create_graph("bad:name").is_err());
    assert!(store.create_graph("{slot}graph").is_err());
    assert!(store.create_graph("").is_err());
}

#[test]
fn graph_exists_and_list() {
    let store = InMemoryGraphStore::new();
    assert!(!store.graph_exists("g1").unwrap());
    store.create_graph("g1").unwrap();
    store.create_graph("g2").unwrap();
    assert!(store.graph_exists("g1").unwrap());
    let mut names = store.list_graphs().unwrap();
    names.sort();
    assert_eq!(names, vec!["g1", "g2"]);
}

#[test]
fn graph_stats_correct() {
    let store = make_store();
    let a = store
        .create_node("test", vec!["Person".to_string()], IndexMap::new())
        .unwrap();
    let b = store
        .create_node("test", vec!["Person".to_string()], IndexMap::new())
        .unwrap();
    store
        .create_edge("test", a, b, "KNOWS".to_string(), IndexMap::new())
        .unwrap();

    let stats = store.graph_stats("test").unwrap();
    assert_eq!(stats.node_count, 2);
    assert_eq!(stats.edge_count, 1);
    assert_eq!(stats.label_count, 1);
    assert_eq!(stats.rel_type_count, 1);
}

#[test]
fn snapshot_creates_independent_copy() {
    let store = make_store();
    let mut props = IndexMap::new();
    props.insert("name".to_string(), Value::Str("Alice".into()));
    let id = store
        .create_node("test", vec!["Person".to_string()], props)
        .unwrap();

    let snap = store.snapshot(&["test"]).unwrap();

    let node = snap.get_node("test", id).unwrap().unwrap();
    assert_eq!(node.properties["name"], Value::Str("Alice".into()));

    snap.create_node("test", vec!["Extra".to_string()], IndexMap::new())
        .unwrap();
    let snap_stats = snap.graph_stats("test").unwrap();
    let orig_stats = store.graph_stats("test").unwrap();
    assert_eq!(snap_stats.node_count, 2);
    assert_eq!(orig_stats.node_count, 1);
}

#[test]
fn snapshot_skips_missing_graphs() {
    let store = make_store();
    let snap = store.snapshot(&["test", "nonexistent"]).unwrap();
    assert!(snap.graph_exists("test").unwrap());
    assert!(!snap.graph_exists("nonexistent").unwrap());
}

#[test]
fn merge_from_overwrites_existing_graph() {
    let store = make_store();
    store
        .create_node("test", vec!["Original".to_string()], IndexMap::new())
        .unwrap();
    assert_eq!(store.graph_stats("test").unwrap().node_count, 1);

    let snap = store.snapshot(&["test"]).unwrap();
    snap.create_node("test", vec!["Added".to_string()], IndexMap::new())
        .unwrap();
    assert_eq!(snap.graph_stats("test").unwrap().node_count, 2);

    store.merge_from(&snap).unwrap();
    assert_eq!(store.graph_stats("test").unwrap().node_count, 2);
}

#[test]
fn merge_from_adds_new_graphs() {
    let store = make_store();
    let other = InMemoryGraphStore::new();
    other.create_graph("new_graph").unwrap();
    other
        .create_node("new_graph", vec![], IndexMap::new())
        .unwrap();

    store.merge_from(&other).unwrap();
    assert!(store.graph_exists("new_graph").unwrap());
    assert_eq!(store.graph_stats("new_graph").unwrap().node_count, 1);
}

// ── Constraint tests ─────────────────────────────────────────────────

#[test]
fn create_uniqueness_constraint() {
    let store = make_store();
    store
        .create_constraint("test", "Person", "email", ConstraintType::Unique)
        .unwrap();
    let constraints = store.list_constraints("test").unwrap();
    assert_eq!(constraints.len(), 1);
    assert_eq!(constraints[0].label, "Person");
    assert_eq!(constraints[0].property, "email");
}

#[test]
fn constraint_blocks_duplicate_on_create() {
    let store = make_store();
    store
        .create_constraint("test", "Person", "email", ConstraintType::Unique)
        .unwrap();

    let mut props = IndexMap::new();
    props.insert("email".to_string(), Value::Str("alice@example.com".into()));
    store
        .create_node("test", vec!["Person".to_string()], props.clone())
        .unwrap();

    // Second node with the same email should fail.
    let result = store.create_node("test", vec!["Person".to_string()], props);
    assert!(
        matches!(result, Err(StorageError::ConstraintViolation { .. })),
        "Expected ConstraintViolation, got {:?}",
        result
    );
}

#[test]
fn constraint_allows_different_values() {
    let store = make_store();
    store
        .create_constraint("test", "Person", "email", ConstraintType::Unique)
        .unwrap();

    let mut props1 = IndexMap::new();
    props1.insert("email".to_string(), Value::Str("alice@example.com".into()));
    store
        .create_node("test", vec!["Person".to_string()], props1)
        .unwrap();

    let mut props2 = IndexMap::new();
    props2.insert("email".to_string(), Value::Str("bob@example.com".into()));
    store
        .create_node("test", vec!["Person".to_string()], props2)
        .unwrap();
}

#[test]
fn constraint_blocks_duplicate_on_set_properties() {
    let store = make_store();
    store
        .create_constraint("test", "Person", "email", ConstraintType::Unique)
        .unwrap();

    let mut props1 = IndexMap::new();
    props1.insert("email".to_string(), Value::Str("alice@example.com".into()));
    store
        .create_node("test", vec!["Person".to_string()], props1)
        .unwrap();

    let mut props2 = IndexMap::new();
    props2.insert("email".to_string(), Value::Str("bob@example.com".into()));
    let id2 = store
        .create_node("test", vec!["Person".to_string()], props2)
        .unwrap();

    // Try to update id2's email to alice's email.
    let mut update = IndexMap::new();
    update.insert("email".to_string(), Value::Str("alice@example.com".into()));
    let result = store.set_node_properties("test", id2, update);
    assert!(matches!(
        result,
        Err(StorageError::ConstraintViolation { .. })
    ));
}

#[test]
fn constraint_creation_fails_with_existing_duplicates() {
    let store = make_store();
    let mut props = IndexMap::new();
    props.insert("email".to_string(), Value::Str("alice@example.com".into()));
    store
        .create_node("test", vec!["Person".to_string()], props.clone())
        .unwrap();
    store
        .create_node("test", vec!["Person".to_string()], props)
        .unwrap();

    let result = store.create_constraint("test", "Person", "email", ConstraintType::Unique);
    assert!(matches!(
        result,
        Err(StorageError::ConstraintViolation { .. })
    ));
}

#[test]
fn drop_constraint_removes_it() {
    let store = make_store();
    store
        .create_constraint("test", "Person", "email", ConstraintType::Unique)
        .unwrap();
    assert_eq!(store.list_constraints("test").unwrap().len(), 1);

    store.drop_constraint("test", "Person", "email").unwrap();
    assert_eq!(store.list_constraints("test").unwrap().len(), 0);
}

#[test]
fn drop_constraint_allows_duplicate_after() {
    let store = make_store();
    store
        .create_constraint("test", "Person", "email", ConstraintType::Unique)
        .unwrap();

    let mut props = IndexMap::new();
    props.insert("email".to_string(), Value::Str("alice@example.com".into()));
    store
        .create_node("test", vec!["Person".to_string()], props.clone())
        .unwrap();

    store.drop_constraint("test", "Person", "email").unwrap();

    // Now duplicate should succeed.
    store
        .create_node("test", vec!["Person".to_string()], props)
        .unwrap();
}

#[test]
fn constraint_does_not_apply_to_different_labels() {
    let store = make_store();
    store
        .create_constraint("test", "Person", "email", ConstraintType::Unique)
        .unwrap();

    let mut props = IndexMap::new();
    props.insert("email".to_string(), Value::Str("alice@example.com".into()));
    store
        .create_node("test", vec!["Person".to_string()], props.clone())
        .unwrap();

    // Creating a Company node with the same email should succeed.
    store
        .create_node("test", vec!["Company".to_string()], props)
        .unwrap();
}
