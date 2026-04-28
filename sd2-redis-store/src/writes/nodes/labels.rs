// writes/nodes/labels.rs -- Label add/remove operations on existing nodes.
//
// All hot mutations on this path are performed via slot-local Lua scripts
// (F2 in the cluster consistency hardening package, issue #1769). The
// scripts validate the per-graph fence token (F1) before mutating, so a
// stalled writer whose lease was stolen fails closed with `LockFenced`
// instead of landing partial late writes.

use cypher_core::error::StorageError;
use cypher_core::storage::NodeId;

use crate::keys::{
    key_graph_lock_fence, key_label, key_label_names, key_node, key_node_write_lock,
};
use crate::lock::WRITE_LOCK_TTL_MS;
use crate::writes::lua_atomic::{
    map_script_error, NODE_LABELS_ADD_SCRIPT, NODE_LABELS_REMOVE_SCRIPT,
};
use crate::RedisGraphStore;

use super::validate_labels;

fn labels_add_script() -> &'static redis::Script {
    static SCRIPT: std::sync::OnceLock<redis::Script> = std::sync::OnceLock::new();
    SCRIPT.get_or_init(|| redis::Script::new(NODE_LABELS_ADD_SCRIPT))
}

fn labels_remove_script() -> &'static redis::Script {
    static SCRIPT: std::sync::OnceLock<redis::Script> = std::sync::OnceLock::new();
    SCRIPT.get_or_init(|| redis::Script::new(NODE_LABELS_REMOVE_SCRIPT))
}

/// Map `NODE_MISSING` from a labels Lua script to `NodeNotFound`. All other
/// errors (including `STALE_FENCE`) flow through `map_script_error`.
fn map_labels_error(graph: &str, id: u64, err: redis::RedisError) -> StorageError {
    if err.to_string().contains("NODE_MISSING") {
        return StorageError::NodeNotFound { id };
    }
    map_script_error(graph, err)
}

impl RedisGraphStore {
    /// Add labels to an existing node without removing any existing labels.
    ///
    /// Adds the node ID to each new label's index set, registers any
    /// previously-unseen labels in `_labels`, and writes the merged label
    /// list back to the node hash — all inside a single slot-local Lua
    /// script gated on the per-graph fence token. Property re-indexing
    /// runs in Rust afterwards while the writer still holds the lock.
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
                    node.labels.iter().cloned().collect();
                let mut added_labels: Vec<String> = Vec::new();
                for label in labels {
                    if existing.insert(label.clone()) {
                        added_labels.push(label.clone());
                    }
                }
                if added_labels.is_empty() {
                    // Nothing to do — fence-token check is intentionally
                    // skipped because there are no mutations to gate.
                    return Ok(());
                }

                let mut new_labels: Vec<String> = existing.into_iter().collect();
                new_labels.sort();
                let labels_json = serde_json::to_string(&new_labels)
                    .map_err(|e| StorageError::Serialization { msg: e.to_string() })?;

                let fence_str = conn.fence().to_string();
                let mut keys: Vec<String> = Vec::with_capacity(3 + added_labels.len());
                keys.push(self.kp(key_graph_lock_fence(graph)));
                keys.push(self.kp(key_node(graph, id.0)));
                keys.push(self.kp(key_label_names(graph)));
                for label in &added_labels {
                    keys.push(self.kp(key_label(graph, label)));
                }

                let mut args: Vec<String> = Vec::with_capacity(3 + added_labels.len());
                args.push(fence_str);
                args.push(id.0.to_string());
                args.push(labels_json);
                for label in &added_labels {
                    args.push(label.clone());
                }

                conn.renew_lock()?;
                let mut invocation = labels_add_script().prepare_invoke();
                for k in &keys {
                    invocation.key(k.as_str());
                }
                for a in &args {
                    invocation.arg(a.as_str());
                }
                let _: i64 = invocation
                    .invoke(conn.as_mut())
                    .map_err(|e| map_labels_error(graph, id.0, e))?;

                self.index_node_properties(conn, graph, id.0, &added_labels, &node.properties)?;
                Ok(())
            },
        )
    }

    /// Remove labels from an existing node.
    ///
    /// SREMs the node id from each removed label's index set and writes the
    /// filtered label list back to the node hash — all inside a single
    /// slot-local Lua script gated on the fence token. Property
    /// de-indexing runs in Rust beforehand while still under the lock.
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

                let new_labels: Vec<String> = node
                    .labels
                    .iter()
                    .filter(|l| !to_remove.contains(l))
                    .cloned()
                    .collect();
                let labels_json = serde_json::to_string(&new_labels)
                    .map_err(|e| StorageError::Serialization { msg: e.to_string() })?;

                let fence_str = conn.fence().to_string();
                let mut keys: Vec<String> = Vec::with_capacity(2 + removed_labels.len());
                keys.push(self.kp(key_graph_lock_fence(graph)));
                keys.push(self.kp(key_node(graph, id.0)));
                for label in &removed_labels {
                    keys.push(self.kp(key_label(graph, label)));
                }

                let args = [fence_str, id.0.to_string(), labels_json];

                conn.renew_lock()?;
                let mut invocation = labels_remove_script().prepare_invoke();
                for k in &keys {
                    invocation.key(k.as_str());
                }
                for a in &args {
                    invocation.arg(a.as_str());
                }
                let _: i64 = invocation
                    .invoke(conn.as_mut())
                    .map_err(|e| map_labels_error(graph, id.0, e))?;
                Ok(())
            },
        )
    }
}
