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

// ── Read delegation to primary ───────────────────────────────────────────

#[test]
fn test_list_graphs_comes_from_primary_only() {
    let (primary_ref, primary_box) = shared_store();
    let (_secondary_ref, secondary_box) = shared_store();
    let fanout = FanoutStore::new(vec![primary_box, secondary_box]);

    fanout.create_graph("a").unwrap();
    fanout.create_graph("b").unwrap();
    // Both should be in primary.
    assert_eq!(primary_ref.list_graphs().unwrap().len(), 2);

    let listed = fanout.list_graphs().unwrap();
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().any(|g| g == "a"));
    assert!(listed.iter().any(|g| g == "b"));
}

#[test]
fn test_graph_exists_and_graph_stats_delegate_to_primary() {
    let fanout = FanoutStore::new(vec![mem_store(), mem_store()]);
    fanout.create_graph("g").unwrap();
    assert!(fanout.graph_exists("g").unwrap());
    assert!(!fanout.graph_exists("missing").unwrap());

    let stats = fanout.graph_stats("g").unwrap();
    assert_eq!(stats.node_count, 0);
    assert_eq!(stats.edge_count, 0);

    fanout
        .create_node("g", vec!["L".into()], IndexMap::new())
        .unwrap();
    let stats = fanout.graph_stats("g").unwrap();
    assert_eq!(stats.node_count, 1);
}

#[test]
fn test_finders_and_adjacency_delegate_to_primary() {
    use cypher_core::storage::RangeQuery;

    let fanout = FanoutStore::new(vec![mem_store(), mem_store()]);
    fanout.create_graph("g").unwrap();

    let mut p = IndexMap::new();
    p.insert("age".into(), Value::Int(30));
    let alice = fanout.create_node("g", vec!["Person".into()], p).unwrap();

    let mut p = IndexMap::new();
    p.insert("age".into(), Value::Int(25));
    let bob = fanout.create_node("g", vec!["Person".into()], p).unwrap();

    let eid = fanout
        .create_edge("g", alice, bob, "KNOWS".into(), IndexMap::new())
        .unwrap();

    let by_label = fanout.find_nodes_by_label("g", "Person").unwrap();
    assert_eq!(by_label.len(), 2);

    let thirty = Value::Int(30);
    let range = RangeQuery::Eq(&thirty);
    let by_prop = fanout
        .find_nodes_by_label_and_property("g", "Person", "age", range)
        .unwrap();
    assert_eq!(by_prop, vec![alice]);

    let edges_t = fanout.find_edges_by_type("g", "KNOWS").unwrap();
    assert_eq!(edges_t, vec![eid]);

    assert_eq!(fanout.out_edges("g", alice).unwrap().len(), 1);
    assert_eq!(fanout.in_edges("g", bob).unwrap().len(), 1);
    assert_eq!(
        fanout
            .out_edges_by_type("g", alice, &["KNOWS"])
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        fanout.in_edges_by_type("g", bob, &["KNOWS"]).unwrap().len(),
        1
    );
    assert_eq!(fanout.out_edge_ids("g", alice).unwrap(), vec![eid]);
    assert_eq!(fanout.in_edge_ids("g", bob).unwrap(), vec![eid]);
    assert_eq!(fanout.all_node_ids("g").unwrap().len(), 2);
    assert_eq!(fanout.all_edge_ids("g").unwrap(), vec![eid]);
    let labels = fanout.all_label_names("g").unwrap();
    assert!(labels.contains(&"Person".to_string()));
}

#[test]
fn test_batch_reads_delegate_to_primary() {
    let fanout = FanoutStore::new(vec![mem_store(), mem_store()]);
    fanout.create_graph("g").unwrap();
    let a = fanout
        .create_node("g", vec!["L".into()], IndexMap::new())
        .unwrap();
    let b = fanout
        .create_node("g", vec!["L".into()], IndexMap::new())
        .unwrap();
    let eid = fanout
        .create_edge("g", a, b, "R".into(), IndexMap::new())
        .unwrap();

    let nodes = fanout.get_nodes_batch("g", &[a, b, NodeId(999)]).unwrap();
    assert_eq!(nodes.len(), 3);
    assert!(nodes[0].is_some());
    assert!(nodes[1].is_some());
    assert!(nodes[2].is_none());

    let edges = fanout.get_edges_batch("g", &[eid, EdgeId(999)]).unwrap();
    assert_eq!(edges.len(), 2);
    assert!(edges[0].is_some());
    assert!(edges[1].is_none());
}

#[test]
fn test_property_mutations_propagate() {
    let (primary_ref, primary_box) = shared_store();
    let (secondary_ref, secondary_box) = shared_store();
    let fanout = FanoutStore::new(vec![primary_box, secondary_box]);

    fanout.create_graph("g").unwrap();
    let nid = fanout
        .create_node("g", vec!["L".into()], IndexMap::new())
        .unwrap();
    let a = fanout
        .create_node("g", vec!["L".into()], IndexMap::new())
        .unwrap();
    let eid = fanout
        .create_edge("g", nid, a, "R".into(), IndexMap::new())
        .unwrap();

    let mut p = IndexMap::new();
    p.insert("x".into(), Value::Int(1));
    fanout.set_node_properties("g", nid, p.clone()).unwrap();
    assert!(primary_ref
        .get_node("g", nid)
        .unwrap()
        .unwrap()
        .properties
        .contains_key("x"));
    assert!(secondary_ref
        .get_node("g", nid)
        .unwrap()
        .unwrap()
        .properties
        .contains_key("x"));

    fanout
        .replace_node_properties("g", nid, IndexMap::new())
        .unwrap();
    assert!(primary_ref
        .get_node("g", nid)
        .unwrap()
        .unwrap()
        .properties
        .is_empty());

    fanout.set_edge_properties("g", eid, p.clone()).unwrap();
    assert!(primary_ref
        .get_edge("g", eid)
        .unwrap()
        .unwrap()
        .properties
        .contains_key("x"));

    fanout
        .replace_edge_properties("g", eid, IndexMap::new())
        .unwrap();
    assert!(primary_ref
        .get_edge("g", eid)
        .unwrap()
        .unwrap()
        .properties
        .is_empty());

    // remove_* on absent key is a no-op in InMemoryGraphStore.
    fanout.remove_node_property("g", nid, "missing").unwrap();
    fanout.remove_edge_property("g", eid, "missing").unwrap();
}

// ── as_any round-trip (downcast back to FanoutStore) ─────────────────────

#[test]
fn test_as_any_downcasts_to_fanout_store() {
    let fanout = FanoutStore::new(vec![mem_store()]);
    let any = fanout.as_any();
    assert!(any.downcast_ref::<FanoutStore>().is_some());
}

#[test]
#[should_panic(expected = "at least one backend")]
fn test_new_with_empty_vec_panics() {
    let _ = FanoutStore::new(vec![]);
}
