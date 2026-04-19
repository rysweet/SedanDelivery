// writes/nodes/batch.rs -- Batch node creation pipelining.

use std::collections::HashSet;

use indexmap::IndexMap;

use cypher_core::error::StorageError;
use cypher_core::serde_value::props_to_json;
use cypher_core::storage::NodeId;
use cypher_core::types::Value;

use crate::keys::{
    key_batch_node_write_lock, key_label, key_label_names, key_node, key_node_seq, key_nodes_set,
};
use crate::lock::{BATCH_WRITE_MAX_RETRIES, BATCH_WRITE_RETRY_DELAY_MS, WRITE_LOCK_TTL_MS};
use crate::writes::helpers::{
    batch_pipeline_chunk_size, clean_fanout_props, ensure_key_absent, extract_injected_id,
    max_batch_size, raise_sequence_floor,
};
use crate::RedisGraphStore;

use super::{validate_labels, validate_property_keys};

impl RedisGraphStore {
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
