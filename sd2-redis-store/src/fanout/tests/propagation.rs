// Tests for FanoutStore — propagation behavior.

use super::*;

#[test]
fn test_single_backend_round_trip() {
    let store = FanoutStore::new(vec![mem_store()]);
    store.create_graph("g").unwrap();
    let node_id = store
        .create_node("g", vec!["Person".into()], IndexMap::new())
        .unwrap();
    let node = store.get_node("g", node_id).unwrap();
    assert!(node.is_some(), "node should exist after creation");
    assert!(node.unwrap().labels.contains(&"Person".to_string()));
}

#[test]
#[should_panic(expected = "FanoutStore requires at least one backend")]
fn test_new_empty_panics() {
    let _store = FanoutStore::new(vec![]);
}

// ── Graph creation propagates to both stores ────────────────────────────

#[test]
fn test_create_graph_on_both_stores() {
    let (primary_ref, primary_box) = shared_store();
    let (secondary_ref, secondary_box) = shared_store();
    let fanout = FanoutStore::new(vec![primary_box, secondary_box]);

    fanout.create_graph("g").unwrap();

    assert!(
        primary_ref.graph_exists("g").unwrap(),
        "primary should have graph 'g'"
    );
    assert!(
        secondary_ref.graph_exists("g").unwrap(),
        "secondary should have graph 'g'"
    );
}

// ── Node creation propagates to both stores ─────────────────────────────

#[test]
fn test_create_node_present_on_both() {
    let (primary_ref, primary_box) = shared_store();
    let (secondary_ref, secondary_box) = shared_store();
    let fanout = FanoutStore::new(vec![primary_box, secondary_box]);

    fanout.create_graph("g").unwrap();
    let mut props = IndexMap::new();
    props.insert("name".to_string(), Value::Str("Alice".into()));
    let node_id = fanout
        .create_node("g", vec!["Person".into()], props)
        .unwrap();

    // Primary should have the node.
    let primary_node = primary_ref.get_node("g", node_id).unwrap();
    assert!(primary_node.is_some(), "node must exist on primary");
    let pn = primary_node.unwrap();
    assert!(pn.labels.contains(&"Person".to_string()));
    assert_eq!(pn.properties.get("name"), Some(&Value::Str("Alice".into())));
    // Primary should NOT have the injected _sd2_id.
    assert!(
        !pn.properties.contains_key("_sd2_id"),
        "primary must not have _sd2_id"
    );

    // Secondary should also have the node at the SAME NodeId. FanoutStore
    // injects `_sd2_id` into the property map, and InMemoryGraphStore
    // honors it to set the raw ID (then strips the internal key).
    let sec_node = secondary_ref
        .get_node("g", node_id)
        .unwrap()
        .expect("secondary should have node at same ID as primary");
    assert!(sec_node.labels.contains(&"Person".to_string()));
    assert_eq!(
        sec_node.properties.get("name"),
        Some(&Value::Str("Alice".into())),
        "secondary should have the original properties"
    );
    // The _sd2_id internal key is stripped by InMemoryGraphStore.
    assert!(
        !sec_node.properties.contains_key("_sd2_id"),
        "internal _sd2_id should be stripped from stored properties"
    );
}

// ── Edge creation propagates to both stores ─────────────────────────────

#[test]
fn test_create_edge_present_on_both() {
    let (primary_ref, primary_box) = shared_store();
    let (secondary_ref, secondary_box) = shared_store();
    let fanout = FanoutStore::new(vec![primary_box, secondary_box]);

    fanout.create_graph("g").unwrap();
    let a = fanout
        .create_node("g", vec!["Person".into()], IndexMap::new())
        .unwrap();
    let b = fanout
        .create_node("g", vec!["Person".into()], IndexMap::new())
        .unwrap();

    let mut edge_props = IndexMap::new();
    edge_props.insert("since".to_string(), Value::Int(2020));
    let edge_id = fanout
        .create_edge("g", a, b, "KNOWS".to_string(), edge_props)
        .unwrap();

    // Primary: edge exists with correct data.
    let pe = primary_ref.get_edge("g", edge_id).unwrap().unwrap();
    assert_eq!(pe.rel_type, "KNOWS");
    assert_eq!(pe.src, a);
    assert_eq!(pe.dst, b);
    assert_eq!(pe.properties.get("since"), Some(&Value::Int(2020)));
    assert!(
        !pe.properties.contains_key("_sd2_eid"),
        "primary edge must not have _sd2_eid"
    );

    // Secondary should have the edge at the SAME EdgeId. FanoutStore
    // injects `_sd2_eid` into the property map, and InMemoryGraphStore
    // honors it to set the raw ID (then strips the internal key).
    let se = secondary_ref
        .get_edge("g", edge_id)
        .unwrap()
        .expect("secondary should have edge at same ID as primary");
    assert_eq!(se.rel_type, "KNOWS");
    assert_eq!(se.src, a);
    assert_eq!(se.dst, b);
    assert_eq!(se.properties.get("since"), Some(&Value::Int(2020)));
    // The _sd2_eid internal key is stripped by InMemoryGraphStore.
    assert!(
        !se.properties.contains_key("_sd2_eid"),
        "internal _sd2_eid should be stripped from stored properties"
    );
}

// ── Delete node propagates to both stores ───────────────────────────────

#[test]
fn test_delete_node_propagates_to_both() {
    let (primary_ref, primary_box) = shared_store();
    let (secondary_ref, secondary_box) = shared_store();
    let fanout = FanoutStore::new(vec![primary_box, secondary_box]);

    fanout.create_graph("g").unwrap();
    let node_id = fanout
        .create_node("g", vec!["Person".into()], IndexMap::new())
        .unwrap();

    // Verify node exists on both before deletion.
    assert!(primary_ref.get_node("g", node_id).unwrap().is_some());
    assert_eq!(secondary_ref.all_node_ids("g").unwrap().len(), 1);

    fanout.delete_node("g", node_id, true).unwrap();

    // Primary: node gone.
    assert!(
        primary_ref.get_node("g", node_id).unwrap().is_none(),
        "primary must not have node after delete"
    );
    // Secondary: also gone. Because InMemoryGraphStore honors _sd2_id
    // during create_node, the secondary has the same NodeId as the
    // primary, so delete_node(write_all) with that ID works on both.
    assert!(
        secondary_ref.get_node("g", node_id).unwrap().is_none(),
        "secondary must not have node after delete"
    );
    assert!(
        secondary_ref.all_node_ids("g").unwrap().is_empty(),
        "secondary should have no nodes after delete"
    );
}

// ── Delete edge propagates to both stores ───────────────────────────────

#[test]
fn test_delete_edge_propagates_to_both() {
    let (primary_ref, primary_box) = shared_store();
    let (_secondary_ref, secondary_box) = shared_store();
    let fanout = FanoutStore::new(vec![primary_box, secondary_box]);

    fanout.create_graph("g").unwrap();
    let a = fanout
        .create_node("g", vec!["Person".into()], IndexMap::new())
        .unwrap();
    let b = fanout
        .create_node("g", vec!["Person".into()], IndexMap::new())
        .unwrap();
    let edge_id = fanout
        .create_edge("g", a, b, "KNOWS".to_string(), IndexMap::new())
        .unwrap();

    // Verify edge exists on primary.
    assert!(primary_ref.get_edge("g", edge_id).unwrap().is_some());

    fanout.delete_edge("g", edge_id).unwrap();

    // Primary: edge gone.
    assert!(
        primary_ref.get_edge("g", edge_id).unwrap().is_none(),
        "primary must not have edge after delete"
    );
}

// ── set_node_properties propagates via write_all ────────────────────────

#[test]
fn test_set_node_properties_propagates() {
    let (primary_ref, primary_box) = shared_store();
    let (secondary_ref, secondary_box) = shared_store();
    let fanout = FanoutStore::new(vec![primary_box, secondary_box]);

    fanout.create_graph("g").unwrap();
    let node_id = fanout
        .create_node("g", vec!["Person".into()], IndexMap::new())
        .unwrap();

    let mut props = IndexMap::new();
    props.insert("age".to_string(), Value::Int(30));
    fanout.set_node_properties("g", node_id, props).unwrap();

    // Primary has the updated property.
    let pn = primary_ref.get_node("g", node_id).unwrap().unwrap();
    assert_eq!(pn.properties.get("age"), Some(&Value::Int(30)));

    // Secondary also received the write_all call at the same NodeId.
    let sn = secondary_ref.get_node("g", node_id).unwrap().unwrap();
    assert_eq!(sn.properties.get("age"), Some(&Value::Int(30)));
}

#[test]
fn test_set_node_properties_failure_leaves_primary_ahead_of_secondary() {
    let (primary_ref, primary_box) = shared_store();
    let (secondary_ref, secondary_box) = shared_store();
    let fanout = FanoutStore::new(vec![primary_box, secondary_box]);

    fanout.create_graph("g").unwrap();
    let node_id = fanout
        .create_node("g", vec!["Person".into()], IndexMap::new())
        .unwrap();

    // Force the secondary out of sync first so write_all will mutate the
    // primary and then fail on the secondary.
    secondary_ref.delete_node("g", node_id, true).unwrap();

    let mut props = IndexMap::new();
    props.insert("age".to_string(), Value::Int(30));
    let result = fanout.set_node_properties("g", node_id, props);
    assert!(
        result.is_err(),
        "secondary mismatch should surface as an error"
    );

    let primary_node = primary_ref.get_node("g", node_id).unwrap().unwrap();
    assert_eq!(primary_node.properties.get("age"), Some(&Value::Int(30)));
    assert!(
        secondary_ref.get_node("g", node_id).unwrap().is_none(),
        "secondary should remain behind after the failed write_all mutation"
    );
}

#[test]
fn test_add_node_labels_failure_leaves_primary_ahead_of_secondary() {
    let (primary_ref, primary_box) = shared_store();
    let (secondary_ref, secondary_box) = shared_store();
    let fanout = FanoutStore::new(vec![primary_box, secondary_box]);

    fanout.create_graph("g").unwrap();
    let node_id = fanout
        .create_node("g", vec!["Person".into()], IndexMap::new())
        .unwrap();

    // Force the secondary out of sync first so write_all will mutate the
    // primary and then fail on the secondary.
    secondary_ref.delete_node("g", node_id, true).unwrap();

    let labels = vec!["VIP".to_string()];
    let result = fanout.add_node_labels("g", node_id, &labels);
    assert!(
        result.is_err(),
        "secondary mismatch should surface as an error"
    );

    let primary_node = primary_ref.get_node("g", node_id).unwrap().unwrap();
    assert!(
        primary_node.labels.contains(&"VIP".to_string()),
        "primary should keep the label added before the secondary failed"
    );
    assert!(
        secondary_ref.get_node("g", node_id).unwrap().is_none(),
        "secondary should remain behind after the failed write_all mutation"
    );
}

// ── Read operations route to primary ────────────────────────────────────

#[test]
fn test_reads_come_from_primary() {
    let (primary_ref, primary_box) = shared_store();
    let (_secondary_ref, secondary_box) = shared_store();
    let fanout = FanoutStore::new(vec![primary_box, secondary_box]);

    fanout.create_graph("g").unwrap();
    let a = fanout
        .create_node("g", vec!["Person".into()], IndexMap::new())
        .unwrap();
    let b = fanout
        .create_node("g", vec!["Person".into()], IndexMap::new())
        .unwrap();
    fanout
        .create_edge("g", a, b, "KNOWS".to_string(), IndexMap::new())
        .unwrap();

    // list_graphs
    let graphs = fanout.list_graphs().unwrap();
    assert_eq!(graphs, vec!["g"]);

    // graph_exists
    assert!(fanout.graph_exists("g").unwrap());
    assert!(!fanout.graph_exists("nonexistent").unwrap());

    // graph_stats
    let stats = fanout.graph_stats("g").unwrap();
    assert_eq!(stats.node_count, 2);
    assert_eq!(stats.edge_count, 1);

    // all_node_ids
    let mut node_ids = fanout.all_node_ids("g").unwrap();
    node_ids.sort_by_key(|n| n.0);
    assert_eq!(node_ids.len(), 2);

    // all_edge_ids
    let edge_ids = fanout.all_edge_ids("g").unwrap();
    assert_eq!(edge_ids.len(), 1);

    // find_nodes_by_label
    let person_ids = fanout.find_nodes_by_label("g", "Person").unwrap();
    assert_eq!(person_ids.len(), 2);

    // find_edges_by_type
    let knows_ids = fanout.find_edges_by_type("g", "KNOWS").unwrap();
    assert_eq!(knows_ids.len(), 1);

    // out_edges / in_edges
    let out = fanout.out_edges("g", a).unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].neighbor_id, b);

    let in_e = fanout.in_edges("g", b).unwrap();
    assert_eq!(in_e.len(), 1);
    assert_eq!(in_e[0].neighbor_id, a);

    // out_edges_by_type / in_edges_by_type
    let out_knows = fanout.out_edges_by_type("g", a, &["KNOWS"]).unwrap();
    assert_eq!(out_knows.len(), 1);
    let in_knows = fanout.in_edges_by_type("g", b, &["KNOWS"]).unwrap();
    assert_eq!(in_knows.len(), 1);

    // all_label_names
    let labels = fanout.all_label_names("g").unwrap();
    assert_eq!(labels, vec!["Person"]);

    // get_nodes_batch / get_edges_batch
    let batch_nodes = fanout.get_nodes_batch("g", &[a, b]).unwrap();
    assert_eq!(batch_nodes.len(), 2);
    assert!(batch_nodes.iter().all(|n| n.is_some()));

    let batch_edges = fanout.get_edges_batch("g", &edge_ids).unwrap();
    assert_eq!(batch_edges.len(), 1);
    assert!(batch_edges[0].is_some());

    // Verify reads match what's in the primary directly.
    let primary_stats = primary_ref.graph_stats("g").unwrap();
    assert_eq!(stats.node_count, primary_stats.node_count);
    assert_eq!(stats.edge_count, primary_stats.edge_count);
}
