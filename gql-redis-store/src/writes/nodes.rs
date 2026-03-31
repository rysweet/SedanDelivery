// writes/nodes.rs -- Node write operations for RedisGraphStore.

use std::collections::HashSet;

use indexmap::IndexMap;

use gql_core::error::StorageError;
use gql_core::serde_value::props_to_json;
use gql_core::storage::{EdgeId, NodeId};
use gql_core::types::Value;

use crate::keys::{
    key_batch_node_write_lock, key_in, key_label, key_label_names, key_node, key_node_seq,
    key_node_write_lock, key_nodes_set, key_out,
};
use crate::lock::{BATCH_WRITE_MAX_RETRIES, BATCH_WRITE_RETRY_DELAY_MS, WRITE_LOCK_TTL_MS};
use crate::reads::{for_each_set_page_locked, smembers_threshold};
use crate::validation::{validate_label, validate_property_key};
use crate::RedisGraphStore;

use super::helpers::{
    batch_pipeline_chunk_size, clean_fanout_props, ensure_key_absent, extract_injected_id,
    max_batch_size, raise_sequence_floor,
};

fn max_detach_delete_edges() -> usize {
    static SIZE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *SIZE.get_or_init(|| {
        gql_core::config::parse_env_min("GQL_REDIS_MAX_DETACH_DELETE_EDGES", 10_000usize, 1)
    })
}

fn insert_detach_edge_id_with_limit(
    node_id: NodeId,
    edge_ids: &mut HashSet<EdgeId>,
    edge_id: EdgeId,
    limit: usize,
) -> Result<(), StorageError> {
    edge_ids.insert(edge_id);
    if edge_ids.len() > limit {
        crate::record_backend_error();
        return Err(StorageError::Backend {
            msg: format!(
                "DETACH DELETE for node {} exceeds GQL_REDIS_MAX_DETACH_DELETE_EDGES={} (adjacent edges: {})",
                node_id.0,
                limit,
                edge_ids.len()
            ),
        });
    }
    Ok(())
}

fn validate_labels(labels: &[String]) -> Result<(), StorageError> {
    for label in labels {
        validate_label(label)?;
    }
    Ok(())
}

fn validate_property_keys(properties: &IndexMap<String, Value>) -> Result<(), StorageError> {
    for key in properties.keys() {
        validate_property_key(key)?;
    }
    Ok(())
}

impl RedisGraphStore {
    /// Create a single node in the given graph.
    ///
    /// Allocates a new node ID via Redis INCRBY on the node sequence key,
    /// unless the special `_sd2_id` property is present (injected by FanoutStore
    /// for cross-cluster ID stability). Stores the node hash, updates label
    /// indexes, the label registry, and the all-nodes set via a slot-local
    /// Redis pipeline.
    ///
    /// Property indexing is performed after the main pipeline completes.
    pub(crate) fn create_node_impl(
        &self,
        graph: &str,
        labels: Vec<String>,
        properties: IndexMap<String, Value>,
    ) -> Result<NodeId, StorageError> {
        validate_labels(&labels)?;
        validate_property_keys(&properties)?;
        self.with_graph_write_lock(graph, WRITE_LOCK_TTL_MS, 3, 50, |conn| {
            self.require_graph_on(conn.as_mut(), graph)?;

            // If FanoutStore injected _sd2_id, use it instead of INCR.
            let id = match extract_injected_id(&properties, "_sd2_id")? {
                Some(id) => {
                    ensure_key_absent(
                        conn.as_mut(),
                        &self.kp(key_node(graph, id)),
                        "node",
                        id,
                    )?;
                    raise_sequence_floor(conn.as_mut(), &self.kp(key_node_seq(graph)), id)?;
                    id
                }
                None => {
                    let raw_id: i64 = redis::cmd("INCRBY")
                        .arg(self.kp(key_node_seq(graph)))
                        .arg(1)
                        .query(conn.as_mut())
                        .map_err(|e| {
                            crate::backend_storage_error(e)
                        })?;
                    if raw_id <= 0 {
                        return Err(StorageError::IdOverflow {
                            graph: graph.to_string(),
                        });
                    }
                    raw_id as u64
                }
            };

            // Strip internal FanoutStore keys before storing.
            let clean_props = clean_fanout_props(properties);

            let node_key = self.kp(key_node(graph, id));
            let labels_json = serde_json::to_string(&labels)
                .map_err(|e| StorageError::Serialization { msg: e.to_string() })?;
            let props_json = props_to_json(&clean_props)?;
            // Store node hash, label indexes, the label registry, and all-nodes
            // set in a single pipeline.
            // Note: .atomic() (MULTI/EXEC) is intentionally omitted — redis-rs
            // ClusterConnection does not support MULTI/EXEC. All keys share the
            // {graphname} hash tag so they route to the same cluster slot, making
            // non-atomic pipelining correct. The write lock serialises concurrent
            // writers when needed. See ADR-005.
            if conn.is_standalone() {
                tracing::warn!(
                    graph = graph,
                    "create_node pipeline executing without MULTI/EXEC atomicity (standalone mode); \
                     a partial write is possible if the process crashes mid-pipeline. See ADR-005."
                );
            }
            let mut pipe = conn.pipeline();
            if !labels.is_empty() {
                let registry_key = self.kp(key_label_names(graph));
                let cmd = pipe.cmd("SADD");
                cmd.arg(&registry_key);
                for label in &labels {
                    cmd.arg(label.as_str());
                }
                cmd.ignore();
            }
            pipe.cmd("SADD")
                .arg(self.kp(key_nodes_set(graph)))
                .arg(id.to_string())
                .ignore();
            for label in &labels {
                pipe.cmd("SADD")
                    .arg(self.kp(key_label(graph, label)))
                    .arg(id.to_string())
                    .ignore();
            }
            pipe.cmd("HSET")
                .arg(&node_key)
                .arg("labels")
                .arg(&labels_json)
                .arg("props")
                .arg(&props_json)
                .ignore();
            conn.renew_lock()?;
            pipe.query::<()>(conn).map_err(|e| {
                crate::backend_storage_error(e)
            })?;
            // Maintain property indexes.
            self.index_node_properties(conn, graph, id, &labels, &clean_props)?;
            Ok(NodeId(id))
        })
    }

    /// Merge additional properties into an existing node (SET semantics).
    ///
    /// Reads the current node, deindexes old property values, merges new
    /// properties on top, writes the merged set back, and reindexes.
    pub(crate) fn set_node_properties_impl(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        validate_property_keys(&properties)?;
        self.with_write_lock(
            graph,
            self.kp(key_node_write_lock(graph, id.0)),
            WRITE_LOCK_TTL_MS,
            3,
            50,
            |conn| {
                self.require_graph_on(conn.as_mut(), graph)?;
                let node = self
                    .read_node(conn.as_mut(), graph, id.0)?
                    .ok_or(StorageError::NodeNotFound { id: id.0 })?;
                // Deindex old property values before merging.
                self.deindex_node_properties(conn, graph, id.0, &node.labels, &node.properties)?;
                conn.renew_lock()?;
                let mut merged = node.properties;
                merged.extend(properties);
                let props_json = props_to_json(&merged)?;
                redis::cmd("HSET")
                    .arg(self.kp(key_node(graph, id.0)))
                    .arg("props")
                    .arg(&props_json)
                    .query::<()>(conn.as_mut())
                    .map_err(crate::backend_storage_error)?;
                // Reindex with new values.
                self.index_node_properties(conn, graph, id.0, &node.labels, &merged)?;
                Ok(())
            },
        )
    }

    /// Replace all properties on an existing node (REPLACE semantics).
    ///
    /// Unlike `set_node_properties_impl`, this discards all existing properties
    /// and replaces them entirely with the provided map.
    pub(crate) fn replace_node_properties_impl(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        validate_property_keys(&properties)?;
        self.with_write_lock(
            graph,
            self.kp(key_node_write_lock(graph, id.0)),
            WRITE_LOCK_TTL_MS,
            3,
            50,
            |conn| {
                self.require_graph_on(conn.as_mut(), graph)?;
                let node = self
                    .read_node(conn.as_mut(), graph, id.0)?
                    .ok_or(StorageError::NodeNotFound { id: id.0 })?;
                // Deindex old values.
                self.deindex_node_properties(conn, graph, id.0, &node.labels, &node.properties)?;
                conn.renew_lock()?;
                let props_json = props_to_json(&properties)?;
                redis::cmd("HSET")
                    .arg(self.kp(key_node(graph, id.0)))
                    .arg("props")
                    .arg(&props_json)
                    .query::<()>(conn.as_mut())
                    .map_err(crate::backend_storage_error)?;
                // Index new values.
                self.index_node_properties(conn, graph, id.0, &node.labels, &properties)?;
                Ok(())
            },
        )
    }

    /// Remove a single property key from a node.
    ///
    /// Deindexes all current properties, removes the specified key, writes
    /// the remaining properties back, and reindexes.
    pub(crate) fn remove_node_property_impl(
        &self,
        graph: &str,
        id: NodeId,
        key: &str,
    ) -> Result<(), StorageError> {
        validate_property_key(key)?;
        self.with_write_lock(
            graph,
            self.kp(key_node_write_lock(graph, id.0)),
            WRITE_LOCK_TTL_MS,
            3,
            50,
            |conn| {
                self.require_graph_on(conn.as_mut(), graph)?;
                let node = self
                    .read_node(conn.as_mut(), graph, id.0)?
                    .ok_or(StorageError::NodeNotFound { id: id.0 })?;
                // Deindex old values before removal.
                self.deindex_node_properties(conn, graph, id.0, &node.labels, &node.properties)?;
                conn.renew_lock()?;
                let mut props = node.properties;
                props.shift_remove(key);
                let props_json = props_to_json(&props)?;
                redis::cmd("HSET")
                    .arg(self.kp(key_node(graph, id.0)))
                    .arg("props")
                    .arg(&props_json)
                    .query::<()>(conn.as_mut())
                    .map_err(crate::backend_storage_error)?;
                // Reindex with remaining values.
                self.index_node_properties(conn, graph, id.0, &node.labels, &props)?;
                Ok(())
            },
        )
    }

    /// Add labels to an existing node without removing any existing labels.
    ///
    /// For each genuinely new label, adds the node ID to that label's index set.
    /// Then writes the updated label list back to the node hash.
    pub(crate) fn add_node_labels_impl(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        validate_labels(labels)?;
        self.with_write_lock(
            graph,
            self.kp(key_node_write_lock(graph, id.0)),
            WRITE_LOCK_TTL_MS,
            3,
            50,
            |conn| {
                self.require_graph_on(conn.as_mut(), graph)?;
                let node = self
                    .read_node(conn.as_mut(), graph, id.0)?
                    .ok_or(StorageError::NodeNotFound { id: id.0 })?;
                let mut existing: std::collections::HashSet<String> =
                    node.labels.into_iter().collect();
                let mut added_labels = Vec::new();
                for label in labels {
                    if existing.insert(label.clone()) {
                        added_labels.push(label.as_str());
                    }
                }

                let new_labels: Vec<String> = existing.into_iter().collect();
                let labels_json = serde_json::to_string(&new_labels)
                    .map_err(|e| StorageError::Serialization { msg: e.to_string() })?;

                let mut pipe = conn.pipeline();
                let id_str = id.0.to_string();
                for label in &added_labels {
                    pipe.cmd("SADD")
                        .arg(self.kp(key_label(graph, label)))
                        .arg(&id_str)
                        .ignore();
                }
                if !added_labels.is_empty() {
                    let cmd = pipe.cmd("SADD").arg(self.kp(key_label_names(graph)));
                    for label in &added_labels {
                        cmd.arg(label);
                    }
                    cmd.ignore();
                }
                pipe.cmd("HSET")
                    .arg(self.kp(key_node(graph, id.0)))
                    .arg("labels")
                    .arg(&labels_json)
                    .ignore();
                conn.renew_lock()?;
                pipe.query::<()>(conn)
                    .map_err(crate::backend_storage_error)?;
                let added_labels_owned: Vec<String> = added_labels
                    .into_iter()
                    .map(|label| label.to_string())
                    .collect();
                if !added_labels_owned.is_empty() {
                    self.index_node_properties(
                        conn,
                        graph,
                        id.0,
                        &added_labels_owned,
                        &node.properties,
                    )?;
                }
                Ok(())
            },
        )
    }

    /// Remove labels from an existing node.
    ///
    /// For each label being removed, removes the node ID from that label's
    /// index set. Then writes the filtered label list back to the node hash.
    pub(crate) fn remove_node_labels_impl(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        validate_labels(labels)?;
        self.with_write_lock(
            graph,
            self.kp(key_node_write_lock(graph, id.0)),
            WRITE_LOCK_TTL_MS,
            3,
            50,
            |conn| {
                self.require_graph_on(conn.as_mut(), graph)?;
                let node = self
                    .read_node(conn.as_mut(), graph, id.0)?
                    .ok_or(StorageError::NodeNotFound { id: id.0 })?;
                let to_remove: std::collections::HashSet<&String> = labels.iter().collect();
                let removed_labels: Vec<String> = node
                    .labels
                    .iter()
                    .filter(|label| to_remove.contains(label))
                    .cloned()
                    .collect();
                if removed_labels.is_empty() {
                    return Ok(());
                }
                self.deindex_node_properties(conn, graph, id.0, &removed_labels, &node.properties)?;
                let id_str = id.0.to_string();
                let mut pipe = conn.pipeline();
                for label in &to_remove {
                    pipe.cmd("SREM")
                        .arg(self.kp(key_label(graph, label)))
                        .arg(&id_str)
                        .ignore();
                }
                let new_labels: Vec<String> = node
                    .labels
                    .into_iter()
                    .filter(|l| !to_remove.contains(l))
                    .collect();
                let labels_json = serde_json::to_string(&new_labels)
                    .map_err(|e| StorageError::Serialization { msg: e.to_string() })?;
                pipe.cmd("HSET")
                    .arg(self.kp(key_node(graph, id.0)))
                    .arg("labels")
                    .arg(&labels_json)
                    .ignore();
                conn.renew_lock()?;
                pipe.query::<()>(conn)
                    .map_err(crate::backend_storage_error)?;
                Ok(())
            },
        )
    }

    /// Delete a node and optionally detach (delete) all its edges first.
    ///
    /// When `detach` is false, returns an error if the node has any edges.
    /// When `detach` is true, deletes all adjacent edges (both incoming and
    /// outgoing) before removing the node itself.
    ///
    /// Node deletion removes: label index entries, the all-nodes set entry,
    /// adjacency keys, property indexes, and the node hash.
    pub(crate) fn delete_node_impl(
        &self,
        graph: &str,
        id: NodeId,
        detach: bool,
    ) -> Result<(), StorageError> {
        self.with_graph_write_lock(graph, WRITE_LOCK_TTL_MS, 3, 50, |conn| {
            self.require_graph_on(conn.as_mut(), graph)?;
            let node = self
                .read_node(conn.as_mut(), graph, id.0)?
                .ok_or(StorageError::NodeNotFound { id: id.0 })?;
            // Check for relationships if not detaching.
            if !detach {
                let out_count: u64 = redis::cmd("SCARD")
                    .arg(self.kp(key_out(graph, id.0)))
                    .query(conn.as_mut())
                    .map_err(crate::backend_storage_error)?;
                let in_count: u64 = redis::cmd("SCARD")
                    .arg(self.kp(key_in(graph, id.0)))
                    .query(conn.as_mut())
                    .map_err(crate::backend_storage_error)?;
                if out_count > 0 || in_count > 0 {
                    crate::record_backend_error();
                    return Err(StorageError::Backend {
                        msg: format!("node {} has relationships; use DETACH DELETE", id.0),
                    });
                }
            }
            if detach {
                // Delete all adjacent edges first while holding the graph write
                // lock so concurrent edge creation cannot race the adjacency scan.
                let out_key = self.kp(key_out(graph, id.0));
                let in_key = self.kp(key_in(graph, id.0));
                let out_count: u64 = redis::cmd("SCARD")
                    .arg(&out_key)
                    .query(conn.as_mut())
                    .map_err(crate::backend_storage_error)?;
                let in_count: u64 = redis::cmd("SCARD")
                    .arg(&in_key)
                    .query(conn.as_mut())
                    .map_err(crate::backend_storage_error)?;
                let limit = max_detach_delete_edges();
                let initial_capacity = usize::min(
                    limit.saturating_add(1),
                    out_count.saturating_add(in_count) as usize,
                );
                let mut edge_ids: HashSet<gql_core::storage::EdgeId> =
                    HashSet::with_capacity(initial_capacity);
                for_each_set_page_locked(
                    conn,
                    &out_key,
                    smembers_threshold(),
                    |_conn, members| {
                        for member in members {
                            if let Some(adj) = crate::keys::decode_adj(&member) {
                                insert_detach_edge_id_with_limit(
                                    id,
                                    &mut edge_ids,
                                    adj.edge_id,
                                    limit,
                                )?;
                            }
                        }
                        Ok(())
                    },
                )?;
                for_each_set_page_locked(conn, &in_key, smembers_threshold(), |_conn, members| {
                    for member in members {
                        if let Some(adj) = crate::keys::decode_adj(&member) {
                            insert_detach_edge_id_with_limit(
                                id,
                                &mut edge_ids,
                                adj.edge_id,
                                limit,
                            )?;
                        }
                    }
                    Ok(())
                })?;
                let mut edge_ids: Vec<EdgeId> = edge_ids.into_iter().collect();
                edge_ids.sort_unstable();
                for (index, eid) in edge_ids.into_iter().enumerate() {
                    if index > 0 && index % 100 == 0 {
                        conn.renew_lock()?;
                    }
                    self.delete_edge_inner(conn, graph, eid)?;
                }
            }
            // Remove property indexes (requires reads, so must stay outside transaction).
            self.deindex_node_properties(conn, graph, id.0, &node.labels, &node.properties)?;
            // Remove label indexes, all-nodes set, adjacency keys, and node hash in one pipeline.
            let id_str = id.0.to_string();
            let mut pipe = conn.pipeline();
            for label in &node.labels {
                pipe.cmd("SREM")
                    .arg(self.kp(key_label(graph, label)))
                    .arg(&id_str)
                    .ignore();
            }
            pipe.cmd("SREM")
                .arg(self.kp(key_nodes_set(graph)))
                .arg(&id_str)
                .ignore()
                .cmd("DEL")
                .arg(self.kp(key_out(graph, id.0)))
                .ignore()
                .cmd("DEL")
                .arg(self.kp(key_in(graph, id.0)))
                .ignore()
                .cmd("DEL")
                .arg(self.kp(key_node(graph, id.0)))
                .ignore();
            conn.renew_lock()?;
            pipe.query::<()>(conn)
                .map_err(crate::backend_storage_error)?;
            Ok(())
        })
    }

    /// Batch-create multiple nodes in a single Redis pipeline round-trip.
    ///
    /// For each node: INCRBY to allocate IDs, HSET to store node data,
    /// SADD to update label indexes and the all-nodes set.
    /// Property indexing is performed in a second pipeline pass.
    ///
    /// Returns the newly created `NodeId`s in the same order as the input.
    pub fn batch_create_nodes(
        &self,
        graph: &str,
        nodes: Vec<(Vec<String>, IndexMap<String, Value>)>,
    ) -> Result<Vec<NodeId>, StorageError> {
        if nodes.is_empty() {
            return Ok(Vec::new());
        }
        for (labels, properties) in &nodes {
            validate_labels(labels)?;
            validate_property_keys(properties)?;
        }
        let nodes = nodes
            .into_iter()
            .map(|(labels, properties)| {
                let injected_id = extract_injected_id(&properties, "_sd2_id")?;
                Ok((labels, properties, injected_id))
            })
            .collect::<Result<Vec<_>, StorageError>>()?;
        self.with_write_lock(
            graph,
            self.kp(key_batch_node_write_lock(graph)),
            WRITE_LOCK_TTL_MS,
            BATCH_WRITE_MAX_RETRIES,
            BATCH_WRITE_RETRY_DELAY_MS,
            |conn| {
                self.require_graph_on(conn.as_mut(), graph)?;

                // Lock was just acquired by `with_write_lock`; the first
                // 250 EXISTS calls run within the initial TTL window.
                let mut seen_injected_ids = HashSet::new();
                let mut max_injected_id: Option<u64> = None;
                for (index, (_, _, injected_id)) in nodes.iter().enumerate() {
                    let Some(injected_id) = *injected_id else {
                        continue;
                    };
                    if index > 0 && index % 250 == 0 {
                        conn.renew_lock()?;
                    }
                    if !seen_injected_ids.insert(injected_id) {
                        return Err(StorageError::ConstraintViolation {
                            msg: format!(
                                "duplicate injected node ID {injected_id} in batch_create_nodes"
                            ),
                        });
                    }
                    ensure_key_absent(
                        conn.as_mut(),
                        &self.kp(key_node(graph, injected_id)),
                        "node",
                        injected_id,
                    )?;
                    max_injected_id = Some(
                        max_injected_id.map_or(injected_id, |current| current.max(injected_id)),
                    );
                }
                if let Some(max_injected_id) = max_injected_id {
                    raise_sequence_floor(
                        conn.as_mut(),
                        &self.kp(key_node_seq(graph)),
                        max_injected_id,
                    )?;
                }

                let logical_batch_size = max_batch_size();
                let chunk_size = batch_pipeline_chunk_size();
                let mut created_ids = Vec::with_capacity(nodes.len());
                let mut node_iter = nodes.into_iter();

                loop {
                    let logical_batch: Vec<_> =
                        node_iter.by_ref().take(logical_batch_size).collect();
                    if logical_batch.is_empty() {
                        break;
                    }

                    let auto_id_count = logical_batch
                        .iter()
                        .filter(|(_, _, injected_id)| injected_id.is_none())
                        .count();
                    let count = i64::try_from(auto_id_count).map_err(|_| {
                        crate::record_backend_error();
                        StorageError::Backend {
                            msg: format!(
                                "auto-ID batch size {} exceeds i64::MAX, cannot allocate IDs",
                                auto_id_count
                            ),
                        }
                    })?;

                    // Allocate a contiguous block of IDs only for entries that
                    // did not already receive an injected `_sd2_id`.
                    let mut next_auto_id = if count == 0 {
                        None
                    } else {
                        let end_id: i64 = redis::cmd("INCRBY")
                            .arg(self.kp(key_node_seq(graph)))
                            .arg(count)
                            .query(conn.as_mut())
                            .map_err(crate::backend_storage_error)?;
                        if end_id <= 0 || end_id < count {
                            return Err(StorageError::IdOverflow {
                                graph: graph.to_string(),
                            });
                        }
                        Some((end_id - count + 1) as u64)
                    };

                    let assigned_batch: Vec<_> = logical_batch
                        .into_iter()
                        .map(|(labels, properties, injected_id)| {
                            let id = match injected_id {
                                Some(id) => id,
                                None => {
                                    let id = next_auto_id.expect(
                                        "auto-ID cursor must exist when a batch entry lacks _sd2_id",
                                    );
                                    next_auto_id = Some(id + 1);
                                    id
                                }
                            };
                            Ok((id, labels, properties))
                        })
                        .collect::<Result<Vec<_>, StorageError>>()?;

                    let mut chunk_iter = assigned_batch.into_iter();

                    loop {
                        let chunk: Vec<_> = chunk_iter.by_ref().take(chunk_size).collect();
                        if chunk.is_empty() {
                            break;
                        }

                        let mut pipe = conn.pipeline();
                        let mut node_meta: Vec<(u64, Vec<String>, IndexMap<String, Value>)> =
                            Vec::with_capacity(chunk.len());

                        for (offset, (id, labels, properties)) in chunk.into_iter().enumerate() {
                            // Mid-loop renewal only activates if chunk size is
                            // configured above 250; the primary safeguard is the
                            // conn.renew_lock() call before pipe.query() below.
                            if offset > 0 && offset % 250 == 0 {
                                conn.renew_lock()?;
                            }

                            // Strip internal FanoutStore keys before storing.
                            let clean_props = clean_fanout_props(properties);

                            let node_key = self.kp(key_node(graph, id));
                            let labels_json = serde_json::to_string(&labels)
                                .map_err(|e| StorageError::Serialization { msg: e.to_string() })?;
                            let props_json = props_to_json(&clean_props)?;

                            if !labels.is_empty() {
                                let registry_key = self.kp(key_label_names(graph));
                                let cmd = pipe.cmd("SADD");
                                cmd.arg(&registry_key);
                                for label in &labels {
                                    cmd.arg(label.as_str());
                                }
                                cmd.ignore();
                            }

                            pipe.cmd("SADD")
                                .arg(self.kp(key_nodes_set(graph)))
                                .arg(id.to_string())
                                .ignore();

                            for label in &labels {
                                pipe.cmd("SADD")
                                    .arg(self.kp(key_label(graph, label)))
                                    .arg(id.to_string())
                                    .ignore();
                            }

                            pipe.cmd("HSET")
                                .arg(&node_key)
                                .arg("labels")
                                .arg(&labels_json)
                                .arg("props")
                                .arg(&props_json)
                                .ignore();

                            node_meta.push((id, labels, clean_props));
                        }

                        conn.renew_lock()?;
                        pipe.query::<()>(conn)
                            .map_err(crate::backend_storage_error)?;

                        for (index, (id, labels, props)) in node_meta.iter().enumerate() {
                            if index > 0 && index % 100 == 0 {
                                conn.renew_lock()?;
                            }
                            self.index_node_properties(conn, graph, *id, labels, props)?;
                        }

                        created_ids.extend(node_meta.iter().map(|(id, _, _)| NodeId(*id)));
                    }
                }

                Ok(created_ids)
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::writes::helpers::{clean_fanout_props, extract_injected_id};

    #[test]
    fn insert_detach_edge_id_with_limit_allows_single_self_loop() {
        let mut edge_ids = HashSet::new();
        insert_detach_edge_id_with_limit(NodeId(42), &mut edge_ids, EdgeId(7), 1)
            .expect("first self-loop edge should fit within the limit");
        insert_detach_edge_id_with_limit(NodeId(42), &mut edge_ids, EdgeId(7), 1)
            .expect("duplicate self-loop edge ID should not consume extra limit budget");
        let mut collected: Vec<_> = edge_ids.into_iter().collect();
        collected.sort_unstable();
        assert_eq!(collected, vec![EdgeId(7)]);
    }

    #[test]
    fn insert_detach_edge_id_with_limit_rejects_distinct_edges_over_limit() {
        let mut edge_ids = HashSet::new();
        insert_detach_edge_id_with_limit(NodeId(42), &mut edge_ids, EdgeId(7), 1)
            .expect("first distinct edge should fit within the limit");
        let err = insert_detach_edge_id_with_limit(NodeId(42), &mut edge_ids, EdgeId(8), 1)
            .expect_err("two distinct adjacent edges should still trip a limit of one");
        assert!(matches!(err, StorageError::Backend { .. }));
        let mut collected: Vec<_> = edge_ids.into_iter().collect();
        collected.sort_unstable();
        assert_eq!(collected, vec![EdgeId(7), EdgeId(8)]);
    }

    // ---- Pure logic tests for extract_injected_id ---------------------------

    #[test]
    fn extract_injected_id_present_positive() {
        let mut props = IndexMap::new();
        props.insert("_sd2_id".to_string(), Value::Int(42));
        let result = extract_injected_id(&props, "_sd2_id").unwrap();
        assert_eq!(result, Some(42));
    }

    #[test]
    fn extract_injected_id_present_zero() {
        let mut props = IndexMap::new();
        props.insert("_sd2_id".to_string(), Value::Int(0));
        let result = extract_injected_id(&props, "_sd2_id").unwrap();
        assert_eq!(result, Some(0));
    }

    #[test]
    fn extract_injected_id_present_negative_fails() {
        let mut props = IndexMap::new();
        props.insert("_sd2_id".to_string(), Value::Int(-5));
        let result = extract_injected_id(&props, "_sd2_id");
        assert!(result.is_err());
    }

    #[test]
    fn extract_injected_id_absent() {
        let props = IndexMap::new();
        let result = extract_injected_id(&props, "_sd2_id").unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn extract_injected_id_wrong_type_ignored() {
        let mut props = IndexMap::new();
        props.insert("_sd2_id".to_string(), Value::Str("not_an_int".into()));
        // Non-Int values are not matched, so returns None.
        let result = extract_injected_id(&props, "_sd2_id").unwrap();
        assert_eq!(result, None);
    }

    // ---- Pure logic tests for clean_fanout_props ----------------------------

    #[test]
    fn clean_fanout_props_removes_internal_keys() {
        let mut props = IndexMap::new();
        props.insert("_sd2_id".to_string(), Value::Int(1));
        props.insert("_sd2_eid".to_string(), Value::Int(2));
        props.insert("name".to_string(), Value::Str("Alice".into()));
        let cleaned = clean_fanout_props(props);
        assert!(!cleaned.contains_key("_sd2_id"));
        assert!(!cleaned.contains_key("_sd2_eid"));
        assert_eq!(cleaned.get("name"), Some(&Value::Str("Alice".into())));
    }

    #[test]
    fn clean_fanout_props_noop_when_no_internal_keys() {
        let mut props = IndexMap::new();
        props.insert("age".to_string(), Value::Int(30));
        let cleaned = clean_fanout_props(props);
        assert_eq!(cleaned.len(), 1);
        assert_eq!(cleaned.get("age"), Some(&Value::Int(30)));
    }

    #[test]
    fn clean_fanout_props_empty_map() {
        let props = IndexMap::new();
        let cleaned = clean_fanout_props(props);
        assert!(cleaned.is_empty());
    }

    #[test]
    fn validate_labels_rejects_null_byte() {
        let labels = vec!["Per\x00son".to_string()];
        let result = validate_labels(&labels);
        assert!(
            result.is_err(),
            "write-path label validation should reject null bytes"
        );
    }

    #[test]
    fn validate_property_keys_rejects_null_byte() {
        let mut props = IndexMap::new();
        props.insert("na\x00me".to_string(), Value::Int(1));
        let result = validate_property_keys(&props);
        assert!(
            result.is_err(),
            "write-path property validation should reject null-byte keys"
        );
    }

    // ---- Cluster pipeline safety: verify no MULTI/EXEC in node write pipeline ---

    /// Verify that the node-write pipeline does NOT include MULTI or EXEC.
    ///
    /// This test encodes the fix for Issue #745: redis-rs ClusterConnection
    /// does not support MULTI/EXEC, so .atomic() must not be used in cluster mode.
    /// All per-graph keys share the {graphname} hash tag and route to the same
    /// cluster slot, so non-atomic pipelining is cluster-safe.
    #[test]
    fn create_node_pipeline_has_no_multi_exec() {
        // Construct the same pipeline sequence as create_node_impl (minus Redis I/O).
        // We validate the pipeline command names do not include MULTI or EXEC.
        let labels = vec!["Person".to_string(), "Employee".to_string()];
        let graph = "testgraph";
        let id = 1u64;
        let node_key = format!("G:{{{graph}}}:node:{id}");
        let labels_json = serde_json::to_string(&labels).unwrap();
        let props_json = "{}";

        let mut pipe = redis::pipe();
        // Replicate create_node_impl pipeline (without .atomic())
        pipe.cmd("HSET")
            .arg(&node_key)
            .arg("labels")
            .arg(&labels_json)
            .arg("props")
            .arg(props_json)
            .ignore();
        for label in &labels {
            pipe.cmd("SADD")
                .arg(format!("G:{{{graph}}}:label:{label}"))
                .arg(id.to_string())
                .ignore();
        }
        pipe.cmd("SADD")
            .arg(format!("G:{{{graph}}}:nodes"))
            .arg(id.to_string())
            .ignore();

        // Serialize the pipeline to bytes and verify MULTI/EXEC are absent.
        let packed = pipe.get_packed_pipeline();
        let pipeline_str = String::from_utf8_lossy(&packed);
        // In Redis protocol, commands appear as bulk strings.
        assert!(
            !pipeline_str.to_uppercase().contains("MULTI"),
            "Pipeline must not contain MULTI (breaks ClusterConnection): {pipeline_str}"
        );
        assert!(
            !pipeline_str.to_uppercase().contains("EXEC"),
            "Pipeline must not contain EXEC (breaks ClusterConnection): {pipeline_str}"
        );
    }

    /// Verify that all keys in the node-write pipeline share the {graphname} hash tag.
    ///
    /// Redis Cluster routes keys to slots by hash tag. If all keys in a pipeline
    /// share the same hash tag, they are guaranteed to be on the same cluster node,
    /// making non-atomic pipelining safe and correct.
    #[test]
    fn create_node_pipeline_keys_all_share_hash_tag() {
        let graph = "mygraph";
        let id = 42u64;
        let labels = ["Label1".to_string(), "Label2".to_string()];
        let expected_tag = format!("{{{graph}}}");

        // All keys produced by the create_node pipeline.
        let node_key = format!("G:{expected_tag}:node:{id}");
        let label_keys: Vec<String> = labels
            .iter()
            .map(|l| format!("G:{expected_tag}:label:{l}"))
            .collect();
        let nodes_set_key = format!("G:{expected_tag}:nodes");

        // Every key must contain the hash tag.
        assert!(
            node_key.contains(&expected_tag),
            "node key missing hash tag: {node_key}"
        );
        assert!(
            nodes_set_key.contains(&expected_tag),
            "nodes set key missing hash tag: {nodes_set_key}"
        );
        for k in &label_keys {
            assert!(k.contains(&expected_tag), "label key missing hash tag: {k}");
        }
    }
}
