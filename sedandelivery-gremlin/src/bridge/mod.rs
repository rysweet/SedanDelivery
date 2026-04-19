//! Bridge between the Gremlin protocol and cypher-core.
//!
//! Translates Gremlin bytecode into cypher-core execution and converts
//! results back to GraphSON values.

use cypher_core::error::{GqlError, StorageError};
use cypher_core::storage::GraphStore;

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

pub(crate) mod cypher_translation;
pub mod executor;
pub mod types;
