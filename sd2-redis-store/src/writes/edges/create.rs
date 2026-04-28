// writes/edges/create.rs -- Single-edge creation for RedisGraphStore.

use indexmap::IndexMap;

use cypher_core::error::StorageError;
use cypher_core::serde_value::props_to_json;
use cypher_core::storage::{EdgeId, NodeId};
use cypher_core::types::Value;

use crate::keys::{
    encode_adj, key_edge, key_edge_seq, key_edge_write_lock, key_edges_set, key_graph_lock_fence,
    key_in, key_out, key_reltype, key_reltype_names,
};
use crate::lock::WRITE_LOCK_TTL_MS;
use crate::writes::helpers::{clean_fanout_props, extract_injected_id};
use crate::writes::lua_atomic::{run_hot_write_script, EDGE_CREATE_SCRIPT};
use crate::{LockedRedisConn, RedisGraphStore};

use super::validate_property_keys;

fn edge_create_script() -> &'static redis::Script {
    static SCRIPT: std::sync::OnceLock<redis::Script> = std::sync::OnceLock::new();
    SCRIPT.get_or_init(|| redis::Script::new(EDGE_CREATE_SCRIPT))
}

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

        let injected = extract_injected_id(&properties, "_sd2_eid")?;
        let id = match injected {
            Some(id) => id,
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
        let props_json = props_to_json(&clean_props)?;
        let out_member = encode_adj(EdgeId(id), dst, &rel_type);
        let in_member = encode_adj(EdgeId(id), src, &rel_type);

        let fence = conn.fence().to_string();

        let keys: Vec<String> = vec![
            self.kp(key_graph_lock_fence(graph)),
            self.kp(key_edge(graph, id)),
            self.kp(key_edge_seq(graph)),
            self.kp(key_edges_set(graph)),
            self.kp(key_reltype_names(graph)),
            self.kp(key_reltype(graph, &rel_type)),
            self.kp(key_out(graph, src.0)),
            self.kp(key_in(graph, dst.0)),
        ];
        let args: Vec<String> = vec![
            fence,
            id.to_string(),
            rel_type.clone(),
            src.0.to_string(),
            dst.0.to_string(),
            props_json,
            out_member,
            in_member,
        ];

        conn.renew_lock()?;
        let returned_id: i64 =
            run_hot_write_script(edge_create_script(), conn.as_mut(), graph, &keys, &args)?;
        if returned_id == 0 {
            return Err(StorageError::ConstraintViolation {
                msg: format!("edge id collision on injected id {id}"),
            });
        }
        if returned_id < 0 || (returned_id as u64) != id {
            return Err(StorageError::Backend {
                msg: format!(
                    "edge-create script returned unexpected id {returned_id} (expected {id})"
                ),
            });
        }
        Ok(EdgeId(id))
    }

    /// Create an edge between two existing nodes.
    ///
    /// Verifies that both source and destination nodes exist, allocates a new
    /// edge ID (or uses the injected `_sd2_eid` from FanoutStore), and stores
    /// the edge hash plus reltype index, reltype registry, out-adjacency,
    /// in-adjacency, and all-edges set in a single slot-local Lua script
    /// (F2 in the cluster consistency hardening package, issue #1769). The
    /// script's first action validates the per-graph fence token (F1) and
    /// fails closed with [`StorageError::LockFenced`] if the writer's lease
    /// has been stolen.
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
