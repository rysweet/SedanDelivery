//! Shared application state backed by a single `GraphStore` implementation.
//!
//! When `GQL_REDIS_URL` is set, the store is a `RedisGraphStore` (persistent,
//! with r2d2 connection pooling).  Otherwise an `InMemoryGraphStore` is used.

use std::sync::Arc;

use cypher_core::storage::GraphStore;

/// Application state shared across all Axum handlers.
///
/// Holds a single `Arc<dyn GraphStore>` that manages all named graphs.
/// The concrete type is either `RedisGraphStore` or `InMemoryGraphStore`,
/// selected at startup based on the `GQL_REDIS_URL` environment variable.
#[derive(Clone)]
pub struct AppState {
    pub store: Arc<dyn GraphStore>,
}

impl AppState {
    pub fn new(store: Arc<dyn GraphStore>) -> Self {
        Self { store }
    }
}
