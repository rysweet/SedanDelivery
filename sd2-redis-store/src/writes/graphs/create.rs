// writes/graphs/create.rs -- Graph creation lifecycle for RedisGraphStore.

use cypher_core::error::StorageError;
use cypher_core::storage::validate_graph_name;

use crate::lock::WRITE_LOCK_TTL_MS;
use crate::RedisGraphStore;

impl RedisGraphStore {
    /// Create a new named graph in the registry.
    ///
    /// Validates the graph name, then adds it to the graph registry set.
    /// Returns an error if the graph already exists.
    pub(crate) fn create_graph_impl(&self, name: &str) -> Result<(), StorageError> {
        validate_graph_name(name)?;
        self.with_graph_write_lock(name, WRITE_LOCK_TTL_MS, 3, 50, |conn| {
            if self.graph_is_deleting_on(conn.as_mut(), name)? {
                return Err(StorageError::TransactionConflict);
            }
            let added: i64 = redis::cmd("SADD")
                .arg(self.key_graphs())
                .arg(name)
                .query(conn.as_mut())
                .map_err(crate::backend_storage_error)?;
            if added == 0 {
                return Err(StorageError::GraphAlreadyExists {
                    name: name.to_string(),
                });
            }
            Ok(())
        })
    }
}
