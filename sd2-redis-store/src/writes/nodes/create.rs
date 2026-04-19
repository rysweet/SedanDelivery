// writes/nodes/create.rs -- Single-node creation for RedisGraphStore.

use indexmap::IndexMap;

use cypher_core::error::StorageError;
use cypher_core::serde_value::props_to_json;
use cypher_core::storage::NodeId;
use cypher_core::types::Value;

use crate::keys::{key_label, key_label_names, key_node, key_node_seq, key_nodes_set};
use crate::lock::WRITE_LOCK_TTL_MS;
use crate::writes::helpers::{
    clean_fanout_props, ensure_key_absent, extract_injected_id, raise_sequence_floor,
};
use crate::RedisGraphStore;

use super::{validate_labels, validate_property_keys};

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
}
