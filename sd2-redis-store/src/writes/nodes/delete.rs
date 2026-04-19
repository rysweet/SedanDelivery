// writes/nodes/delete.rs -- Single-node deletion with optional DETACH.

use std::collections::HashSet;

use cypher_core::error::StorageError;
use cypher_core::storage::{EdgeId, NodeId};

use crate::keys::{key_in, key_label, key_node, key_nodes_set, key_out};
use crate::lock::WRITE_LOCK_TTL_MS;
use crate::reads::{for_each_set_page_locked, smembers_threshold};
use crate::RedisGraphStore;

use super::{insert_detach_edge_id_with_limit, max_detach_delete_edges};

impl RedisGraphStore {
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
}
