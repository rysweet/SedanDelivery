// writes/edges/delete.rs -- Single-edge delete for RedisGraphStore.

use cypher_core::error::StorageError;
use cypher_core::storage::EdgeId;

use crate::keys::{
    encode_adj, key_edge, key_edge_id_write_lock, key_edges_set, key_graph_lock_fence, key_in,
    key_out, key_reltype,
};
use crate::lock::WRITE_LOCK_TTL_MS;
use crate::writes::lua_atomic::{map_script_error, EDGE_DELETE_SCRIPT};
use crate::{LockedRedisConn, RedisGraphStore};

fn edge_delete_script() -> &'static redis::Script {
    static SCRIPT: std::sync::OnceLock<redis::Script> = std::sync::OnceLock::new();
    SCRIPT.get_or_init(|| redis::Script::new(EDGE_DELETE_SCRIPT))
}

impl RedisGraphStore {
    /// Delete an edge using an already-locked connection.
    ///
    /// This avoids dropping and re-acquiring the connection lock when called
    /// from `delete_node(detach=true)`, which needs to delete all adjacent
    /// edges while holding a single connection.
    ///
    /// Removes the edge hash, the reltype index entry, the all-edges set
    /// entry, the out-adjacency entry on the source node, and the
    /// in-adjacency entry on the destination node — all inside a single
    /// slot-local Lua script gated on the per-graph fence token. A
    /// concurrent reader can never observe an edge hash without its
    /// reverse adjacency, and a stalled writer whose lease was stolen
    /// fails closed with `LockFenced`.
    ///
    /// Note: the reltype-name registry (`G:{g}:_reltypes`) is intentionally
    /// NOT pruned here — that is a multi-step operation (check whether any
    /// other edge of the same reltype still exists) covered by the audit
    /// pass, mirroring the pre-existing behavior of the prior pipeline path.
    pub(crate) fn delete_edge_inner(
        &self,
        conn: &mut LockedRedisConn,
        graph: &str,
        id: EdgeId,
    ) -> Result<(), StorageError> {
        let edge = self
            .read_edge(conn.as_mut(), graph, id.0)?
            .ok_or(StorageError::EdgeNotFound { id: id.0 })?;
        let out_member = encode_adj(id, edge.dst, &edge.rel_type);
        let in_member = encode_adj(id, edge.src, &edge.rel_type);

        let fence_str = conn.fence().to_string();
        let keys = [
            self.kp(key_graph_lock_fence(graph)),
            self.kp(key_edge(graph, id.0)),
            self.kp(key_edges_set(graph)),
            self.kp(key_reltype(graph, &edge.rel_type)),
            self.kp(key_out(graph, edge.src.0)),
            self.kp(key_in(graph, edge.dst.0)),
        ];
        let args = [fence_str, id.0.to_string(), out_member, in_member];

        conn.renew_lock()?;
        let mut invocation = edge_delete_script().prepare_invoke();
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
