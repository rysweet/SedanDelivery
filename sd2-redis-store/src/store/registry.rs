// Graph-registry lookups and key-prefix helpers used across the crate.

use cypher_core::error::StorageError;
use redis::ConnectionLike;

use crate::backend_storage_error;
use crate::keys::{key_graph_deleting, KEY_GRAPHS};

use super::RedisGraphStore;

impl RedisGraphStore {
    /// Check graph existence against the provided connection.
    pub(crate) fn graph_exists_on(
        &self,
        conn: &mut dyn ConnectionLike,
        g: &str,
    ) -> Result<bool, StorageError> {
        let registered: bool = redis::cmd("SISMEMBER")
            .arg(self.key_graphs())
            .arg(g)
            .query(conn)
            .map_err(backend_storage_error)?;
        if !registered {
            return Ok(false);
        }
        Ok(!self.graph_is_deleting_on(conn, g)?)
    }

    /// Return `true` while `delete_graph` cleanup is in progress for `g`.
    pub(crate) fn graph_is_deleting_on(
        &self,
        conn: &mut dyn ConnectionLike,
        g: &str,
    ) -> Result<bool, StorageError> {
        let deleting_key = self.kp(key_graph_deleting(g));
        let exists: i64 = redis::cmd("EXISTS")
            .arg(&deleting_key)
            .query(conn)
            .map_err(backend_storage_error)?;
        Ok(exists > 0)
    }

    /// Check that a graph exists on the provided connection; return error if not.
    pub(crate) fn require_graph_on(
        &self,
        conn: &mut dyn ConnectionLike,
        g: &str,
    ) -> Result<(), StorageError> {
        if !self.graph_exists_on(conn, g)? {
            return Err(StorageError::GraphNotFound {
                name: g.to_string(),
            });
        }
        Ok(())
    }

    /// Apply the configured key prefix to `key`.
    ///
    /// When `key_prefix` is empty (the default) this is a no-op clone.
    #[inline]
    pub(crate) fn kp(&self, key: String) -> String {
        if self.key_prefix.is_empty() {
            key
        } else {
            format!("{}{}", self.key_prefix, key)
        }
    }

    /// Return the prefixed graph-registry key.
    #[inline]
    pub(crate) fn key_graphs(&self) -> String {
        self.kp(KEY_GRAPHS.to_string())
    }
}
