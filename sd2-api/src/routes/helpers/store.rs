//! Store-access helpers: blocking task wrapper and graph existence checks.

use std::sync::Arc;

use cypher_core::storage::GraphStore;

use crate::error::ApiError;
use crate::state::AppState;

/// Run a synchronous store operation on Tokio's blocking pool.
pub(crate) async fn run_store_blocking<T, F>(state: &AppState, operation: F) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce(Arc<dyn GraphStore>) -> Result<T, ApiError> + Send + 'static,
{
    let store = Arc::clone(&state.store);
    tokio::task::spawn_blocking(move || operation(store))
        .await
        .map_err(|_| ApiError::internal("store task panicked"))?
}

/// Verify that a graph exists in the store; return a 404 error if not.
pub async fn require_graph(state: &AppState, name: &str) -> Result<(), ApiError> {
    let graph = name.to_string();
    run_store_blocking(state, move |store| {
        if !store.graph_exists(&graph).map_err(ApiError::from)? {
            return Err(ApiError::not_found(format!("graph '{graph}' not found")));
        }
        Ok(())
    })
    .await
}
