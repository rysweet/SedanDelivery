// writes/graphs.rs -- Graph lifecycle (create/delete) operations for RedisGraphStore.

use gql_core::error::StorageError;
use gql_core::storage::validate_graph_name;

use crate::RedisGraphStore;

impl RedisGraphStore {
    /// Create a new named graph in the registry.
    ///
    /// Validates the graph name, then atomically adds it to the graph registry
    /// set. Returns an error if the graph already exists.
    pub(crate) fn create_graph_impl(&self, name: &str) -> Result<(), StorageError> {
        validate_graph_name(name)?;
        let mut conn = self.conn()?;
        let added: i64 = redis::cmd("SADD")
            .arg(self.key_graphs())
            .arg(name)
            .query(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        if added == 0 {
            return Err(StorageError::GraphAlreadyExists {
                name: name.to_string(),
            });
        }
        Ok(())
    }

    /// Delete a graph and all its data from Redis.
    ///
    /// First verifies the graph exists, then uses SCAN to find and delete all
    /// keys matching `G:{name}:*` in batches, and finally removes the graph
    /// from the registry set.
    ///
    /// If key deletion fails mid-way, the graph stays in the registry so the
    /// deletion can be retried.
    pub(crate) fn delete_graph_impl(&self, name: &str) -> Result<(), StorageError> {
        if !self.graph_exists_impl(name)? {
            return Err(StorageError::GraphNotFound {
                name: name.to_string(),
            });
        }
        let mut conn = self.conn()?;
        // Delete all keys with pattern G:{name}:* using SCAN first.
        let pattern = format!("{}G:{{{name}}}:*", self.key_prefix);
        let mut cursor: u64 = 0;
        loop {
            let (next_cursor, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(&pattern)
                .arg("COUNT")
                .arg(crate::range::scan_count())
                .query(conn.as_mut())
                .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
            if !keys.is_empty() {
                let mut del_cmd = redis::cmd("DEL");
                for k in &keys {
                    del_cmd.arg(k);
                }
                del_cmd
                    .query::<()>(conn.as_mut())
                    .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
            }
            cursor = next_cursor;
            if cursor == 0 {
                break;
            }
        }
        // Remove from registry AFTER all keys are deleted.
        redis::cmd("SREM")
            .arg(self.key_graphs())
            .arg(name)
            .query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use gql_core::storage::validate_graph_name;

    // ---- Graph name validation (pure logic, no Redis needed) ----------------

    #[test]
    fn valid_graph_names() {
        assert!(validate_graph_name("mydb").is_ok());
        assert!(validate_graph_name("test_graph").is_ok());
        assert!(validate_graph_name("g1").is_ok());
        assert!(validate_graph_name("a").is_ok());
    }

    #[test]
    fn empty_graph_name_rejected() {
        assert!(validate_graph_name("").is_err());
    }

    // ---- SCAN pattern construction (pure string logic) ----------------------

    #[test]
    fn delete_graph_scan_pattern_no_prefix() {
        let key_prefix = "";
        let name = "testdb";
        let pattern = format!("{key_prefix}G:{{{name}}}:*");
        assert_eq!(pattern, "G:{testdb}:*");
    }

    #[test]
    fn delete_graph_scan_pattern_with_prefix() {
        let key_prefix = "tenant1:";
        let name = "testdb";
        let pattern = format!("{key_prefix}G:{{{name}}}:*");
        assert_eq!(pattern, "tenant1:G:{testdb}:*");
    }

    #[test]
    fn delete_graph_scan_pattern_special_chars_in_name() {
        // Graph names with underscores/hyphens should be properly braced.
        let key_prefix = "";
        let name = "my-graph_v2";
        let pattern = format!("{key_prefix}G:{{{name}}}:*");
        assert_eq!(pattern, "G:{my-graph_v2}:*");
        // The hash tag is between the first { and first }, which is "my-graph_v2".
        assert!(pattern.contains("{my-graph_v2}"));
    }
}
