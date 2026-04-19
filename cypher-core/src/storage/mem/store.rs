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
