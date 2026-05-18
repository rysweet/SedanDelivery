//! `InMemoryGraphStore` struct definition, snapshot/merge helpers, and the
//! shared lock-acquisition + graph-lookup utilities used across submodules.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::error::StorageError;

use super::graph_inner::GraphInner;

/// Thread-safe in-memory implementation of [`GraphStore`].
///
/// Uses an `Arc`-based copy-on-write approach for snapshot isolation: the inner
/// state is wrapped in `Arc` so readers can clone the `Arc` (a cheap refcount
/// bump) to obtain a frozen snapshot that is immune to concurrent writes.
/// Writers call `Arc::make_mut` which clones the data only when other references
/// exist, providing copy-on-write semantics.
///
/// [`GraphStore`]: crate::storage::GraphStore
pub struct InMemoryGraphStore {
    pub(crate) inner: RwLock<Arc<HashMap<String, GraphInner>>>,
}

impl std::fmt::Debug for InMemoryGraphStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.inner.read() {
            Ok(g) => {
                let graph_names: Vec<String> = g.keys().cloned().collect();
                f.debug_struct("InMemoryGraphStore")
                    .field("graphs", &graph_names)
                    .finish()
            }
            Err(_) => f.write_str("[lock poisoned]"),
        }
    }
}

impl InMemoryGraphStore {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(Arc::new(HashMap::new())),
        }
    }

    /// Create a node with an explicit, caller-supplied [`NodeId`].
    ///
    /// Unlike [`GraphStore::create_node`], this bypasses the per-graph ID
    /// allocator and uses `id` verbatim; the internal seq counter is advanced
    /// past `id` so subsequent auto-allocated IDs will not collide.
    ///
    /// Idempotent: if a node with `id` already exists in `graph`, this is a
    /// no-op. This is the primitive used by WAL replay to achieve in-place
    /// idempotency (issue #1701).
    ///
    /// [`GraphStore::create_node`]: crate::storage::GraphStore::create_node
    pub(crate) fn create_node_with_id(
        &self,
        graph: &str,
        id: crate::storage::NodeId,
        labels: Vec<String>,
        properties: indexmap::IndexMap<String, crate::types::Value>,
    ) -> Result<(), StorageError> {
        super::nodes::create_node_with_id_impl(self, graph, id, labels, properties)
    }

    /// Create an edge with an explicit, caller-supplied [`EdgeId`].
    ///
    /// Mirrors [`Self::create_node_with_id`]. Idempotent: a pre-existing edge
    /// with `id` makes this a no-op.
    pub(crate) fn create_edge_with_id(
        &self,
        graph: &str,
        id: crate::storage::EdgeId,
        src: crate::storage::NodeId,
        dst: crate::storage::NodeId,
        rel_type: String,
        properties: indexmap::IndexMap<String, crate::types::Value>,
    ) -> Result<(), StorageError> {
        super::edges::create_edge_with_id_impl(self, graph, id, src, dst, rel_type, properties)
    }

    /// Allocate the next node ID without creating a node.
    ///
    /// Used by [`PersistentGraphStore`] to write the WAL entry *before* the
    /// in-memory mutation (write-ahead semantics).
    pub(crate) fn allocate_node_id(
        &self,
        graph: &str,
    ) -> Result<crate::storage::NodeId, StorageError> {
        let mut guard = self.write()?;
        let map = Arc::make_mut(&mut guard);
        let g = map
            .get_mut(graph)
            .ok_or_else(|| StorageError::GraphNotFound {
                name: graph.to_string(),
            })?;
        let id = g.next_node_id(graph)?;
        Ok(crate::storage::NodeId(id))
    }

    /// Allocate the next edge ID without creating an edge.
    pub(crate) fn allocate_edge_id(
        &self,
        graph: &str,
    ) -> Result<crate::storage::EdgeId, StorageError> {
        let mut guard = self.write()?;
        let map = Arc::make_mut(&mut guard);
        let g = map
            .get_mut(graph)
            .ok_or_else(|| StorageError::GraphNotFound {
                name: graph.to_string(),
            })?;
        let id = g.next_edge_id(graph)?;
        Ok(crate::storage::EdgeId(id))
    }

    pub(crate) fn read_snapshot(&self) -> Result<Arc<HashMap<String, GraphInner>>, StorageError> {
        let guard = self
            .inner
            .read()
            .map_err(|e| StorageError::backend(format!("lock poisoned (read): {e}")))?;
        Ok(Arc::clone(&guard))
    }

    pub(crate) fn write(
        &self,
    ) -> Result<std::sync::RwLockWriteGuard<'_, Arc<HashMap<String, GraphInner>>>, StorageError>
    {
        self.inner
            .write()
            .map_err(|e| StorageError::backend(format!("lock poisoned (write): {e}")))
    }

    pub fn snapshot(&self, graphs: &[&str]) -> Result<InMemoryGraphStore, StorageError> {
        let snap = self.read_snapshot()?;
        let mut snapshot_inner = HashMap::new();
        for &name in graphs {
            if let Some(graph) = snap.get(name) {
                snapshot_inner.insert(name.to_string(), graph.clone());
            }
        }
        Ok(InMemoryGraphStore {
            inner: RwLock::new(Arc::new(snapshot_inner)),
        })
    }

    pub fn merge_from(&self, other: &InMemoryGraphStore) -> Result<(), StorageError> {
        let other_snap = other.read_snapshot()?;
        let mut self_guard = self.write()?;
        let map = Arc::make_mut(&mut self_guard);
        for (name, graph) in other_snap.iter() {
            map.insert(name.clone(), graph.clone());
        }
        Ok(())
    }

    pub(crate) fn require_graph<'g>(
        map: &'g HashMap<String, GraphInner>,
        graph: &str,
    ) -> Result<&'g GraphInner, StorageError> {
        map.get(graph).ok_or_else(|| StorageError::GraphNotFound {
            name: graph.to_string(),
        })
    }

    pub(crate) fn require_graph_mut<'g>(
        map: &'g mut HashMap<String, GraphInner>,
        graph: &str,
    ) -> Result<&'g mut GraphInner, StorageError> {
        map.get_mut(graph)
            .ok_or_else(|| StorageError::GraphNotFound {
                name: graph.to_string(),
            })
    }
}

impl Default for InMemoryGraphStore {
    fn default() -> Self {
        Self::new()
    }
}
