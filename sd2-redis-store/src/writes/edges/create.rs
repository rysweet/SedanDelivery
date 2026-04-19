// writes/edges/create.rs -- Single-edge creation for RedisGraphStore.

use indexmap::IndexMap;

use cypher_core::error::StorageError;
use cypher_core::serde_value::props_to_json;
use cypher_core::storage::{EdgeId, NodeId};
use cypher_core::types::Value;

use crate::keys::{
    encode_adj, key_edge, key_edge_seq, key_edge_write_lock, key_edges_set, key_in, key_out,
    key_reltype, key_reltype_names,
};
use crate::lock::WRITE_LOCK_TTL_MS;
use crate::writes::helpers::{
    clean_fanout_props, ensure_key_absent, extract_injected_id, raise_sequence_floor,
};
use crate::{LockedRedisConn, RedisGraphStore};

use super::validate_property_keys;

impl RedisGraphStore {
    fn create_edge_locked(
        &self,
        conn: &mut LockedRedisConn,
        graph: &str,
        src: NodeId,
        dst: NodeId,
        rel_type: String,
        properties: IndexMap<String, Value>,
    ) -> Result<EdgeId, StorageError> {
        self.require_graph_on(conn.as_mut(), graph)?;
        if self.read_node(conn.as_mut(), graph, src.0)?.is_none() {
            return Err(StorageError::NodeNotFound { id: src.0 });
        }
        if self.read_node(conn.as_mut(), graph, dst.0)?.is_none() {
            return Err(StorageError::NodeNotFound { id: dst.0 });
        }

        let id = match extract_injected_id(&properties, "_sd2_eid")? {
            Some(id) => {
                ensure_key_absent(conn.as_mut(), &self.kp(key_edge(graph, id)), "edge", id)?;
                raise_sequence_floor(conn.as_mut(), &self.kp(key_edge_seq(graph)), id)?;
                id
            }
            None => {
                let raw_id: i64 = redis::cmd("INCRBY")
                    .arg(self.kp(key_edge_seq(graph)))
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

        let clean_props = clean_fanout_props(properties);
        let edge_key = self.kp(key_edge(graph, id));
        let props_json = props_to_json(&clean_props)?;
        if conn.is_standalone() {
            tracing::warn!(
                graph = graph,
                "create_edge pipeline executing without MULTI/EXEC atomicity (standalone mode); \
                 a partial write is possible if the process crashes mid-pipeline. See ADR-005."
            );
        }
        let out_member = encode_adj(EdgeId(id), dst, &rel_type);
        let in_member = encode_adj(EdgeId(id), src, &rel_type);
        let mut pipe = conn.pipeline();
        pipe.cmd("SADD")
            .arg(self.kp(key_reltype_names(graph)))
            .arg(rel_type.as_str())
            .ignore()
            .cmd("SADD")
            .arg(self.kp(key_edges_set(graph)))
            .arg(id.to_string())
            .ignore()
            .cmd("SADD")
            .arg(self.kp(key_reltype(graph, &rel_type)))
            .arg(id.to_string())
            .ignore()
            .cmd("SADD")
            .arg(self.kp(key_out(graph, src.0)))
            .arg(out_member)
            .ignore()
            .cmd("SADD")
            .arg(self.kp(key_in(graph, dst.0)))
            .arg(in_member)
            .ignore()
            .cmd("HSET")
            .arg(&edge_key)
            .arg("src")
            .arg(src.0.to_string())
            .arg("dst")
            .arg(dst.0.to_string())
            .arg("rel_type")
            .arg(&rel_type)
            .arg("props")
            .arg(&props_json)
            .ignore();
        conn.renew_lock()?;
        pipe.query::<()>(conn)
            .map_err(crate::backend_storage_error)?;
        Ok(EdgeId(id))
    }

    /// Create an edge between two existing nodes.
    ///
    /// Verifies that both source and destination nodes exist, allocates a new
    /// edge ID (or uses the injected `_sd2_eid` from FanoutStore), stores the
    /// edge hash with src/dst/rel_type/props, and updates the reltype index,
    /// reltype registry, out-adjacency set, in-adjacency set, and all-edges
    /// set in a single slot-local pipeline.
    pub(crate) fn create_edge_impl(
        &self,
        graph: &str,
        src: NodeId,
        dst: NodeId,
        rel_type: String,
        properties: IndexMap<String, Value>,
    ) -> Result<EdgeId, StorageError> {
        validate_property_keys(&properties)?;
        self.with_write_lock(
            graph,
            self.kp(key_edge_write_lock(graph, src.0, dst.0, &rel_type)),
            WRITE_LOCK_TTL_MS,
            3,
            50,
            |conn| self.create_edge_locked(conn, graph, src, dst, rel_type, properties),
        )
    }
}
