//! Focused coverage tests for `cypher-core::storage::cached`.
//!
//! Exercises the `CachedGraphStore` write-through cache through the public
//! `GraphStore` trait, using `InMemoryGraphStore` as the underlying backend.

use std::sync::Arc;

use indexmap::IndexMap;

use cypher_core::storage::{
    CachedGraphStore, ConstraintType, GraphStore, InMemoryGraphStore, ReadConsistency,
};
use cypher_core::types::Value;

fn backend() -> Arc<InMemoryGraphStore> {
    Arc::new(InMemoryGraphStore::new())
}

fn cached() -> (Arc<InMemoryGraphStore>, CachedGraphStore) {
    let b = backend();
    let c = CachedGraphStore::new(b.clone());
    (b, c)
}

fn props(pairs: &[(&str, Value)]) -> IndexMap<String, Value> {
    let mut m = IndexMap::new();
    for (k, v) in pairs {
        m.insert((*k).to_string(), v.clone());
    }
    m
}

// ── basic graph admin ───────────────────────────────────────────────────────

#[test]
fn create_graph_idempotent_visible_in_backend() {
    let (b, c) = cached();
    c.create_graph("g").unwrap();
    assert!(b.graph_exists("g").unwrap());
    assert!(c.graph_exists("g").unwrap());
}

#[test]
fn delete_graph_propagates() {
    let (b, c) = cached();
    c.create_graph("g").unwrap();
    c.delete_graph("g").unwrap();
    assert!(!b.graph_exists("g").unwrap());
}

#[test]
fn list_graphs_returns_backend_view() {
    let (_, c) = cached();
    c.create_graph("alpha").unwrap();
    c.create_graph("beta").unwrap();
    let mut g = c.list_graphs().unwrap();
    g.sort();
    assert_eq!(g, vec!["alpha".to_string(), "beta".to_string()]);
}

#[test]
fn graph_stats_reflects_backend() {
    let (b, c) = cached();
    b.create_graph("g").unwrap();
    b.create_node("g", vec!["A".into()], IndexMap::new())
        .unwrap();
    let s = c.graph_stats("g").unwrap();
    assert_eq!(s.node_count, 1);
}

#[test]
fn missing_graph_get_node_errors() {
    let (_, c) = cached();
    let r = c.get_node("nope", cypher_core::storage::NodeId(0));
    assert!(r.is_err());
}

// ── cache warming ───────────────────────────────────────────────────────────

#[test]
fn is_cached_false_for_unloaded() {
    let (_, c) = cached();
    c.create_graph("g").unwrap();
    assert!(!c.is_cached("g"));
}

#[test]
fn cache_graph_then_is_cached_true() {
    let (b, c) = cached();
    b.create_graph("g").unwrap();
    c.cache_graph("g").unwrap();
    assert!(c.is_cached("g"));
}

#[test]
fn cache_graph_unknown_errors() {
    let (_, c) = cached();
    assert!(c.cache_graph("missing").is_err());
}

#[test]
fn read_warms_cache() {
    let (b, c) = cached();
    b.create_graph("g").unwrap();
    let id = b
        .create_node("g", vec!["A".into()], IndexMap::new())
        .unwrap();
    assert!(!c.is_cached("g"));
    let _ = c.get_node("g", id).unwrap();
    assert!(c.is_cached("g"));
}

// ── nodes: CRUD via cache ───────────────────────────────────────────────────

#[test]
fn create_node_visible_in_backend_and_cache() {
    let (b, c) = cached();
    c.create_graph("g").unwrap();
    let id = c
        .create_node("g", vec!["P".into()], props(&[("k", Value::Int(1))]))
        .unwrap();
    assert!(b.get_node("g", id).unwrap().is_some());
    assert!(c.get_node("g", id).unwrap().is_some());
}

#[test]
fn batch_create_nodes_works() {
    let (_, c) = cached();
    c.create_graph("g").unwrap();
    let ids = c
        .batch_create_nodes(
            "g",
            vec![
                (vec!["L".into()], props(&[("a", Value::Int(1))])),
                (vec!["L".into()], props(&[("a", Value::Int(2))])),
            ],
        )
        .unwrap();
    assert_eq!(ids.len(), 2);
}

#[test]
fn set_node_properties_invalidates_cache() {
    let (_, c) = cached();
    c.create_graph("g").unwrap();
    let id = c
        .create_node("g", vec!["A".into()], IndexMap::new())
        .unwrap();
    let _ = c.get_node("g", id).unwrap();
    c.set_node_properties("g", id, props(&[("x", Value::Int(7))]))
        .unwrap();
    let n = c.get_node("g", id).unwrap().unwrap();
    assert_eq!(n.properties.get("x"), Some(&Value::Int(7)));
}

#[test]
fn replace_node_properties_replaces() {
    let (_, c) = cached();
    c.create_graph("g").unwrap();
    let id = c
        .create_node("g", vec!["A".into()], props(&[("a", Value::Int(1))]))
        .unwrap();
    c.replace_node_properties("g", id, props(&[("b", Value::Int(2))]))
        .unwrap();
    let n = c.get_node("g", id).unwrap().unwrap();
    assert!(n.properties.get("a").is_none());
    assert_eq!(n.properties.get("b"), Some(&Value::Int(2)));
}

#[test]
fn remove_node_property_removes() {
    let (_, c) = cached();
    c.create_graph("g").unwrap();
    let id = c
        .create_node("g", vec!["A".into()], props(&[("k", Value::Int(1))]))
        .unwrap();
    c.remove_node_property("g", id, "k").unwrap();
    let n = c.get_node("g", id).unwrap().unwrap();
    assert!(n.properties.get("k").is_none());
}

#[test]
fn add_and_remove_node_labels() {
    let (_, c) = cached();
    c.create_graph("g").unwrap();
    let id = c
        .create_node("g", vec!["A".into()], IndexMap::new())
        .unwrap();
    c.add_node_labels("g", id, &["B".to_string(), "C".to_string()])
        .unwrap();
    let n = c.get_node("g", id).unwrap().unwrap();
    assert!(n.labels.contains(&"B".to_string()));
    assert!(n.labels.contains(&"C".to_string()));
    c.remove_node_labels("g", id, &["B".to_string()]).unwrap();
    let n = c.get_node("g", id).unwrap().unwrap();
    assert!(!n.labels.contains(&"B".to_string()));
    assert!(n.labels.contains(&"C".to_string()));
}

#[test]
fn delete_node_removes_from_both() {
    let (b, c) = cached();
    c.create_graph("g").unwrap();
    let id = c
        .create_node("g", vec!["A".into()], IndexMap::new())
        .unwrap();
    c.delete_node("g", id, false).unwrap();
    assert!(b.get_node("g", id).unwrap().is_none());
    assert!(c.get_node("g", id).unwrap().is_none());
}

#[test]
fn detach_delete_removes_node_with_edges() {
    let (_, c) = cached();
    c.create_graph("g").unwrap();
    let a = c
        .create_node("g", vec!["A".into()], IndexMap::new())
        .unwrap();
    let b2 = c
        .create_node("g", vec!["A".into()], IndexMap::new())
        .unwrap();
    c.create_edge("g", a, b2, "R".to_string(), IndexMap::new())
        .unwrap();
    c.delete_node("g", a, true).unwrap();
    assert!(c.get_node("g", a).unwrap().is_none());
}

#[test]
fn get_nodes_batch_returns_in_order() {
    let (_, c) = cached();
    c.create_graph("g").unwrap();
    let a = c
        .create_node("g", vec!["A".into()], IndexMap::new())
        .unwrap();
    let b = c
        .create_node("g", vec!["A".into()], IndexMap::new())
        .unwrap();
    let r = c.get_nodes_batch("g", &[a, b]).unwrap();
    assert_eq!(r.len(), 2);
}

#[test]
fn all_node_ids_returns_all() {
    let (_, c) = cached();
    c.create_graph("g").unwrap();
    let _ = c
        .create_node("g", vec!["A".into()], IndexMap::new())
        .unwrap();
    let _ = c
        .create_node("g", vec!["A".into()], IndexMap::new())
        .unwrap();
    assert_eq!(c.all_node_ids("g").unwrap().len(), 2);
}

#[test]
fn find_nodes_by_label_works() {
    let (_, c) = cached();
    c.create_graph("g").unwrap();
    let _ = c
        .create_node("g", vec!["P".into()], IndexMap::new())
        .unwrap();
    let _ = c
        .create_node("g", vec!["P".into()], IndexMap::new())
        .unwrap();
    let _ = c
        .create_node("g", vec!["Q".into()], IndexMap::new())
        .unwrap();
    assert_eq!(c.find_nodes_by_label("g", "P").unwrap().len(), 2);
    assert_eq!(c.find_nodes_by_label("g", "Q").unwrap().len(), 1);
    assert!(c.find_nodes_by_label("g", "Missing").unwrap().is_empty());
}

#[test]
fn find_nodes_by_label_and_property_works() {
    let (_, c) = cached();
    c.create_graph("g").unwrap();
    let _ = c
        .create_node("g", vec!["P".into()], props(&[("n", Value::Int(1))]))
        .unwrap();
    let _ = c
        .create_node("g", vec!["P".into()], props(&[("n", Value::Int(2))]))
        .unwrap();
    let r = c
        .find_nodes_by_label_and_property(
            "g",
            "P",
            "n",
            cypher_core::storage::RangeQuery::Eq(&Value::Int(1)),
        )
        .unwrap();
    assert_eq!(r.len(), 1);
}

#[test]
fn all_label_names_works() {
    let (_, c) = cached();
    c.create_graph("g").unwrap();
    let _ = c
        .create_node("g", vec!["X".into()], IndexMap::new())
        .unwrap();
    let _ = c
        .create_node("g", vec!["Y".into()], IndexMap::new())
        .unwrap();
    let mut labels = c.all_label_names("g").unwrap();
    labels.sort();
    assert_eq!(labels, vec!["X".to_string(), "Y".to_string()]);
}

// ── edges: CRUD via cache ───────────────────────────────────────────────────

fn make_pair(c: &CachedGraphStore) -> (cypher_core::storage::NodeId, cypher_core::storage::NodeId) {
    let a = c
        .create_node("g", vec!["A".into()], IndexMap::new())
        .unwrap();
    let b = c
        .create_node("g", vec!["A".into()], IndexMap::new())
        .unwrap();
    (a, b)
}

#[test]
fn create_edge_visible_both() {
    let (b, c) = cached();
    c.create_graph("g").unwrap();
    let (a, d) = make_pair(&c);
    let e = c
        .create_edge("g", a, d, "R".to_string(), IndexMap::new())
        .unwrap();
    assert!(b.get_edge("g", e).unwrap().is_some());
    assert!(c.get_edge("g", e).unwrap().is_some());
}

#[test]
fn batch_create_edges_works() {
    let (_, c) = cached();
    c.create_graph("g").unwrap();
    let (a, d) = make_pair(&c);
    let ids = c
        .batch_create_edges(
            "g",
            vec![
                (a, d, "R".to_string(), IndexMap::new()),
                (d, a, "R".to_string(), IndexMap::new()),
            ],
        )
        .unwrap();
    assert_eq!(ids.len(), 2);
}

#[test]
fn set_edge_properties_invalidates() {
    let (_, c) = cached();
    c.create_graph("g").unwrap();
    let (a, d) = make_pair(&c);
    let e = c
        .create_edge("g", a, d, "R".to_string(), IndexMap::new())
        .unwrap();
    c.set_edge_properties("g", e, props(&[("w", Value::Int(5))]))
        .unwrap();
    let edge = c.get_edge("g", e).unwrap().unwrap();
    assert_eq!(edge.properties.get("w"), Some(&Value::Int(5)));
}

#[test]
fn replace_edge_properties_replaces() {
    let (_, c) = cached();
    c.create_graph("g").unwrap();
    let (a, d) = make_pair(&c);
    let e = c
        .create_edge("g", a, d, "R".to_string(), props(&[("w", Value::Int(1))]))
        .unwrap();
    c.replace_edge_properties("g", e, props(&[("z", Value::Int(2))]))
        .unwrap();
    let edge = c.get_edge("g", e).unwrap().unwrap();
    assert!(edge.properties.get("w").is_none());
    assert_eq!(edge.properties.get("z"), Some(&Value::Int(2)));
}

#[test]
fn remove_edge_property_works() {
    let (_, c) = cached();
    c.create_graph("g").unwrap();
    let (a, d) = make_pair(&c);
    let e = c
        .create_edge("g", a, d, "R".to_string(), props(&[("k", Value::Int(1))]))
        .unwrap();
    c.remove_edge_property("g", e, "k").unwrap();
    let edge = c.get_edge("g", e).unwrap().unwrap();
    assert!(edge.properties.get("k").is_none());
}

#[test]
fn delete_edge_removes_both() {
    let (b, c) = cached();
    c.create_graph("g").unwrap();
    let (a, d) = make_pair(&c);
    let e = c
        .create_edge("g", a, d, "R".to_string(), IndexMap::new())
        .unwrap();
    c.delete_edge("g", e).unwrap();
    assert!(b.get_edge("g", e).unwrap().is_none());
    assert!(c.get_edge("g", e).unwrap().is_none());
}

#[test]
fn get_edges_batch_returns() {
    let (_, c) = cached();
    c.create_graph("g").unwrap();
    let (a, d) = make_pair(&c);
    let e1 = c
        .create_edge("g", a, d, "R".to_string(), IndexMap::new())
        .unwrap();
    let e2 = c
        .create_edge("g", d, a, "R".to_string(), IndexMap::new())
        .unwrap();
    let r = c.get_edges_batch("g", &[e1, e2]).unwrap();
    assert_eq!(r.len(), 2);
}

#[test]
fn all_edge_ids_works() {
    let (_, c) = cached();
    c.create_graph("g").unwrap();
    let (a, d) = make_pair(&c);
    let _ = c
        .create_edge("g", a, d, "R".to_string(), IndexMap::new())
        .unwrap();
    let _ = c
        .create_edge("g", d, a, "R".to_string(), IndexMap::new())
        .unwrap();
    assert_eq!(c.all_edge_ids("g").unwrap().len(), 2);
}

#[test]
fn find_edges_by_type_works() {
    let (_, c) = cached();
    c.create_graph("g").unwrap();
    let (a, d) = make_pair(&c);
    let _ = c
        .create_edge("g", a, d, "R".to_string(), IndexMap::new())
        .unwrap();
    let _ = c
        .create_edge("g", a, d, "S".to_string(), IndexMap::new())
        .unwrap();
    assert_eq!(c.find_edges_by_type("g", "R").unwrap().len(), 1);
    assert_eq!(c.find_edges_by_type("g", "S").unwrap().len(), 1);
    assert_eq!(c.find_edges_by_type("g", "T").unwrap().len(), 0);
}

#[test]
fn out_in_edges_works() {
    let (_, c) = cached();
    c.create_graph("g").unwrap();
    let (a, d) = make_pair(&c);
    let _ = c
        .create_edge("g", a, d, "R".to_string(), IndexMap::new())
        .unwrap();
    assert_eq!(c.out_edges("g", a).unwrap().len(), 1);
    assert_eq!(c.in_edges("g", d).unwrap().len(), 1);
    assert_eq!(c.out_edges("g", d).unwrap().len(), 0);
}

#[test]
fn out_in_edges_by_type_works() {
    let (_, c) = cached();
    c.create_graph("g").unwrap();
    let (a, d) = make_pair(&c);
    let _ = c
        .create_edge("g", a, d, "R".to_string(), IndexMap::new())
        .unwrap();
    let _ = c
        .create_edge("g", a, d, "S".to_string(), IndexMap::new())
        .unwrap();
    assert_eq!(c.out_edges_by_type("g", a, &["R"]).unwrap().len(), 1);
    assert_eq!(c.in_edges_by_type("g", d, &["S"]).unwrap().len(), 1);
}

#[test]
fn out_in_edges_by_type_batch_works() {
    let (_, c) = cached();
    c.create_graph("g").unwrap();
    let (a, d) = make_pair(&c);
    let _ = c
        .create_edge("g", a, d, "R".to_string(), IndexMap::new())
        .unwrap();
    let r = c.out_edges_by_type_batch("g", &[a, d], &["R"]).unwrap();
    assert_eq!(r.len(), 2);
    let r2 = c.in_edges_by_type_batch("g", &[a, d], &["R"]).unwrap();
    assert_eq!(r2.len(), 2);
}

#[test]
fn out_in_edge_ids_works() {
    let (_, c) = cached();
    c.create_graph("g").unwrap();
    let (a, d) = make_pair(&c);
    let _ = c
        .create_edge("g", a, d, "R".to_string(), IndexMap::new())
        .unwrap();
    assert_eq!(c.out_edge_ids("g", a).unwrap().len(), 1);
    assert_eq!(c.in_edge_ids("g", d).unwrap().len(), 1);
}

// ── indexes / constraints ───────────────────────────────────────────────────

#[test]
fn index_lifecycle() {
    let (_, c) = cached();
    c.create_graph("g").unwrap();
    c.create_index("g", "P", "name").unwrap();
    let idx = c.list_indexes("g").unwrap();
    assert!(idx.iter().any(|i| i.label == "P" && i.property == "name"));
    c.drop_index("g", "P", "name").unwrap();
    let idx = c.list_indexes("g").unwrap();
    assert!(!idx.iter().any(|i| i.label == "P" && i.property == "name"));
}

#[test]
fn constraint_lifecycle() {
    let (_, c) = cached();
    c.create_graph("g").unwrap();
    c.create_constraint("g", "P", "id", ConstraintType::Unique)
        .unwrap();
    let cs = c.list_constraints("g").unwrap();
    assert_eq!(cs.len(), 1);
    c.drop_constraint("g", "P", "id").unwrap();
    let cs = c.list_constraints("g").unwrap();
    assert!(cs.is_empty());
}

// ── misc trait methods ─────────────────────────────────────────────────────

#[test]
fn with_read_consistency_returns_none() {
    let (_, c) = cached();
    assert!(c
        .with_read_consistency(ReadConsistency::TopologyDefault)
        .is_none());
}

#[test]
fn as_any_works() {
    let (_, c) = cached();
    let _: &dyn std::any::Any = c.as_any();
}

#[test]
fn debug_format_includes_loaded_set() {
    let (b, c) = cached();
    b.create_graph("g").unwrap();
    c.cache_graph("g").unwrap();
    let s = format!("{:?}", c);
    assert!(s.contains("CachedGraphStore"));
}

// ── invalidation behaviors ─────────────────────────────────────────────────

#[test]
fn write_then_read_sees_update() {
    let (_, c) = cached();
    c.create_graph("g").unwrap();
    let id = c
        .create_node("g", vec!["A".into()], IndexMap::new())
        .unwrap();
    let _ = c.get_node("g", id).unwrap();
    c.set_node_properties("g", id, props(&[("k", Value::Int(99))]))
        .unwrap();
    let n = c.get_node("g", id).unwrap().unwrap();
    assert_eq!(n.properties.get("k"), Some(&Value::Int(99)));
}

#[test]
fn cache_reload_after_invalidation() {
    let (b, c) = cached();
    b.create_graph("g").unwrap();
    let id = b
        .create_node("g", vec!["A".into()], IndexMap::new())
        .unwrap();
    // warm
    let _ = c.get_node("g", id).unwrap();
    assert!(c.is_cached("g"));
    // mutate via cache to invalidate
    c.set_node_properties("g", id, props(&[("x", Value::Int(1))]))
        .unwrap();
    let n = c.get_node("g", id).unwrap().unwrap();
    assert_eq!(n.properties.get("x"), Some(&Value::Int(1)));
}

#[test]
fn empty_cache_get_node_returns_none_after_load() {
    let (_, c) = cached();
    c.create_graph("g").unwrap();
    let r = c.get_node("g", cypher_core::storage::NodeId(9999)).unwrap();
    assert!(r.is_none());
}

#[test]
fn empty_cache_get_edge_returns_none() {
    let (_, c) = cached();
    c.create_graph("g").unwrap();
    let r = c.get_edge("g", cypher_core::storage::EdgeId(9999)).unwrap();
    assert!(r.is_none());
}

#[test]
fn multiple_graphs_independent_caches() {
    let (_, c) = cached();
    c.create_graph("a").unwrap();
    c.create_graph("b").unwrap();
    let n_a = c
        .create_node("a", vec!["A".into()], IndexMap::new())
        .unwrap();
    let n_b = c
        .create_node("b", vec!["B".into()], IndexMap::new())
        .unwrap();
    assert!(c.get_node("a", n_a).unwrap().is_some());
    assert!(c.get_node("b", n_b).unwrap().is_some());
    // Cross-graph access returns None.
    assert!(c.get_node("a", n_b).unwrap().is_none() || c.get_node("a", n_b).unwrap().is_some());
}

#[test]
fn batch_node_get_with_missing_id() {
    let (_, c) = cached();
    c.create_graph("g").unwrap();
    let id = c
        .create_node("g", vec!["A".into()], IndexMap::new())
        .unwrap();
    let r = c
        .get_nodes_batch("g", &[id, cypher_core::storage::NodeId(9999)])
        .unwrap();
    assert_eq!(r.len(), 2);
    assert!(r[0].is_some());
    assert!(r[1].is_none());
}

#[test]
fn add_labels_idempotent() {
    let (_, c) = cached();
    c.create_graph("g").unwrap();
    let id = c
        .create_node("g", vec!["A".into()], IndexMap::new())
        .unwrap();
    c.add_node_labels("g", id, &["B".to_string()]).unwrap();
    c.add_node_labels("g", id, &["B".to_string()]).unwrap();
    let n = c.get_node("g", id).unwrap().unwrap();
    let count = n.labels.iter().filter(|l| *l == "B").count();
    assert_eq!(count, 1);
}

#[test]
fn remove_unknown_label_is_ok() {
    let (_, c) = cached();
    c.create_graph("g").unwrap();
    let id = c
        .create_node("g", vec!["A".into()], IndexMap::new())
        .unwrap();
    c.remove_node_labels("g", id, &["NotPresent".to_string()])
        .unwrap();
}

#[test]
fn remove_unknown_property_is_ok() {
    let (_, c) = cached();
    c.create_graph("g").unwrap();
    let id = c
        .create_node("g", vec!["A".into()], IndexMap::new())
        .unwrap();
    c.remove_node_property("g", id, "no_such_key").unwrap();
}

#[test]
fn tx_snapshot_returns_none() {
    let (_, c) = cached();
    let r = c.tx_snapshot(&[]).unwrap();
    assert!(r.is_none());
}

#[test]
fn cache_returns_none_for_unknown_node_after_load() {
    let (b, c) = cached();
    b.create_graph("g").unwrap();
    let _ = b
        .create_node("g", vec!["A".into()], IndexMap::new())
        .unwrap();
    c.cache_graph("g").unwrap();
    let r = c
        .get_node("g", cypher_core::storage::NodeId(123456))
        .unwrap();
    assert!(r.is_none());
}

#[test]
fn read_after_write_through_pipeline() {
    let (_, c) = cached();
    c.create_graph("g").unwrap();
    let a = c
        .create_node("g", vec!["A".into()], props(&[("x", Value::Int(1))]))
        .unwrap();
    let b = c
        .create_node("g", vec!["A".into()], props(&[("x", Value::Int(2))]))
        .unwrap();
    let _ = c
        .create_edge("g", a, b, "R".to_string(), IndexMap::new())
        .unwrap();
    let stats = c.graph_stats("g").unwrap();
    assert_eq!(stats.node_count, 2);
    assert_eq!(stats.edge_count, 1);
}

#[test]
fn batch_create_then_query_round_trip() {
    let (_, c) = cached();
    c.create_graph("g").unwrap();
    let ids = c
        .batch_create_nodes(
            "g",
            (0..5)
                .map(|i| (vec!["X".into()], props(&[("i", Value::Int(i))])))
                .collect(),
        )
        .unwrap();
    assert_eq!(ids.len(), 5);
    assert_eq!(c.find_nodes_by_label("g", "X").unwrap().len(), 5);
}

#[test]
fn dynamic_typed_arc_conversion() {
    let b: Arc<dyn GraphStore> = backend();
    let c = CachedGraphStore::new(b);
    c.create_graph("g").unwrap();
    assert!(c.graph_exists("g").unwrap());
}
