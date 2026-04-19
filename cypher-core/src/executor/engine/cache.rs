//! Property access cache helpers and ID interning for `QueryExecutor`.
//!
//! These helpers avoid redundant store reads when the same node or edge is
//! accessed multiple times within a single read-only operator (e.g.
//! `RETURN n.name, n.age, n.email`). Mutation operators must call
//! [`QueryExecutor::invalidate_property_cache`] to prevent stale reads.

use crate::error::GqlError;
use crate::storage::{EdgeData, EdgeId, NodeData, NodeId};

use super::QueryExecutor;

impl<'a> QueryExecutor<'a> {
    pub(crate) fn intern_property_id(&self, logical_key: String) -> i64 {
        if let Some(id) = self.property_id_map.borrow().get(&logical_key).copied() {
            return id;
        }
        let id = self.property_id_counter.get();
        self.property_id_counter.set(id + 1);
        self.property_id_map.borrow_mut().insert(logical_key, id);
        id
    }

    /// Fetch a node's data, returning a cached copy if available.
    /// On a cache miss the node is fetched from the store and cached.
    pub(crate) fn cached_get_node(
        &self,
        graph: &str,
        node_id: NodeId,
    ) -> Result<Option<NodeData>, GqlError> {
        {
            let cache = self.node_cache.borrow();
            if let Some(data) = cache.get(&node_id) {
                return Ok(Some(data.clone()));
            }
        }
        let result = self.store.get_node(graph, node_id)?;
        if let Some(ref data) = result {
            self.node_cache.borrow_mut().insert(node_id, data.clone());
        }
        Ok(result)
    }

    /// Fetch an edge's data, returning a cached copy if available.
    /// On a cache miss the edge is fetched from the store and cached.
    pub(crate) fn cached_get_edge(
        &self,
        graph: &str,
        edge_id: EdgeId,
    ) -> Result<Option<EdgeData>, GqlError> {
        {
            let cache = self.edge_cache.borrow();
            if let Some(data) = cache.get(&edge_id) {
                return Ok(Some(data.clone()));
            }
        }
        let result = self.store.get_edge(graph, edge_id)?;
        if let Some(ref data) = result {
            self.edge_cache.borrow_mut().insert(edge_id, data.clone());
        }
        Ok(result)
    }

    /// Invalidate the property access caches.  Must be called after any
    /// mutation (CREATE, SET, REMOVE, DELETE, MERGE) to prevent stale reads.
    pub(crate) fn invalidate_property_cache(&self) {
        self.node_cache.borrow_mut().clear();
        self.edge_cache.borrow_mut().clear();
    }
}
