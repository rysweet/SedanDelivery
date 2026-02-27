//! Shared application state — one `InMemoryGraphStore` per named graph.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use gql_core::storage::InMemoryGraphStore;

/// All named graphs held in memory.
///
/// The outer `Arc` enables cheap cloning into Axum handlers.
/// The inner `Mutex` serialises concurrent access **to the map** only — it is
/// released before any query/mutate execution.  Callers acquire the lock,
/// clone the `Arc<InMemoryGraphStore>` (cheap ref-count bump), then release
/// the lock, and run the executor against the cloned Arc.  This prevents the
/// mutex from being held across potentially slow CPU-bound query execution.
///
/// `InMemoryGraphStore` has its own internal `RwLock` for all data access,
/// so concurrent queries against the same graph are still parallelised.
#[derive(Clone)]
pub struct AppState {
    pub stores: Arc<Mutex<HashMap<String, Arc<InMemoryGraphStore>>>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            stores: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Returns a cloned `Arc` to the named store, or `None` if the graph
    /// does not exist.  The map lock is held only for the duration of the
    /// lookup — callers may execute queries without holding the mutex.
    pub fn get_store(&self, name: &str) -> Option<Arc<InMemoryGraphStore>> {
        self.stores
            .lock()
            .ok()?
            .get(name)
            .cloned()
    }
}
