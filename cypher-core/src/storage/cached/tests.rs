//! Unit tests for the cached graph store.
//!
//! Extracted from `cached.rs` during the module decomposition.

use std::sync::Arc;

use indexmap::IndexMap;

use crate::error::StorageError;
use crate::storage::{
    ConstraintType, EdgeId, GraphStore, InMemoryGraphStore, NodeId, RangeQuery, ReadConsistency,
};
use crate::types::Value;

use super::CachedGraphStore;

fn make_backend() -> Arc<InMemoryGraphStore> {
    Arc::new(InMemoryGraphStore::new())
}

#[test]
fn write_then_read_through_cache() {
    let backend = make_backend();
    let cached = CachedGraphStore::new(backend.clone());

    // Create graph and node via cached store.
    cached.create_graph("test").unwrap();
    let mut props = IndexMap::new();
    props.insert("name".to_string(), Value::Str("Alice".into()));
    let nid = cached
        .create_node("test", vec!["Person".to_string()], props)
        .unwrap();

    // Read back through cache.
    let node = cached.get_node("test", nid).unwrap().unwrap();
    assert_eq!(node.labels, vec!["Person".to_string()]);
    assert_eq!(
        node.properties.get("name"),
        Some(&Value::Str("Alice".into()))
    );
}

#[test]
fn write_through_visible_in_backend() {
    let backend = make_backend();
    let cached = CachedGraphStore::new(backend.clone());

    cached.create_graph("g").unwrap();
    let mut props = IndexMap::new();
    props.insert("x".to_string(), Value::Int(42));
    let nid = cached
        .create_node("g", vec!["Thing".to_string()], props)
        .unwrap();

    // Verify data is in the backend directly.
    let backend_node = backend.get_node("g", nid).unwrap().unwrap();
    assert_eq!(backend_node.properties.get("x"), Some(&Value::Int(42)));
}

#[test]
fn cache_miss_falls_through_to_backend() {
    let backend = make_backend();

    // Write directly to backend (bypassing the cache).
    backend.create_graph("g").unwrap();
    let mut props = IndexMap::new();
    props.insert("val".to_string(), Value::Int(99));
    let nid = backend
        .create_node("g", vec!["Item".to_string()], props)
        .unwrap();

    // Now create CachedGraphStore — cache is cold.
    let cached = CachedGraphStore::new(backend.clone());

    // Read should load from backend into cache.
    let node = cached.get_node("g", nid).unwrap().unwrap();
    assert_eq!(node.properties.get("val"), Some(&Value::Int(99)));
    assert_eq!(node.labels, vec!["Item".to_string()]);

    // Verify it's now cached.
    assert!(cached.is_cached("g"));
}

#[test]
fn edges_cached_correctly() {
    let backend = make_backend();
    backend.create_graph("g").unwrap();
    let n1 = backend
        .create_node("g", vec!["A".into()], IndexMap::new())
        .unwrap();
    let n2 = backend
        .create_node("g", vec!["B".into()], IndexMap::new())
        .unwrap();
    let eid = backend
        .create_edge("g", n1, n2, "KNOWS".into(), IndexMap::new())
        .unwrap();

    let cached = CachedGraphStore::new(backend.clone());

    // Read edge through cache.
    let edge = cached.get_edge("g", eid).unwrap().unwrap();
    assert_eq!(edge.rel_type, "KNOWS");
    assert_eq!(edge.src, n1);
    assert_eq!(edge.dst, n2);

    // Verify adjacency.
    let out = cached.out_edges("g", n1).unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].neighbor_id, n2);

    let inc = cached.in_edges("g", n2).unwrap();
    assert_eq!(inc.len(), 1);
    assert_eq!(inc[0].neighbor_id, n1);
}

#[test]
fn write_through_keeps_cache_valid() {
    let backend = make_backend();
    let cached = CachedGraphStore::new(backend.clone());

    cached.create_graph("g").unwrap();
    // Cache is not loaded yet (create_graph doesn't auto-cache).
    assert!(!cached.is_cached("g"));

    // First read loads the cache.
    let _ = cached.all_node_ids("g").unwrap();
    assert!(cached.is_cached("g"));

    // Create node — write-through keeps cache valid.
    let nid = cached
        .create_node("g", vec!["X".into()], IndexMap::new())
        .unwrap();
    assert!(cached.is_cached("g"));

    // Node is visible through cache without reload.
    let node = cached.get_node("g", nid).unwrap();
    assert!(node.is_some());

    // Property update — cache stays valid.
    let mut props = IndexMap::new();
    props.insert("key".to_string(), Value::Str("val".into()));
    cached.set_node_properties("g", nid, props).unwrap();
    assert!(cached.is_cached("g"));

    // Property visible through cache.
    let node = cached.get_node("g", nid).unwrap().unwrap();
    assert_eq!(node.properties.get("key"), Some(&Value::Str("val".into())));
}

#[test]
fn find_nodes_by_label_cached() {
    let backend = make_backend();
    backend.create_graph("g").unwrap();
    let _ = backend
        .create_node("g", vec!["Person".into()], IndexMap::new())
        .unwrap();
    let _ = backend
        .create_node("g", vec!["Person".into()], IndexMap::new())
        .unwrap();
    let _ = backend
        .create_node("g", vec!["Car".into()], IndexMap::new())
        .unwrap();

    let cached = CachedGraphStore::new(backend.clone());
    let people = cached.find_nodes_by_label("g", "Person").unwrap();
    assert_eq!(people.len(), 2);
}

#[test]
fn graph_admin_delegates_to_backend() {
    let backend = make_backend();
    let cached = CachedGraphStore::new(backend.clone());

    cached.create_graph("a").unwrap();
    cached.create_graph("b").unwrap();

    assert!(cached.graph_exists("a").unwrap());
    assert!(cached.graph_exists("b").unwrap());

    let mut graphs = cached.list_graphs().unwrap();
    graphs.sort();
    assert_eq!(graphs, vec!["a".to_string(), "b".to_string()]);

    cached.delete_graph("a").unwrap();
    assert!(!cached.graph_exists("a").unwrap());
}

#[test]
fn cache_graph_warms_cache() {
    let backend = make_backend();
    backend.create_graph("g").unwrap();
    backend
        .create_node("g", vec!["N".into()], IndexMap::new())
        .unwrap();

    let cached = CachedGraphStore::new(backend.clone());
    assert!(!cached.is_cached("g"));

    cached.cache_graph("g").unwrap();
    assert!(cached.is_cached("g"));
}

#[test]
fn delete_node_write_through() {
    let backend = make_backend();
    let cached = CachedGraphStore::new(backend.clone());

    cached.create_graph("g").unwrap();
    let nid = cached
        .create_node("g", vec!["X".into()], IndexMap::new())
        .unwrap();

    // Node exists.
    assert!(cached.get_node("g", nid).unwrap().is_some());

    // Delete through cache.
    cached.delete_node("g", nid, false).unwrap();

    // Should be gone from backend.
    assert!(backend.get_node("g", nid).unwrap().is_none());

    // And from cache (after reload).
    assert!(cached.get_node("g", nid).unwrap().is_none());
}

fn loaded_cached() -> (
    Arc<InMemoryGraphStore>,
    CachedGraphStore,
    NodeId,
    NodeId,
    EdgeId,
) {
    let backend = make_backend();
    let cached = CachedGraphStore::new(backend.clone());
    cached.create_graph("g").unwrap();
    let mut p1 = IndexMap::new();
    p1.insert("name".to_string(), Value::Str("a".into()));
    let n1 = cached.create_node("g", vec!["L".into()], p1).unwrap();
    let mut p2 = IndexMap::new();
    p2.insert("name".to_string(), Value::Str("b".into()));
    let n2 = cached.create_node("g", vec!["L".into()], p2).unwrap();
    let e = cached
        .create_edge("g", n1, n2, "R".into(), IndexMap::new())
        .unwrap();
    // Force load
    let _ = cached.all_node_ids("g").unwrap();
    (backend, cached, n1, n2, e)
}

#[test]
fn batch_create_and_get_batch_with_cache_loaded() {
    let backend = make_backend();
    let cached = CachedGraphStore::new(backend.clone());
    cached.create_graph("g").unwrap();
    let _ = cached.all_node_ids("g").unwrap();

    let nodes = vec![
        (vec!["A".into()], IndexMap::new()),
        (vec!["A".into()], IndexMap::new()),
    ];
    let nids = cached.batch_create_nodes("g", nodes).unwrap();
    assert_eq!(nids.len(), 2);

    let edges = vec![
        (nids[0], nids[1], "X".into(), IndexMap::new()),
        (nids[1], nids[0], "X".into(), IndexMap::new()),
    ];
    let eids = cached.batch_create_edges("g", edges).unwrap();
    assert_eq!(eids.len(), 2);

    // get_*_batch (one missing)
    let mut probe = nids.clone();
    probe.push(NodeId(99_999));
    let r = cached.get_nodes_batch("g", &probe).unwrap();
    assert_eq!(r.len(), 3);
    assert!(r[0].is_some() && r[1].is_some() && r[2].is_none());

    let mut eprobe = eids.clone();
    eprobe.push(EdgeId(99_999));
    let r = cached.get_edges_batch("g", &eprobe).unwrap();
    assert_eq!(r.len(), 3);
    assert!(r[2].is_none());
}

#[test]
fn property_and_label_mutations_with_cache() {
    let (_, cached, n1, _, e) = loaded_cached();
    // set_node_properties (merge)
    let mut add = IndexMap::new();
    add.insert("k".to_string(), Value::Int(1));
    cached.set_node_properties("g", n1, add).unwrap();
    let n = cached.get_node("g", n1).unwrap().unwrap();
    assert_eq!(n.properties.get("k"), Some(&Value::Int(1)));
    assert!(n.properties.get("name").is_some());

    // replace_node_properties
    let mut repl = IndexMap::new();
    repl.insert("only".to_string(), Value::Int(2));
    cached.replace_node_properties("g", n1, repl).unwrap();
    let n = cached.get_node("g", n1).unwrap().unwrap();
    assert!(n.properties.get("name").is_none());
    assert_eq!(n.properties.get("only"), Some(&Value::Int(2)));

    // remove_node_property
    cached.remove_node_property("g", n1, "only").unwrap();
    let n = cached.get_node("g", n1).unwrap().unwrap();
    assert!(n.properties.is_empty());

    // add_node_labels / remove_node_labels
    cached
        .add_node_labels("g", n1, &["L".to_string(), "M".to_string()])
        .unwrap();
    let n = cached.get_node("g", n1).unwrap().unwrap();
    assert!(n.labels.contains(&"M".to_string()));
    cached
        .remove_node_labels("g", n1, &["M".to_string()])
        .unwrap();
    let n = cached.get_node("g", n1).unwrap().unwrap();
    assert!(!n.labels.contains(&"M".to_string()));

    // edge property mutations
    let mut add = IndexMap::new();
    add.insert("w".to_string(), Value::Int(5));
    cached.set_edge_properties("g", e, add).unwrap();
    assert_eq!(
        cached
            .get_edge("g", e)
            .unwrap()
            .unwrap()
            .properties
            .get("w"),
        Some(&Value::Int(5))
    );

    let mut repl = IndexMap::new();
    repl.insert("w2".to_string(), Value::Int(7));
    cached.replace_edge_properties("g", e, repl).unwrap();
    let ed = cached.get_edge("g", e).unwrap().unwrap();
    assert!(ed.properties.get("w").is_none());
    assert_eq!(ed.properties.get("w2"), Some(&Value::Int(7)));

    cached.remove_edge_property("g", e, "w2").unwrap();
    assert!(cached
        .get_edge("g", e)
        .unwrap()
        .unwrap()
        .properties
        .is_empty());
}

#[test]
fn adjacency_lookups_through_cache() {
    let (_, cached, n1, n2, e) = loaded_cached();
    let r = cached.out_edges_by_type("g", n1, &["R"]).unwrap();
    assert_eq!(r.len(), 1);
    let r = cached.in_edges_by_type("g", n2, &["R"]).unwrap();
    assert_eq!(r.len(), 1);
    let r = cached
        .out_edges_by_type_batch("g", &[n1, n2], &["R"])
        .unwrap();
    assert_eq!(r.len(), 2);
    let r = cached
        .in_edges_by_type_batch("g", &[n1, n2], &["R"])
        .unwrap();
    assert_eq!(r.len(), 2);
    let r = cached.out_edge_ids("g", n1).unwrap();
    assert_eq!(r, vec![e]);
    let r = cached.in_edge_ids("g", n2).unwrap();
    assert_eq!(r, vec![e]);
    let nids = cached.all_node_ids("g").unwrap();
    assert!(nids.contains(&n1) && nids.contains(&n2));
    let eids = cached.all_edge_ids("g").unwrap();
    assert!(eids.contains(&e));
    let labels = cached.all_label_names("g").unwrap();
    assert!(labels.contains(&"L".to_string()));
    let stats = cached.graph_stats("g").unwrap();
    assert!(stats.node_count >= 2);
    let r = cached.find_edges_by_type("g", "R").unwrap();
    assert_eq!(r, vec![e]);
}

#[test]
fn find_nodes_by_label_and_property_with_index() {
    let (_, cached, n1, _, _) = loaded_cached();
    cached.create_index("g", "L", "name").unwrap();
    let r = cached
        .find_nodes_by_label_and_property("g", "L", "name", RangeQuery::Eq(&Value::Str("a".into())))
        .unwrap();
    assert_eq!(r, vec![n1]);
    let idx = cached.list_indexes("g").unwrap();
    assert!(idx.iter().any(|d| d.label == "L" && d.property == "name"));
    cached.drop_index("g", "L", "name").unwrap();
    let idx = cached.list_indexes("g").unwrap();
    assert!(!idx.iter().any(|d| d.label == "L" && d.property == "name"));
}

#[test]
fn constraints_create_and_drop() {
    let (_, cached, _, _, _) = loaded_cached();
    // Either supported (Ok) or unsupported (Err) — both paths exercise the cache delegation.
    let _ = cached.create_constraint("g", "L", "name", ConstraintType::Unique);
    let _ = cached.list_constraints("g").unwrap();
    let _ = cached.drop_constraint("g", "L", "name");
}

#[test]
fn delete_edge_and_detach_node() {
    let (_, cached, n1, n2, e) = loaded_cached();
    cached.delete_edge("g", e).unwrap();
    assert!(cached.get_edge("g", e).unwrap().is_none());
    assert!(cached.out_edges("g", n1).unwrap().is_empty());
    assert!(cached.in_edges("g", n2).unwrap().is_empty());

    // Re-create edge then delete the node with detach.
    let e2 = cached
        .create_edge("g", n1, n2, "R".into(), IndexMap::new())
        .unwrap();
    cached.delete_node("g", n1, true).unwrap();
    assert!(cached.get_node("g", n1).unwrap().is_none());
    assert!(cached.get_edge("g", e2).unwrap().is_none());
}

#[test]
fn invalidate_via_delete_graph() {
    let backend = make_backend();
    let cached = CachedGraphStore::new(backend.clone());
    cached.create_graph("g").unwrap();
    let _ = cached.all_node_ids("g").unwrap();
    assert!(cached.is_cached("g"));
    cached.delete_graph("g").unwrap();
    assert!(!cached.is_cached("g"));
}

#[test]
fn cache_graph_missing_returns_error() {
    let cached = CachedGraphStore::new(make_backend());
    let err = cached.cache_graph("missing").unwrap_err();
    assert!(matches!(err, StorageError::GraphNotFound { .. }));
}

#[test]
fn debug_impl_lists_loaded_graphs() {
    let cached = CachedGraphStore::new(make_backend());
    cached.create_graph("g").unwrap();
    let _ = cached.all_node_ids("g").unwrap();
    let dbg = format!("{:?}", cached);
    assert!(dbg.contains("CachedGraphStore"));
}

#[test]
fn tx_and_consistency_default_to_noop() {
    let cached = CachedGraphStore::new(make_backend());
    assert!(cached
        .with_read_consistency(ReadConsistency::SessionFreshPrimary)
        .is_none());
    let snap = cached.tx_snapshot(&["g"]).unwrap();
    assert!(snap.is_none());
    cached.tx_commit(&InMemoryGraphStore::new()).unwrap();
    // as_any round-trip
    let any = cached.as_any();
    assert!(any.downcast_ref::<CachedGraphStore>().is_some());
}

#[test]
fn writes_on_uncached_graph_dont_load() {
    // When graph isn't cached, write-through skips cache mutations.
    let backend = make_backend();
    backend.create_graph("g").unwrap();
    let nid = backend
        .create_node("g", vec!["X".into()], IndexMap::new())
        .unwrap();
    let cached = CachedGraphStore::new(backend.clone());
    // Not cached yet
    assert!(!cached.is_cached("g"));
    let mut p = IndexMap::new();
    p.insert("k".to_string(), Value::Int(1));
    cached.set_node_properties("g", nid, p).unwrap();
    cached
        .add_node_labels("g", nid, &["Y".to_string()])
        .unwrap();
    cached
        .remove_node_labels("g", nid, &["Y".to_string()])
        .unwrap();
    cached.remove_node_property("g", nid, "k").unwrap();
    // Backend reflects all writes
    let n = backend.get_node("g", nid).unwrap().unwrap();
    assert!(!n.labels.contains(&"Y".to_string()));
}
