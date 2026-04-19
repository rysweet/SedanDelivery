//! Tests for [`BfsTraversal`].

use indexmap::IndexMap;

use super::BfsTraversal;
use crate::ast::Direction;
use crate::executor::budget::ExecutionBudget;
use crate::storage::{GraphStore, InMemoryGraphStore, NodeId};

fn build_chain(n: usize) -> (InMemoryGraphStore, Vec<NodeId>) {
    let store = InMemoryGraphStore::new();
    store.create_graph("g").unwrap();
    let mut ids = Vec::new();
    for _ in 0..n {
        ids.push(store.create_node("g", vec![], IndexMap::new()).unwrap());
    }
    for i in 0..n - 1 {
        store
            .create_edge("g", ids[i], ids[i + 1], "NEXT".to_string(), IndexMap::new())
            .unwrap();
    }
    (store, ids)
}

// ── Basic traversal ────────────────────────────────────────────────────────

#[test]
fn bfs_linear_chain_outgoing() {
    let (store, ids) = build_chain(4);
    let mut budget = ExecutionBudget::unlimited();
    let trav = BfsTraversal::new(
        &store,
        "g",
        ids[0],
        Direction::Outgoing,
        &[],
        10,
        &mut budget,
    );
    let results: Vec<_> = trav.map(|r| r.unwrap()).collect();
    // Should visit all 4 nodes in order: depths 0, 1, 2, 3
    assert_eq!(results.len(), 4);
    assert_eq!(results[0], (ids[0], 0));
    assert_eq!(results[1], (ids[1], 1));
    assert_eq!(results[2], (ids[2], 2));
    assert_eq!(results[3], (ids[3], 3));
}

#[test]
fn bfs_max_depth_limits_traversal() {
    let (store, ids) = build_chain(5);
    let mut budget = ExecutionBudget::unlimited();
    let trav = BfsTraversal::new(
        &store,
        "g",
        ids[0],
        Direction::Outgoing,
        &[],
        2,
        &mut budget,
    );
    let results: Vec<_> = trav.map(|r| r.unwrap()).collect();
    // With max_depth=2: nodes at depths 0, 1, 2 = first 3 nodes
    assert_eq!(results.len(), 3);
    assert_eq!(results[2].1, 2); // depth 2
}

// ── Cycle avoidance ────────────────────────────────────────────────────────

#[test]
fn bfs_cycle_avoidance() {
    let store = InMemoryGraphStore::new();
    store.create_graph("g").unwrap();
    let a = store.create_node("g", vec![], IndexMap::new()).unwrap();
    let b = store.create_node("g", vec![], IndexMap::new()).unwrap();
    let c = store.create_node("g", vec![], IndexMap::new()).unwrap();
    // a -> b -> c -> a (cycle)
    store
        .create_edge("g", a, b, "R".to_string(), IndexMap::new())
        .unwrap();
    store
        .create_edge("g", b, c, "R".to_string(), IndexMap::new())
        .unwrap();
    store
        .create_edge("g", c, a, "R".to_string(), IndexMap::new())
        .unwrap();

    let mut budget = ExecutionBudget::unlimited();
    let trav = BfsTraversal::new(&store, "g", a, Direction::Outgoing, &[], 10, &mut budget);
    let results: Vec<_> = trav.map(|r| r.unwrap()).collect();
    // Each node visited once
    assert_eq!(results.len(), 3);
}

// ── Direction filtering ────────────────────────────────────────────────────

#[test]
fn bfs_incoming_direction() {
    let (store, ids) = build_chain(3);
    // ids[0] -> ids[1] -> ids[2]
    // BFS incoming from ids[2]: should reach ids[1] at depth 1, ids[0] at depth 2
    let mut budget = ExecutionBudget::unlimited();
    let trav = BfsTraversal::new(
        &store,
        "g",
        ids[2],
        Direction::Incoming,
        &[],
        10,
        &mut budget,
    );
    let results: Vec<_> = trav.map(|r| r.unwrap()).collect();
    assert_eq!(results.len(), 3);
    assert_eq!(results[0], (ids[2], 0));
    assert_eq!(results[1], (ids[1], 1));
    assert_eq!(results[2], (ids[0], 2));
}

#[test]
fn bfs_undirected() {
    let store = InMemoryGraphStore::new();
    store.create_graph("g").unwrap();
    let a = store.create_node("g", vec![], IndexMap::new()).unwrap();
    let b = store.create_node("g", vec![], IndexMap::new()).unwrap();
    let c = store.create_node("g", vec![], IndexMap::new()).unwrap();
    // a -> b, c -> b (b has incoming from both a and c)
    store
        .create_edge("g", a, b, "R".to_string(), IndexMap::new())
        .unwrap();
    store
        .create_edge("g", c, b, "R".to_string(), IndexMap::new())
        .unwrap();

    let mut budget = ExecutionBudget::unlimited();
    // Start from b, undirected: should reach both a and c
    let trav = BfsTraversal::new(&store, "g", b, Direction::Undirected, &[], 10, &mut budget);
    let mut node_ids: Vec<NodeId> = trav.map(|r| r.unwrap().0).collect();
    node_ids.sort();
    assert!(node_ids.contains(&a));
    assert!(node_ids.contains(&b));
    assert!(node_ids.contains(&c));
}

// ── Relationship type filtering ────────────────────────────────────────────

#[test]
fn bfs_rel_type_filter() {
    let store = InMemoryGraphStore::new();
    store.create_graph("g").unwrap();
    let a = store.create_node("g", vec![], IndexMap::new()).unwrap();
    let b = store.create_node("g", vec![], IndexMap::new()).unwrap();
    let c = store.create_node("g", vec![], IndexMap::new()).unwrap();

    store
        .create_edge("g", a, b, "KNOWS".to_string(), IndexMap::new())
        .unwrap();
    store
        .create_edge("g", a, c, "LIKES".to_string(), IndexMap::new())
        .unwrap();

    let mut budget = ExecutionBudget::unlimited();
    // Only follow KNOWS edges — should not reach c
    let trav = BfsTraversal::new(
        &store,
        "g",
        a,
        Direction::Outgoing,
        &["KNOWS"],
        10,
        &mut budget,
    );
    let ids: Vec<NodeId> = trav.map(|r| r.unwrap().0).collect();
    assert!(ids.contains(&b));
    assert!(!ids.contains(&c));
}

// ── exclude_start variant ─────────────────────────────────────────────────

#[test]
fn bfs_exclude_start_node() {
    let (store, ids) = build_chain(3);
    let mut budget = ExecutionBudget::unlimited();
    let trav = BfsTraversal::new_exclude_start(
        &store,
        "g",
        ids[0],
        Direction::Outgoing,
        &[],
        10,
        &mut budget,
    );
    let results: Vec<_> = trav.map(|r| r.unwrap()).collect();
    // ids[0] should not be in results
    assert!(!results.iter().any(|(nid, _)| *nid == ids[0]));
    assert_eq!(results.len(), 2);
}

// ── Budget enforcement ────────────────────────────────────────────────────

#[test]
fn bfs_respects_budget_timeout() {
    use crate::executor::budget::CHECK_INTERVAL;
    use std::time::Duration;
    let (store, ids) = build_chain(10);
    // Create an already-expired budget and pump the check counter close to
    // the amortization threshold so the next check_or_err() call actually
    // tests Instant::now().
    let mut budget = ExecutionBudget::new(Duration::from_millis(0), usize::MAX);
    std::thread::sleep(Duration::from_millis(5));
    for _ in 0..CHECK_INTERVAL - 1 {
        let _ = budget.check_or_err();
    }

    let mut trav = BfsTraversal::new(
        &store,
        "g",
        ids[0],
        Direction::Outgoing,
        &[],
        10,
        &mut budget,
    );
    // First call should trigger the amortized timeout check and return an error.
    let first = trav.next();
    assert!(first.is_some());
    assert!(first.unwrap().is_err());
}

// ── Isolated node (no edges) ──────────────────────────────────────────────

#[test]
fn bfs_isolated_node_yields_start_only() {
    let store = InMemoryGraphStore::new();
    store.create_graph("g").unwrap();
    let a = store.create_node("g", vec![], IndexMap::new()).unwrap();
    let mut budget = ExecutionBudget::unlimited();
    let trav = BfsTraversal::new(&store, "g", a, Direction::Outgoing, &[], 10, &mut budget);
    let results: Vec<_> = trav.map(|r| r.unwrap()).collect();
    assert_eq!(results, vec![(a, 0)]);
}

// ── Diamond / fan-out graph ───────────────────────────────────────────────

#[test]
fn bfs_diamond_each_node_once() {
    let store = InMemoryGraphStore::new();
    store.create_graph("g").unwrap();
    let a = store.create_node("g", vec![], IndexMap::new()).unwrap();
    let b = store.create_node("g", vec![], IndexMap::new()).unwrap();
    let c = store.create_node("g", vec![], IndexMap::new()).unwrap();
    let d = store.create_node("g", vec![], IndexMap::new()).unwrap();
    // a -> b, a -> c, b -> d, c -> d
    store
        .create_edge("g", a, b, "R".to_string(), IndexMap::new())
        .unwrap();
    store
        .create_edge("g", a, c, "R".to_string(), IndexMap::new())
        .unwrap();
    store
        .create_edge("g", b, d, "R".to_string(), IndexMap::new())
        .unwrap();
    store
        .create_edge("g", c, d, "R".to_string(), IndexMap::new())
        .unwrap();

    let mut budget = ExecutionBudget::unlimited();
    let trav = BfsTraversal::new(&store, "g", a, Direction::Outgoing, &[], 10, &mut budget);
    let results: Vec<_> = trav.map(|r| r.unwrap()).collect();
    // 4 nodes, each visited exactly once
    assert_eq!(results.len(), 4);
    let node_ids: Vec<NodeId> = results.iter().map(|(n, _)| *n).collect();
    // d appears once (not twice despite two paths reaching it)
    let d_count = node_ids.iter().filter(|&&n| n == d).count();
    assert_eq!(d_count, 1);
}
