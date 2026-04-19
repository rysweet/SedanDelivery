// writes/edges/delete.rs -- Single-edge delete for RedisGraphStore.

use cypher_core::error::StorageError;
use cypher_core::storage::EdgeId;

use crate::keys::{
    encode_adj, key_edge, key_edge_id_write_lock, key_edges_set, key_in, key_out, key_reltype,
};
use crate::lock::WRITE_LOCK_TTL_MS;
use crate::{LockedRedisConn, RedisGraphStore};

impl RedisGraphStore {
    /// Delete an edge using an already-locked connection.
    ///
    /// This avoids dropping and re-acquiring the connection lock when called
    /// from `delete_node(detach=true)`, which needs to delete all adjacent
    /// edges while holding a single connection.
    ///
    /// Removes the edge hash, the reltype index entry, the reltype registry
    /// entry, the out-adjacency entry on the source node, and the in-adjacency
    /// entry on the destination node in a single slot-local pipeline.
    pub(crate) fn delete_edge_inner(
        &self,
        conn: &mut LockedRedisConn,
        graph: &str,
        id: EdgeId,
    ) -> Result<(), StorageError> {
        let edge = self
            .read_edge(conn.as_mut(), graph, id.0)?
            .ok_or(StorageError::EdgeNotFound { id: id.0 })?;
        // Remove all edge data in a single pipeline. MULTI/EXEC (.atomic()) is
        // intentionally omitted — redis-rs ClusterConnection does not support it.
        // All keys share the {graphname} hash tag (same cluster slot). Callers
        // hold the write lock for serialisation. See ADR-005.
        if conn.is_standalone() {
            tracing::warn!(
                graph = graph,
                edge_id = id.0,
                "delete_edge pipeline executing without MULTI/EXEC atomicity (standalone mode); \
                 a partial delete is possible if the process crashes mid-pipeline. See ADR-005."
            );
        }
        let out_member = encode_adj(id, edge.dst, &edge.rel_type);
        let in_member = encode_adj(id, edge.src, &edge.rel_type);
        let mut pipe = conn.pipeline();
        pipe.cmd("SREM")
            .arg(self.kp(key_reltype(graph, &edge.rel_type)))
            .arg(id.0.to_string())
            .ignore()
            .cmd("SREM")
            .arg(self.kp(key_edges_set(graph)))
            .arg(id.0.to_string())
            .ignore()
            .cmd("SREM")
            .arg(self.kp(key_out(graph, edge.src.0)))
            .arg(out_member)
            .ignore()
            .cmd("SREM")
            .arg(self.kp(key_in(graph, edge.dst.0)))
            .arg(in_member)
            .ignore()
            .cmd("DEL")
            .arg(self.kp(key_edge(graph, id.0)))
            .ignore();
        conn.renew_lock()?;
        pipe.query::<()>(conn)
            .map_err(crate::backend_storage_error)?;
        Ok(())
    }

    /// Delete an edge by ID (public entry point that acquires its own connection).
    pub(crate) fn delete_edge_impl(&self, graph: &str, id: EdgeId) -> Result<(), StorageError> {
        self.with_write_lock(
            graph,
            self.kp(key_edge_id_write_lock(graph, id.0)),
            WRITE_LOCK_TTL_MS,
            3,
            50,
            |conn| {
                self.require_graph_on(conn.as_mut(), graph)?;
                self.delete_edge_inner(conn, graph, id)
            },
        )
    }
}
