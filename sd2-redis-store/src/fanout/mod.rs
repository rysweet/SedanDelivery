// fanout — FanoutStore: write to all backends, read from the first.
//
// Typical use: FanoutStore { stores: [RedisGraphStore, InMemoryGraphStore] }
// ensures all writes reach both backends while reads always come from Redis.
//
// This module is decomposed into focused submodules:
//   - mod.rs        — `FanoutStore` struct, constructors, and dispatch helpers
//   - graph_store   — the single `impl GraphStore for FanoutStore` block
//   - tests/        — unit tests, split by concern (propagation, rollback, admin)

use cypher_core::error::StorageError;
use cypher_core::storage::GraphStore;

mod graph_store;

#[cfg(test)]
mod tests;

/// Writes to ALL backing stores simultaneously; reads from the first one.
///
/// If any write fails, the error from that store is returned immediately.
/// The caller must choose a suitable retry/rollback strategy.
pub struct FanoutStore {
    stores: Vec<Box<dyn GraphStore>>,
}

impl FanoutStore {
    /// Create a fanout store over `stores`.
    ///
    /// # Panics
    /// Panics if `stores` is empty — a fanout over zero backends makes no sense.
    pub fn new(stores: Vec<Box<dyn GraphStore>>) -> Self {
        assert!(
            !stores.is_empty(),
            "FanoutStore requires at least one backend"
        );
        Self { stores }
    }

    /// Apply a read operation against the primary (first) store.
    fn read<T, F>(&self, f: F) -> Result<T, StorageError>
    where
        F: Fn(&dyn GraphStore) -> Result<T, StorageError>,
    {
        f(self.stores[0].as_ref())
    }

    /// Apply a write operation against all stores. Returns on the first error.
    fn write_all<F>(&self, f: F) -> Result<(), StorageError>
    where
        F: Fn(&dyn GraphStore) -> Result<(), StorageError>,
    {
        for store in &self.stores {
            f(store.as_ref())?;
        }
        Ok(())
    }
}
