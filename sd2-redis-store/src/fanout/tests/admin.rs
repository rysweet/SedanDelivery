// Tests for FanoutStore — admin behavior.

use super::*;

#[test]
fn test_delete_graph_propagates_to_both() {
    let (primary_ref, primary_box) = shared_store();
    let (secondary_ref, secondary_box) = shared_store();
    let fanout = FanoutStore::new(vec![primary_box, secondary_box]);

    fanout.create_graph("g").unwrap();
    assert!(primary_ref.graph_exists("g").unwrap());
    assert!(secondary_ref.graph_exists("g").unwrap());

    fanout.delete_graph("g").unwrap();
    assert!(
        !primary_ref.graph_exists("g").unwrap(),
        "primary should not have graph after delete"
    );
    assert!(
        !secondary_ref.graph_exists("g").unwrap(),
        "secondary should not have graph after delete"
    );
}

// ── Index operations propagate to both stores ───────────────────────────

#[test]
fn test_index_operations_propagate() {
    let (primary_ref, primary_box) = shared_store();
    let (secondary_ref, secondary_box) = shared_store();
    let fanout = FanoutStore::new(vec![primary_box, secondary_box]);

    fanout.create_graph("g").unwrap();
    fanout.create_index("g", "Person", "age").unwrap();

    // Both stores should have the index.
    let p_indexes = primary_ref.list_indexes("g").unwrap();
    assert_eq!(p_indexes.len(), 1);
    assert_eq!(p_indexes[0].label, "Person");
    assert_eq!(p_indexes[0].property, "age");

    let s_indexes = secondary_ref.list_indexes("g").unwrap();
    assert_eq!(s_indexes.len(), 1);
    assert_eq!(s_indexes[0].label, "Person");

    // Drop index propagates too.
    fanout.drop_index("g", "Person", "age").unwrap();
    assert!(primary_ref.list_indexes("g").unwrap().is_empty());
    assert!(secondary_ref.list_indexes("g").unwrap().is_empty());
}

// ── Label operations propagate via write_all ────────────────────────────

#[test]
fn test_add_remove_labels_propagate() {
    let (primary_ref, primary_box) = shared_store();
    let (_secondary_ref, secondary_box) = shared_store();
    let fanout = FanoutStore::new(vec![primary_box, secondary_box]);

    fanout.create_graph("g").unwrap();
    let node_id = fanout
        .create_node("g", vec!["Person".into()], IndexMap::new())
        .unwrap();

    fanout
        .add_node_labels("g", node_id, &["Employee".to_string()])
        .unwrap();
    let node = primary_ref.get_node("g", node_id).unwrap().unwrap();
    assert!(node.labels.contains(&"Person".to_string()));
    assert!(node.labels.contains(&"Employee".to_string()));

    fanout
        .remove_node_labels("g", node_id, &["Person".to_string()])
        .unwrap();
    let node = primary_ref.get_node("g", node_id).unwrap().unwrap();
    assert!(!node.labels.contains(&"Person".to_string()));
    assert!(node.labels.contains(&"Employee".to_string()));
}
