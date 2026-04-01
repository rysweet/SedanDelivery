// indexing.rs — Property index management for RedisGraphStore.

use indexmap::IndexMap;

use redis::ConnectionLike;

use cypher_core::error::StorageError;
use cypher_core::serde_value::json_to_props;
use cypher_core::storage::IndexDescriptor;
use cypher_core::types::Value;

use crate::keys::{
    key_indexes, key_label, key_node, key_propidx, key_propidx_manifest, value_to_index_key,
    IDX_SEP,
};
use crate::lock::WRITE_LOCK_TTL_MS;
use crate::reads::{for_each_set_page_locked, smembers_threshold};
use crate::validation::{validate_label, validate_property_key};
use crate::{LockedRedisConn, RedisGraphStore};

/// Configurable index-building batch size, read from `GQL_REDIS_INDEX_BATCH_SIZE`
/// env var (default 500, minimum 1).
fn index_batch_size() -> usize {
    static SIZE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *SIZE.get_or_init(|| {
        cypher_core::config::parse_env_min("GQL_REDIS_INDEX_BATCH_SIZE", 500usize, 1)
    })
}

impl RedisGraphStore {
    const INDEX_BUILD_LOCK_TTL_MS: u64 = WRITE_LOCK_TTL_MS * 12;

    /// Check which indexes exist for a given graph (returns set of "label\x00property" strings).
    pub(crate) fn active_indexes(
        &self,
        conn: &mut dyn ConnectionLike,
        graph: &str,
    ) -> Result<std::collections::HashSet<String>, StorageError> {
        let members: Vec<String> = redis::cmd("SMEMBERS")
            .arg(self.kp(key_indexes(graph)))
            .query(conn)
            .map_err(crate::backend_storage_error)?;
        Ok(members.into_iter().collect())
    }

    /// Add a node's properties to all active property indexes.
    pub(crate) fn index_node_properties(
        &self,
        conn: &mut LockedRedisConn,
        graph: &str,
        node_id: u64,
        labels: &[String],
        properties: &IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        conn.renew_lock()?;
        let indexes = self.active_indexes(conn.as_mut(), graph)?;
        if indexes.is_empty() {
            return Ok(());
        }
        let mut pipe = conn.pipeline();
        let mut has_cmds = false;
        let manifest_key = self.kp(key_propidx_manifest(graph));
        for (index, idx_spec) in indexes.iter().enumerate() {
            if index > 0 && index % 100 == 0 {
                conn.renew_lock()?;
            }
            if let Some((idx_label, idx_prop)) = idx_spec.split_once(IDX_SEP) {
                if labels.contains(&idx_label.to_string()) {
                    if let Some(val) = properties.get(idx_prop) {
                        if !val.is_null() {
                            let vkey = value_to_index_key(val);
                            let propidx_key =
                                self.kp(key_propidx(graph, idx_label, idx_prop, &vkey));
                            pipe.cmd("SADD")
                                .arg(&manifest_key)
                                .arg(&propidx_key)
                                .ignore();
                            pipe.cmd("SADD")
                                .arg(&propidx_key)
                                .arg(node_id.to_string())
                                .ignore();
                            has_cmds = true;
                        }
                    }
                }
            }
        }
        if has_cmds {
            conn.renew_lock()?;
            pipe.query::<()>(conn)
                .map_err(crate::backend_storage_error)?;
        }
        Ok(())
    }

    /// Remove a node's properties from all active property indexes.
    pub(crate) fn deindex_node_properties(
        &self,
        conn: &mut LockedRedisConn,
        graph: &str,
        node_id: u64,
        labels: &[String],
        properties: &IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        conn.renew_lock()?;
        let indexes = self.active_indexes(conn.as_mut(), graph)?;
        if indexes.is_empty() {
            return Ok(());
        }
        let mut pipe = conn.pipeline();
        let mut has_cmds = false;
        for (index, idx_spec) in indexes.iter().enumerate() {
            if index > 0 && index % 100 == 0 {
                conn.renew_lock()?;
            }
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
            conn.renew_lock()?;
            pipe.query::<()>(conn)
                .map_err(crate::backend_storage_error)?;
        }
        Ok(())
    }

    // ── GraphStore trait method implementations for indexing ──────────────────

    pub(crate) fn list_indexes_impl(
        &self,
        graph: &str,
    ) -> Result<Vec<IndexDescriptor>, StorageError> {
        let mut conn = self.conn()?;
        self.require_graph_on(conn.as_mut(), graph)?;
        let members: Vec<String> = redis::cmd("SMEMBERS")
            .arg(self.kp(key_indexes(graph)))
            .query(conn.as_mut())
            .map_err(crate::backend_storage_error)?;
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
        validate_label(label)?;
        validate_property_key(property)?;
        self.with_graph_write_lock(graph, Self::INDEX_BUILD_LOCK_TTL_MS, 10, 100, |conn| {
            self.require_graph_on(conn.as_mut(), graph)?;
            // Build the index from existing nodes before registering it so query
            // traffic never sees a partially-populated propidx set.
            let batch_size = index_batch_size();
            let label_key = self.kp(key_label(graph, label));
            let manifest_key = self.kp(key_propidx_manifest(graph));
            for_each_set_page_locked(conn, &label_key, smembers_threshold(), |conn, node_ids| {
                for chunk in node_ids.chunks(batch_size) {
                    conn.renew_lock()?;
                    let mut valid_ids = Vec::new();
                    let mut read_pipe = conn.pipeline();
                    for id_str in chunk {
                        if let Ok(id) = id_str.parse::<u64>() {
                            valid_ids.push(id_str.as_str());
                            read_pipe
                                .cmd("HMGET")
                                .arg(self.kp(key_node(graph, id)))
                                .arg("labels")
                                .arg("props");
                        }
                    }
                    if valid_ids.is_empty() {
                        continue;
                    }
                    let raw: Vec<Vec<Option<String>>> = read_pipe
                        .query(conn)
                        .map_err(crate::backend_storage_error)?;

                    let mut write_pipe = conn.pipeline();
                    let mut has_cmds = false;
                    for (id_str, fields) in valid_ids.into_iter().zip(raw.iter()) {
                        if let (Some(Some(_)), Some(Some(props_json))) =
                            (fields.first(), fields.get(1))
                        {
                            if let Ok(properties) = json_to_props(props_json) {
                                if let Some(val) = properties.get(property) {
                                    if !val.is_null() {
                                        let vkey = value_to_index_key(val);
                                        let propidx_key =
                                            self.kp(key_propidx(graph, label, property, &vkey));
                                        write_pipe
                                            .cmd("SADD")
                                            .arg(&manifest_key)
                                            .arg(&propidx_key)
                                            .ignore();
                                        write_pipe
                                            .cmd("SADD")
                                            .arg(&propidx_key)
                                            .arg(id_str)
                                            .ignore();
                                        has_cmds = true;
                                    }
                                }
                            }
                        }
                    }
                    if has_cmds {
                        write_pipe
                            .query::<()>(conn)
                            .map_err(crate::backend_storage_error)?;
                    }
                }
                Ok(())
            })?;

            conn.renew_lock()?;
            redis::cmd("SADD")
                .arg(self.kp(key_indexes(graph)))
                .arg(format!("{label}{IDX_SEP}{property}"))
                .query::<()>(conn.as_mut())
                .map_err(crate::backend_storage_error)?;
            Ok(())
        })
    }

    pub(crate) fn drop_index_impl(
        &self,
        graph: &str,
        label: &str,
        property: &str,
    ) -> Result<(), StorageError> {
        validate_label(label)?;
        validate_property_key(property)?;
        self.with_graph_write_lock(graph, WRITE_LOCK_TTL_MS, 3, 50, |conn| {
            self.require_graph_on(conn.as_mut(), graph)?;
            conn.renew_lock()?;
            // Remove the metadata entry from the indexes set.
            redis::cmd("SREM")
                .arg(self.kp(key_indexes(graph)))
                .arg(format!("{label}{IDX_SEP}{property}"))
                .query::<()>(conn.as_mut())
                .map_err(crate::backend_storage_error)?;

            // Reconstruct propidx keys from the indexed label set. This stays on
            // slot-local graph keys and avoids cluster-unsafe SCAN.
            let label_key = self.kp(key_label(graph, label));
            for_each_set_page_locked(
                conn,
                &label_key,
                smembers_threshold(),
                |conn, label_members| {
                    // Delete each page's propidx keys immediately. Repeated DELs
                    // across later pages are harmless, so we do not need a
                    // cross-page accumulator here.
                    let mut keys_to_delete: Vec<String> = Vec::new();
                    for chunk in label_members.chunks(500) {
                        conn.renew_lock()?;
                        let mut pipe = conn.pipeline();
                        for id_str in chunk {
                            if let Ok(id) = id_str.parse::<u64>() {
                                pipe.cmd("HGET")
                                    .arg(self.kp(key_node(graph, id)))
                                    .arg("props");
                            }
                        }
                        let results: Vec<Option<String>> =
                            pipe.query(conn).map_err(crate::backend_storage_error)?;
                        for json in results.into_iter().flatten() {
                            let props = json_to_props(&json)?;
                            if let Some(val) = props.get(property) {
                                if !val.is_null() {
                                    let vkey = value_to_index_key(val);
                                    keys_to_delete
                                        .push(self.kp(key_propidx(graph, label, property, &vkey)));
                                }
                            }
                        }
                    }
                    keys_to_delete.sort_unstable();
                    keys_to_delete.dedup();
                    for chunk in keys_to_delete.chunks(100) {
                        conn.renew_lock()?;
                        let mut del = redis::cmd("DEL");
                        for key in chunk {
                            del.arg(key.as_str());
                        }
                        del.query::<()>(conn.as_mut())
                            .map_err(crate::backend_storage_error)?;
                    }
                    Ok(())
                },
            )?;
            Ok(())
        })
    }
}
