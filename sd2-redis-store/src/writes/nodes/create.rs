// writes/nodes/create.rs -- Single-node creation for RedisGraphStore.

use indexmap::IndexMap;

use cypher_core::error::StorageError;
use cypher_core::serde_value::props_to_json;
use cypher_core::storage::NodeId;
use cypher_core::types::Value;

use crate::keys::{
    key_graph_lock_fence, key_label, key_label_names, key_node, key_node_seq, key_nodes_set,
};
use crate::lock::WRITE_LOCK_TTL_MS;
use crate::writes::helpers::{clean_fanout_props, extract_injected_id};
use crate::writes::lua_atomic::{run_hot_write_script, NODE_CREATE_SCRIPT};
use crate::RedisGraphStore;

use super::{validate_labels, validate_property_keys};

fn node_create_script() -> &'static redis::Script {
    static SCRIPT: std::sync::OnceLock<redis::Script> = std::sync::OnceLock::new();
    SCRIPT.get_or_init(|| redis::Script::new(NODE_CREATE_SCRIPT))
}

impl RedisGraphStore {
    /// Create a single node in the given graph.
    ///
    /// All Redis state changes for this node — id allocation, all-nodes
    /// membership, label-registry membership, label-index membership, and
    /// the node hash — execute inside a single slot-local Lua script
    /// (F2 in the cluster consistency hardening package, issue #1769).
    /// The script's first action validates the per-graph fence token (F1),
    /// so a stalled writer whose lease has been stolen fails closed with
    /// [`StorageError::LockFenced`] instead of landing a partial late write.
    ///
    /// Property indexing happens after the script returns, while the writer
    /// still holds the per-graph write lock.
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

            let injected = extract_injected_id(&properties, "_sd2_id")?;
            let clean_props = clean_fanout_props(properties);

            let labels_json = serde_json::to_string(&labels)
                .map_err(|e| StorageError::Serialization { msg: e.to_string() })?;
            let props_json = props_to_json(&clean_props)?;

            // Allocate the id under the lock before invoking the Lua script.
            // Doing this in Rust keeps the script's KEYS list fully known up
            // front (Redis Cluster routes scripts by their declared keys, so
            // every hash we touch — including `node:{id}` — must be present
            // in `KEYS` from the moment the EVAL is dispatched).
            let id = match injected {
                Some(id) => id,
                None => {
                    let raw_id: i64 = redis::cmd("INCRBY")
                        .arg(self.kp(key_node_seq(graph)))
                        .arg(1)
                        .query(conn.as_mut())
                        .map_err(crate::backend_storage_error)?;
                    if raw_id <= 0 {
                        return Err(StorageError::IdOverflow {
                            graph: graph.to_string(),
                        });
                    }
                    raw_id as u64
                }
            };

            let fence = conn.fence().to_string();

            let fence_key = self.kp(key_graph_lock_fence(graph));
            let node_key = self.kp(key_node(graph, id));
            let node_seq_key = self.kp(key_node_seq(graph));
            let nodes_set_key = self.kp(key_nodes_set(graph));
            let label_names_key = self.kp(key_label_names(graph));

            let mut keys: Vec<String> = Vec::with_capacity(5 + labels.len());
            keys.push(fence_key);
            keys.push(node_key);
            keys.push(node_seq_key);
            keys.push(nodes_set_key);
            keys.push(label_names_key);
            for label in &labels {
                keys.push(self.kp(key_label(graph, label)));
            }

            let mut args: Vec<String> = Vec::with_capacity(5 + labels.len());
            args.push(fence);
            // The id is already allocated above (either caller-injected or
            // INCR'd under the lock). Pass it through the script's injected-id
            // slot so the script does not bump the sequence again. The
            // collision check in the script is a safety net for the injected
            // path; for sequence-allocated ids it cannot match an existing
            // hash because INCR returned a fresh id under the write lock.
            args.push(id.to_string());
            args.push(labels_json);
            args.push(props_json);
            args.push(labels.len().to_string());
            for label in &labels {
                args.push(label.clone());
            }

            conn.renew_lock()?;
            let returned_id: i64 =
                run_hot_write_script(node_create_script(), conn.as_mut(), graph, &keys, &args)?;
            if returned_id == 0 {
                return Err(StorageError::ConstraintViolation {
                    msg: format!("node id collision on injected id {id}"),
                });
            }
            if returned_id < 0 || (returned_id as u64) != id {
                return Err(StorageError::Backend {
                    msg: format!(
                        "node-create script returned unexpected id {returned_id} (expected {id})"
                    ),
                });
            }

            // Maintain property indexes outside of the Lua script — they
            // require key formatting that depends on the actual values and
            // would balloon the script's KEYS list.
            self.index_node_properties(conn, graph, id, &labels, &clean_props)?;
            Ok(NodeId(id))
        })
    }
}
