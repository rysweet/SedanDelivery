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
///     &mut budget,
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
    budget: &'s mut ExecutionBudget,
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
        budget: &'s mut ExecutionBudget,
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
        budget: &'s mut ExecutionBudget,
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
