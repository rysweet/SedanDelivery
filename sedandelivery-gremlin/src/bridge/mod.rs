//! Bridge between the Gremlin protocol and gql-core.
//!
//! Translates Gremlin bytecode into gql-core execution and converts
//! results back to GraphSON values.

use gql_core::error::{GqlError, StorageError};
use gql_core::storage::GraphStore;

/// Ensure a graph exists, tolerating race conditions.
pub(crate) fn ensure_graph_exists(store: &dyn GraphStore, graph: &str) -> Result<(), GqlError> {
    if store.graph_exists(graph).map_err(GqlError::from)? {
        return Ok(());
    }

    match store.create_graph(graph) {
        Ok(()) | Err(StorageError::GraphAlreadyExists { .. }) => Ok(()),
        Err(err) => Err(GqlError::from(err)),
    }
}

pub mod executor;
pub mod types;
