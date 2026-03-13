// indexing.rs — Property index management for RedisGraphStore.

use std::collections::HashMap;

use redis::ConnectionLike;

use gql_core::error::StorageError;
use gql_core::serde_value::json_to_props;
use gql_core::storage::IndexDescriptor;
use gql_core::types::Value;

use crate::keys::{key_indexes, key_label, key_node, key_propidx, value_to_index_key, IDX_SEP};
use crate::RedisGraphStore;

/// Configurable index-building batch size, read from `GQL_REDIS_INDEX_BATCH_SIZE`
/// env var (default 500, minimum 1).
fn index_batch_size() -> usize {
    static SIZE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *SIZE.get_or_init(|| {
        gql_core::config::parse_env_min("GQL_REDIS_INDEX_BATCH_SIZE", 500usize, 1)
    })
}

impl RedisGraphStore {
    /// Check which indexes exist for a given graph (returns set of "label\x00property" strings).
    pub(crate) fn active_indexes(
        &self,
        conn: &mut dyn ConnectionLike,
        graph: &str,
    ) -> Result<std::collections::HashSet<String>, StorageError> {
        let members: Vec<String> = redis::cmd("SMEMBERS")
            .arg(self.kp(key_indexes(graph)))
            .query(conn)
            .map_err(|e| { crate::record_backend_error(); StorageError::Backend { msg: e.to_string() } })?;
        Ok(members.into_iter().collect())
    }

    /// Add a node's properties to all active property indexes.
    pub(crate) fn index_node_properties(
        &self,
        conn: &mut dyn ConnectionLike,
        graph: &str,
        node_id: u64,
        labels: &[String],
        properties: &HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        let indexes = self.active_indexes(conn, graph)?;
        if indexes.is_empty() {
            return Ok(());
        }
        let mut pipe = redis::pipe();
        let mut has_cmds = false;
        for idx_spec in &indexes {
            if let Some((idx_label, idx_prop)) = idx_spec.split_once(IDX_SEP) {
                if labels.contains(&idx_label.to_string()) {
                    if let Some(val) = properties.get(idx_prop) {
                        if !val.is_null() {
                            let vkey = value_to_index_key(val);
                            pipe.cmd("SADD")
                                .arg(self.kp(key_propidx(graph, idx_label, idx_prop, &vkey)))
                                .arg(node_id.to_string())
                                .ignore();
                            has_cmds = true;
                        }
                    }
                }
            }
        }
        if has_cmds {
            pipe.query::<()>(conn)
                .map_err(|e| { crate::record_backend_error(); StorageError::Backend { msg: e.to_string() } })?;
        }
        Ok(())
    }

    /// Remove a node's properties from all active property indexes.
    pub(crate) fn deindex_node_properties(
        &self,
        conn: &mut dyn ConnectionLike,
        graph: &str,
        node_id: u64,
        labels: &[String],
        properties: &HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        let indexes = self.active_indexes(conn, graph)?;
        if indexes.is_empty() {
            return Ok(());
        }
        let mut pipe = redis::pipe();
        let mut has_cmds = false;
        for idx_spec in &indexes {
            if let Some((idx_label, idx_prop)) = idx_spec.split_once(IDX_SEP) {
                if labels.contains(&idx_label.to_string()) {
                    if let Some(val) = properties.get(idx_prop) {
                        if !val.is_null() {
                            let vkey = value_to_index_key(val);
                            pipe.cmd("SREM")
                                .arg(self.kp(key_propidx(graph, idx_label, idx_prop, &vkey)))
                                .arg(node_id.to_string())
                                .ignore();
                            has_cmds = true;
                        }
                    }
                }
            }
        }
        if has_cmds {
            pipe.query::<()>(conn)
                .map_err(|e| { crate::record_backend_error(); StorageError::Backend { msg: e.to_string() } })?;
        }
        Ok(())
    }

    // ── GraphStore trait method implementations for indexing ──────────────────

    pub(crate) fn list_indexes_impl(
        &self,
        graph: &str,
    ) -> Result<Vec<IndexDescriptor>, StorageError> {
        let mut conn = self.conn()?;
        let members: Vec<String> = redis::cmd("SMEMBERS")
            .arg(self.kp(key_indexes(graph)))
            .query(conn.as_mut())
            .map_err(|e| { crate::record_backend_error(); StorageError::Backend { msg: e.to_string() } })?;
        let mut result = Vec::new();
        for m in members {
            if let Some((label, property)) = m.split_once(IDX_SEP) {
                result.push(IndexDescriptor {
                    label: label.to_string(),
                    property: property.to_string(),
                });
            }
        }
        Ok(result)
    }

    pub(crate) fn create_index_impl(
        &self,
        graph: &str,
        label: &str,
        property: &str,
    ) -> Result<(), StorageError> {
        self.require_graph(graph)?;
        let mut conn = self.conn()?;
        // Record the index metadata.
        redis::cmd("SADD")
            .arg(self.kp(key_indexes(graph)))
            .arg(format!("{label}{IDX_SEP}{property}"))
            .query::<()>(conn.as_mut())
            .map_err(|e| { crate::record_backend_error(); StorageError::Backend { msg: e.to_string() } })?;

        // Build the index from existing nodes: scan all nodes with this label
        // and add entries for those that have the indexed property.
        let node_ids: Vec<String> = redis::cmd("SMEMBERS")
            .arg(self.kp(key_label(graph, label)))
            .query(conn.as_mut())
            .map_err(|e| { crate::record_backend_error(); StorageError::Backend { msg: e.to_string() } })?;

        // Pipeline in batches for efficiency.
        let batch_size = index_batch_size();
        for chunk in node_ids.chunks(batch_size) {
            // First, pipeline-read all nodes in this batch.
            let mut read_pipe = redis::pipe();
            for id_str in chunk {
                if let Ok(id) = id_str.parse::<u64>() {
                    read_pipe
                        .cmd("HMGET")
                        .arg(self.kp(key_node(graph, id)))
                        .arg("labels")
                        .arg("props");
                }
            }
            let raw: Vec<Vec<Option<String>>> = read_pipe
                .query(conn.as_mut())
                .map_err(|e| { crate::record_backend_error(); StorageError::Backend { msg: e.to_string() } })?;

            // Then pipeline-write all index entries.
            let mut write_pipe = redis::pipe();
            let mut has_cmds = false;
            for (i, fields) in raw.iter().enumerate() {
                if let (Some(Some(_)), Some(Some(props_json))) = (fields.first(), fields.get(1)) {
                    if let Ok(properties) = json_to_props(props_json) {
                        if let Some(val) = properties.get(property) {
                            if !val.is_null() {
                                let vkey = value_to_index_key(val);
                                write_pipe
                                    .cmd("SADD")
                                    .arg(self.kp(key_propidx(graph, label, property, &vkey)))
                                    .arg(&chunk[i])
                                    .ignore();
                                has_cmds = true;
                            }
                        }
                    }
                }
            }
            if has_cmds {
                write_pipe
                    .query::<()>(conn.as_mut())
                    .map_err(|e| { crate::record_backend_error(); StorageError::Backend { msg: e.to_string() } })?;
            }
        }
        Ok(())
    }

    pub(crate) fn drop_index_impl(
        &self,
        graph: &str,
        label: &str,
        property: &str,
    ) -> Result<(), StorageError> {
        let mut conn = self.conn()?;
        // Remove the metadata entry from the indexes set.
        redis::cmd("SREM")
            .arg(self.kp(key_indexes(graph)))
            .arg(format!("{label}{IDX_SEP}{property}"))
            .query::<()>(conn.as_mut())
            .map_err(|e| { crate::record_backend_error(); StorageError::Backend { msg: e.to_string() } })?;

        // Clean up orphaned propidx keys via SCAN+DEL.
        let pattern = format!(
            "{}G:{{{graph}}}:propidx:{label}{sep}{property}{sep}*",
            self.key_prefix,
            sep = IDX_SEP
        );
        let mut cursor: u64 = 0;
        loop {
            let (next_cursor, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(&pattern)
                .arg("COUNT")
                .arg(crate::range::scan_count())
                .query(conn.as_mut())
                .map_err(|e| { crate::record_backend_error(); StorageError::Backend { msg: e.to_string() } })?;
            if !keys.is_empty() {
                let mut del = redis::cmd("DEL");
                for key in &keys {
                    del.arg(key);
                }
                del.query::<()>(conn.as_mut())
                    .map_err(|e| { crate::record_backend_error(); StorageError::Backend { msg: e.to_string() } })?;
            }
            cursor = next_cursor;
            if cursor == 0 {
                break;
            }
        }
        Ok(())
    }
}
