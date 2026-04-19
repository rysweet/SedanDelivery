//! Write-through cached graph store.
//!
//! Wraps any `GraphStore` backend with an `InMemoryGraphStore` read cache.
//! On first access to a graph, the entire graph is lazily loaded from the
//! backend into the cache. Subsequent reads are served from cache.
//! Writes go to the backend first, then the cache is updated in-place
//! (write-through) so subsequent reads stay in sync without a full reload.
//!
//! This avoids ID synchronization issues between backend and cache by
//! preserving the backend-allocated IDs when injecting into the cache.
//!
//! # Module layout
//!
//! - [`loading`]: lazy load of a full graph into the cache + invalidation.
//! - [`invalidation`]: in-cache mutation helpers used by write-through paths.
//! - [`reads`]: read-path inherent helpers used by the trait impl.
//! - [`writes`]: write-path inherent helpers used by the trait impl.
//! - [`schema_admin`]: schema and graph-administration inherent helpers.
//! - [`store_impl`]: the `GraphStore` trait implementation (single block,
//!   as required by Rust); each method delegates to a sibling helper.

use std::collections::HashSet;
use std::sync::{Arc, RwLock};

use crate::error::StorageError;
use crate::storage::{GraphStore, InMemoryGraphStore};

mod invalidation;
mod loading;
mod reads;
mod schema_admin;
mod store_impl;
mod writes;

#[cfg(test)]
mod tests;

/// A write-through cached graph store.
///
/// - Reads: lazy-load full graph into `InMemoryGraphStore` cache on first access,
///   then serve from cache.
/// - Writes: apply to backend first, then mirror the change into the cache
///   so subsequent reads stay coherent.
pub struct CachedGraphStore {
    backend: Arc<dyn GraphStore>,
    cache: InMemoryGraphStore,
    /// Set of graph names that have been loaded into the cache.
    loaded: RwLock<HashSet<String>>,
}

impl std::fmt::Debug for CachedGraphStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let loaded = self
            .loaded
            .read()
            .map(|g| g.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        f.debug_struct("CachedGraphStore")
            .field("loaded_graphs", &loaded)
            .finish()
    }
}

impl CachedGraphStore {
    /// Create a new cached store wrapping the given backend.
    pub fn new(backend: Arc<dyn GraphStore>) -> Self {
        Self {
            backend,
            cache: InMemoryGraphStore::new(),
            loaded: RwLock::new(HashSet::new()),
        }
    }

    /// Eagerly warm the cache for a specific graph.
    pub fn cache_graph(&self, graph: &str) -> Result<(), StorageError> {
        self.load_graph(graph)
    }

    /// Check if a graph is currently cached.
    pub fn is_cached(&self, graph: &str) -> bool {
        self.loaded
            .read()
            .map(|s| s.contains(graph))
            .unwrap_or(false)
    }
}
