//! Bridge between the Bolt protocol and cypher-core.
//!
//! Translates Bolt values into cypher-core `Value` types, executes Cypher
//! queries via cypher-core, and converts results back to Bolt records.

use cypher_core::error::{GqlError, StorageError};
use cypher_core::storage::GraphStore;

/// Ensure a graph exists, tolerating replica-lag false negatives.
///
/// Some Bolt flows consult `graph_exists()` through a replica-routed read path
/// before issuing a write. In master+replica topologies that read can lag the
/// primary briefly, so a graph that already exists on the primary may appear
/// absent and `create_graph()` can race with an existing registration. Treat
/// `GraphAlreadyExists` as success so the check-then-create sequence is safe.
pub(crate) fn ensure_graph_exists(store: &dyn GraphStore, graph: &str) -> Result<(), GqlError> {
    if store.graph_exists(graph).map_err(GqlError::from)? {
        return Ok(());
    }

    match store.create_graph(graph) {
        Ok(()) | Err(StorageError::GraphAlreadyExists { .. }) => Ok(()),
        Err(err) => Err(GqlError::from(err)),
    }
}

pub mod admin;
pub mod errors;
pub mod executor;
pub mod spatial;
pub mod temporal;
pub mod types;
