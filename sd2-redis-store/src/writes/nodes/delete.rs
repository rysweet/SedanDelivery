// writes/nodes/delete.rs -- Single-node deletion with optional DETACH.

use std::collections::HashSet;

use cypher_core::error::StorageError;
use cypher_core::storage::{EdgeId, NodeId};

use crate::keys::{key_graph_lock_fence, key_in, key_label, key_node, key_nodes_set, key_out};
use crate::lock::WRITE_LOCK_TTL_MS;
use crate::reads::{for_each_set_page_locked, smembers_threshold};
use crate::writes::lua_atomic::{map_script_error, NODE_DELETE_SCRIPT};
use crate::RedisGraphStore;

use super::{insert_detach_edge_id_with_limit, max_detach_delete_edges};

fn node_delete_script() -> &'static redis::Script {
    static SCRIPT: std::sync::OnceLock<redis::Script> = std::sync::OnceLock::new();
    SCRIPT.get_or_init(|| redis::Script::new(NODE_DELETE_SCRIPT))
}

impl RedisGraphStore {
    /// Delete a node and optionally detach (delete) all its edges first.
    ///
    /// When `detach` is false, returns an error if the node has any edges.
    /// When `detach` is true, deletes all adjacent edges (both incoming and
    /// outgoing) before removing the node itself.
    ///
    /// The final teardown — label-index SREMs, all-nodes SREM, adjacency
    /// DELs, and node-hash DEL — runs inside a single slot-local Lua
    /// script gated on the per-graph fence token (F1+F2 in the cluster
    /// consistency hardening package, issue #1769). A concurrent reader
    /// can never observe a node hash whose label-index entries have been
    /// half-cleared, and a stalled writer whose lease was stolen fails
    /// closed with `LockFenced`.
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
                let mut edge_ids: HashSet<cypher_core::storage::EdgeId> =
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
            // Remove property indexes (requires reads, so must stay outside the script).
            self.deindex_node_properties(conn, graph, id.0, &node.labels, &node.properties)?;

            // Slot-local Lua teardown of label-index, all-nodes, adjacency, and node hash.
            let fence_str = conn.fence().to_string();
            let mut keys: Vec<String> = Vec::with_capacity(5 + node.labels.len());
            keys.push(self.kp(key_graph_lock_fence(graph)));
            keys.push(self.kp(key_node(graph, id.0)));
            keys.push(self.kp(key_nodes_set(graph)));
            keys.push(self.kp(key_out(graph, id.0)));
            keys.push(self.kp(key_in(graph, id.0)));
            for label in &node.labels {
                keys.push(self.kp(key_label(graph, label)));
            }
            let args = [fence_str, id.0.to_string()];

            conn.renew_lock()?;
            let mut invocation = node_delete_script().prepare_invoke();
            for k in &keys {
                invocation.key(k.as_str());
            }
            for a in &args {
                invocation.arg(a.as_str());
            }
            let _: i64 = invocation
                .invoke(conn.as_mut())
                .map_err(|e| map_script_error(graph, e))?;
            Ok(())
        })
    }
}
