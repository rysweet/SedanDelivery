//! BFS graph traversal for variable-length pattern matching.
//!
//! [`BfsTraversal`] implements a breadth-first traversal over a [`GraphStore`].
//! It yields `(NodeId, depth)` pairs and uses batch fetching (via
//! `out_edges_by_type` / `in_edges_by_type`) to minimise round-trips to the
//! storage backend.
//!
//! # Budget enforcement
//!
//! Every call to [`Iterator::next`] begins with `budget.check_or_err()`.
//! If the deadline has passed or the row count is exhausted, the iterator
//! immediately returns `Some(Err(...))`.  Callers MUST propagate this error.
//!
//! # Cycle avoidance
//!
//! A `FxHashSet<NodeId>` tracks visited nodes.  Each node is yielded at most
//! once, at its earliest (shallowest) depth.

use std::collections::VecDeque;

use rustc_hash::FxHashSet;

use crate::ast::Direction;
use crate::error::GqlError;
use crate::executor::budget::ExecutionBudget;
use crate::storage::{GraphStore, NodeId};

/// Breadth-first traversal over a graph stored in a [`GraphStore`].
///
/// # Example
///
/// ```ignore
/// let traversal = BfsTraversal::new(
///     store,
///     "social",
///     start_id,
///     Direction::Outgoing,
///     &["KNOWS"],
///     /*max_depth*/ 3,
///     &budget,
/// );
/// for result in traversal {
///     let (node_id, depth) = result?;
///     // process node
/// }
/// ```
pub struct BfsTraversal<'s> {
    store: &'s dyn GraphStore,
    graph: &'s str,
    queue: VecDeque<(NodeId, u32)>,
    visited: FxHashSet<NodeId>,
    /// Empty means all relationship types.
    rel_type_filter: Vec<String>,
    direction: Direction,
    max_depth: u32,
    budget: &'s ExecutionBudget,
    /// Whether to yield the start node itself (depth 0).
    include_start: bool,
    /// Pending error from a previous expansion step, to be returned next call.
    pending_error: Option<GqlError>,
}

impl<'s> BfsTraversal<'s> {
    /// Create a new BFS traversal.
    ///
    /// # Parameters
    ///
    /// - `store` — the graph storage backend
    /// - `graph` — name of the graph to traverse
    /// - `start` — starting node ID
    /// - `direction` — which edges to follow
    /// - `rel_types` — relationship types to follow; empty slice = all types
    /// - `max_depth` — maximum traversal depth (inclusive); 0 yields only the start node
    /// - `budget` — execution budget to check on every iteration step
    pub fn new(
        store: &'s dyn GraphStore,
        graph: &'s str,
        start: NodeId,
        direction: Direction,
        rel_types: &[&str],
        max_depth: u32,
        budget: &'s ExecutionBudget,
    ) -> Self {
        let mut visited = FxHashSet::default();
        visited.insert(start);
        let mut queue = VecDeque::new();
        queue.push_back((start, 0));
        Self {
            store,
            graph,
            queue,
            visited,
            rel_type_filter: rel_types.iter().map(|s| s.to_string()).collect(),
            direction,
            max_depth,
            budget,
            include_start: true,
            pending_error: None,
        }
    }

    /// Create a BFS traversal that does NOT yield the start node.
    ///
    /// Useful for `(a)-[*1..3]->(b)` patterns where `a` must not appear in
    /// the result set as a path endpoint unless separately matched.
    pub fn new_exclude_start(
        store: &'s dyn GraphStore,
        graph: &'s str,
        start: NodeId,
        direction: Direction,
        rel_types: &[&str],
        max_depth: u32,
        budget: &'s ExecutionBudget,
    ) -> Self {
        let mut t = Self::new(store, graph, start, direction, rel_types, max_depth, budget);
        t.include_start = false;
        t
    }

    /// Expand the node at the front of the queue, pushing its neighbours.
    /// Returns `Err` if storage access fails.
    fn expand(&mut self, node_id: NodeId, current_depth: u32) -> Result<(), GqlError> {
        if current_depth >= self.max_depth {
            return Ok(()); // do not expand beyond max depth
        }

        let filter_refs: Vec<&str> = self.rel_type_filter.iter().map(String::as_str).collect();

        let adj = match self.direction {
            Direction::Outgoing => {
                self.store
                    .out_edges_by_type(self.graph, node_id, &filter_refs)?
            }
            Direction::Incoming => {
                self.store
                    .in_edges_by_type(self.graph, node_id, &filter_refs)?
            }
            Direction::Undirected => {
                let mut out = self
                    .store
                    .out_edges_by_type(self.graph, node_id, &filter_refs)?;
                let in_ = self
                    .store
                    .in_edges_by_type(self.graph, node_id, &filter_refs)?;
                out.extend(in_);
                out
            }
        };

        let next_depth = current_depth + 1;
        for entry in adj {
            let neighbor = entry.neighbor_id;
            if !self.visited.contains(&neighbor) {
                self.visited.insert(neighbor);
                self.queue.push_back((neighbor, next_depth));
            }
        }
        Ok(())
    }
}

impl<'s> Iterator for BfsTraversal<'s> {
    type Item = Result<(NodeId, u32), GqlError>;

    fn next(&mut self) -> Option<Self::Item> {
        // Budget check FIRST, as mandated by executor architecture.
        if let Err(e) = self.budget.check_or_err() {
            return Some(Err(e));
        }

        // Propagate any error from a previous expansion.
        if let Some(e) = self.pending_error.take() {
            return Some(Err(e));
        }

        loop {
            let (node_id, depth) = self.queue.pop_front()?;

            // Expand this node's neighbours (unless we're already at max depth).
            if let Err(e) = self.expand(node_id, depth) {
                // Store the error and yield the current node first, then the
                // error on the next call.  This ensures the caller can
                // observe the partial traversal before seeing the error.
                self.pending_error = Some(e);
            }

            // Decide whether to yield this node.
            let should_yield = if depth == 0 { self.include_start } else { true };

            if should_yield {
                return Some(Ok((node_id, depth)));
            }
            // If we should not yield the start node, loop to get the next item.
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::ast::Direction;
    use crate::executor::budget::ExecutionBudget;
    use crate::storage::{InMemoryGraphStore, NodeId};

    fn build_chain(n: usize) -> (InMemoryGraphStore, Vec<NodeId>) {
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();
        let mut ids = Vec::new();
        for _ in 0..n {
            ids.push(store.create_node("g", vec![], HashMap::new()).unwrap());
        }
        for i in 0..n - 1 {
            store
                .create_edge("g", ids[i], ids[i + 1], "NEXT".to_string(), HashMap::new())
                .unwrap();
        }
        (store, ids)
    }

    // ── Basic traversal ────────────────────────────────────────────────────────

    #[test]
    fn bfs_linear_chain_outgoing() {
        let (store, ids) = build_chain(4);
        let budget = ExecutionBudget::unlimited();
        let trav = BfsTraversal::new(&store, "g", ids[0], Direction::Outgoing, &[], 10, &budget);
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
        let budget = ExecutionBudget::unlimited();
        let trav = BfsTraversal::new(&store, "g", ids[0], Direction::Outgoing, &[], 2, &budget);
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
        let a = store.create_node("g", vec![], HashMap::new()).unwrap();
        let b = store.create_node("g", vec![], HashMap::new()).unwrap();
        let c = store.create_node("g", vec![], HashMap::new()).unwrap();
        // a -> b -> c -> a (cycle)
        store
            .create_edge("g", a, b, "R".to_string(), HashMap::new())
            .unwrap();
        store
            .create_edge("g", b, c, "R".to_string(), HashMap::new())
            .unwrap();
        store
            .create_edge("g", c, a, "R".to_string(), HashMap::new())
            .unwrap();

        let budget = ExecutionBudget::unlimited();
        let trav = BfsTraversal::new(&store, "g", a, Direction::Outgoing, &[], 10, &budget);
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
        let budget = ExecutionBudget::unlimited();
        let trav = BfsTraversal::new(&store, "g", ids[2], Direction::Incoming, &[], 10, &budget);
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
        let a = store.create_node("g", vec![], HashMap::new()).unwrap();
        let b = store.create_node("g", vec![], HashMap::new()).unwrap();
        let c = store.create_node("g", vec![], HashMap::new()).unwrap();
        // a -> b, c -> b (b has incoming from both a and c)
        store
            .create_edge("g", a, b, "R".to_string(), HashMap::new())
            .unwrap();
        store
            .create_edge("g", c, b, "R".to_string(), HashMap::new())
            .unwrap();

        let budget = ExecutionBudget::unlimited();
        // Start from b, undirected: should reach both a and c
        let trav = BfsTraversal::new(&store, "g", b, Direction::Undirected, &[], 10, &budget);
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
        let a = store.create_node("g", vec![], HashMap::new()).unwrap();
        let b = store.create_node("g", vec![], HashMap::new()).unwrap();
        let c = store.create_node("g", vec![], HashMap::new()).unwrap();

        store
            .create_edge("g", a, b, "KNOWS".to_string(), HashMap::new())
            .unwrap();
        store
            .create_edge("g", a, c, "LIKES".to_string(), HashMap::new())
            .unwrap();

        let budget = ExecutionBudget::unlimited();
        // Only follow KNOWS edges — should not reach c
        let trav = BfsTraversal::new(&store, "g", a, Direction::Outgoing, &["KNOWS"], 10, &budget);
        let ids: Vec<NodeId> = trav.map(|r| r.unwrap().0).collect();
        assert!(ids.contains(&b));
        assert!(!ids.contains(&c));
    }

    // ── exclude_start variant ─────────────────────────────────────────────────

    #[test]
    fn bfs_exclude_start_node() {
        let (store, ids) = build_chain(3);
        let budget = ExecutionBudget::unlimited();
        let trav = BfsTraversal::new_exclude_start(
            &store,
            "g",
            ids[0],
            Direction::Outgoing,
            &[],
            10,
            &budget,
        );
        let results: Vec<_> = trav.map(|r| r.unwrap()).collect();
        // ids[0] should not be in results
        assert!(!results.iter().any(|(nid, _)| *nid == ids[0]));
        assert_eq!(results.len(), 2);
    }

    // ── Budget enforcement ────────────────────────────────────────────────────

    #[test]
    fn bfs_respects_budget_timeout() {
        use std::time::Duration;
        let (store, ids) = build_chain(10);
        // Create an already-expired budget.
        let budget = ExecutionBudget::new(Duration::from_millis(0), usize::MAX);
        std::thread::sleep(Duration::from_millis(5));

        let mut trav =
            BfsTraversal::new(&store, "g", ids[0], Direction::Outgoing, &[], 10, &budget);
        // First call should return an error.
        let first = trav.next();
        assert!(first.is_some());
        assert!(first.unwrap().is_err());
    }

    // ── Isolated node (no edges) ──────────────────────────────────────────────

    #[test]
    fn bfs_isolated_node_yields_start_only() {
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();
        let a = store.create_node("g", vec![], HashMap::new()).unwrap();
        let budget = ExecutionBudget::unlimited();
        let trav = BfsTraversal::new(&store, "g", a, Direction::Outgoing, &[], 10, &budget);
        let results: Vec<_> = trav.map(|r| r.unwrap()).collect();
        assert_eq!(results, vec![(a, 0)]);
    }

    // ── Diamond / fan-out graph ───────────────────────────────────────────────

    #[test]
    fn bfs_diamond_each_node_once() {
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();
        let a = store.create_node("g", vec![], HashMap::new()).unwrap();
        let b = store.create_node("g", vec![], HashMap::new()).unwrap();
        let c = store.create_node("g", vec![], HashMap::new()).unwrap();
        let d = store.create_node("g", vec![], HashMap::new()).unwrap();
        // a -> b, a -> c, b -> d, c -> d
        store
            .create_edge("g", a, b, "R".to_string(), HashMap::new())
            .unwrap();
        store
            .create_edge("g", a, c, "R".to_string(), HashMap::new())
            .unwrap();
        store
            .create_edge("g", b, d, "R".to_string(), HashMap::new())
            .unwrap();
        store
            .create_edge("g", c, d, "R".to_string(), HashMap::new())
            .unwrap();

        let budget = ExecutionBudget::unlimited();
        let trav = BfsTraversal::new(&store, "g", a, Direction::Outgoing, &[], 10, &budget);
        let results: Vec<_> = trav.map(|r| r.unwrap()).collect();
        // 4 nodes, each visited exactly once
        assert_eq!(results.len(), 4);
        let node_ids: Vec<NodeId> = results.iter().map(|(n, _)| *n).collect();
        // d appears once (not twice despite two paths reaching it)
        let d_count = node_ids.iter().filter(|&&n| n == d).count();
        assert_eq!(d_count, 1);
    }
}
