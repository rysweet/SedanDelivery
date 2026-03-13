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
    /// First verifies the graph exists, then removes it from the registry
    /// (so new operations immediately get `GraphNotFound`), and finally runs
    /// a Lua `SCAN`+`DEL` script to atomically clean up all `G:{name}:*` keys.
    ///
    /// If the Lua script fails mid-way, the graph is already gone from the
    /// registry (no new operations can reach it), but orphaned data keys may
    /// remain until manual cleanup or a retry of the deletion.
    pub(crate) fn delete_graph_impl(&self, name: &str) -> Result<(), StorageError> {
        if !self.graph_exists_impl(name)? {
            return Err(StorageError::GraphNotFound {
                name: name.to_string(),
            });
        }
        let mut conn = self.conn()?;
        // Remove from registry first.
        redis::cmd("SREM")
            .arg(self.key_graphs())
            .arg(name)
            .query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        // Atomic SCAN+DEL via Lua to prevent partial-delete windows.
        // All G:{name}: keys share the same hash tag -> same Redis Cluster slot,
        // so the Lua script can safely SCAN and DEL them on one node.
        const DELETE_GRAPH_LUA: &str = r#"
local cursor = '0'
local pattern = ARGV[1]
repeat
    local result = redis.call('SCAN', cursor, 'MATCH', pattern, 'COUNT', 1000)
    cursor = result[1]
    local keys = result[2]
    if #keys > 0 then
        redis.call('DEL', unpack(keys))
    end
until cursor == '0'
return 1
"#;
        let pattern = format!("{}G:{{{name}}}:*", self.key_prefix);
        redis::Script::new(DELETE_GRAPH_LUA)
            .arg(&pattern)
            .invoke::<i64>(conn.as_mut())
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
