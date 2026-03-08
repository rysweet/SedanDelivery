// writes/edges.rs -- Edge write operations for RedisGraphStore.

use std::collections::HashMap;

use redis::ConnectionLike;

use gql_core::error::StorageError;
use gql_core::serde_value::props_to_json;
use gql_core::storage::{EdgeId, NodeId};
use gql_core::types::Value;

use crate::keys::{
    encode_adj, key_edge, key_edge_seq, key_edges_set, key_in, key_out, key_reltype,
};
use crate::RedisGraphStore;

use super::helpers::{clean_fanout_props, extract_injected_id};

impl RedisGraphStore {
    /// Delete an edge using an already-locked connection.
    ///
    /// This avoids dropping and re-acquiring the connection lock when called
    /// from `delete_node(detach=true)`, which needs to delete all adjacent
    /// edges while holding a single connection.
    ///
    /// Removes the edge hash, the reltype index entry, the out-adjacency
    /// entry on the source node, and the in-adjacency entry on the
    /// destination node, all atomically via MULTI/EXEC pipeline.
    pub(crate) fn delete_edge_inner(
        &self,
        conn: &mut dyn ConnectionLike,
        graph: &str,
        id: EdgeId,
    ) -> Result<(), StorageError> {
        let edge = self
            .read_edge(conn, graph, id.0)?
            .ok_or(StorageError::EdgeNotFound { id: id.0 })?;
        // Remove all edge data atomically via MULTI/EXEC pipeline.
        let out_member = encode_adj(id, edge.dst, &edge.rel_type);
        let in_member = encode_adj(id, edge.src, &edge.rel_type);
        redis::pipe()
            .atomic()
            .cmd("SREM")
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
            .ignore()
            .query::<()>(conn)
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        Ok(())
    }

    /// Create an edge between two existing nodes.
    ///
    /// Verifies that both source and destination nodes exist, allocates a new
    /// edge ID (or uses the injected `_sd2_eid` from FanoutStore), stores the
    /// edge hash with src/dst/rel_type/props, and updates the reltype index,
    /// out-adjacency set, in-adjacency set, and all-edges set atomically.
    pub(crate) fn create_edge_impl(
        &self,
        graph: &str,
        src: NodeId,
        dst: NodeId,
        rel_type: String,
        properties: HashMap<String, Value>,
    ) -> Result<EdgeId, StorageError> {
        self.require_graph(graph)?;
        let mut conn = self.conn()?;
        // Verify src and dst exist.
        if self.read_node(conn.as_mut(), graph, src.0)?.is_none() {
            return Err(StorageError::NodeNotFound { id: src.0 });
        }
        if self.read_node(conn.as_mut(), graph, dst.0)?.is_none() {
            return Err(StorageError::NodeNotFound { id: dst.0 });
        }

        // If FanoutStore injected _sd2_eid, use it instead of INCR.
        let id = match extract_injected_id(&properties, "_sd2_eid")? {
            Some(id) => id,
            None => {
                let raw_id: i64 = redis::cmd("INCRBY")
                    .arg(self.kp(key_edge_seq(graph)))
                    .arg(1)
                    .query(conn.as_mut())
                    .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
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

        let edge_key = self.kp(key_edge(graph, id));
        let props_json = props_to_json(&clean_props)?;
        // Store edge hash and update all indexes atomically.
        let out_member = encode_adj(EdgeId(id), dst, &rel_type);
        let in_member = encode_adj(EdgeId(id), src, &rel_type);
        redis::pipe()
            .atomic()
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
            .cmd("SADD")
            .arg(self.kp(key_edges_set(graph)))
            .arg(id.to_string())
            .ignore()
            .query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        Ok(EdgeId(id))
    }

    /// Merge additional properties into an existing edge (SET semantics).
    ///
    /// Reads the current edge, merges new properties on top of existing ones,
    /// and writes the merged result back.
    pub(crate) fn set_edge_properties_impl(
        &self,
        graph: &str,
        id: EdgeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        let mut conn = self.conn()?;
        let edge = self
            .read_edge(conn.as_mut(), graph, id.0)?
            .ok_or(StorageError::EdgeNotFound { id: id.0 })?;
        let mut merged = edge.properties;
        merged.extend(properties);
        let props_json = props_to_json(&merged)?;
        redis::cmd("HSET")
            .arg(self.kp(key_edge(graph, id.0)))
            .arg("props")
            .arg(&props_json)
            .query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })
    }

    /// Replace all properties on an existing edge (REPLACE semantics).
    ///
    /// Unlike `set_edge_properties_impl`, this discards all existing properties
    /// and replaces them entirely with the provided map.
    pub(crate) fn replace_edge_properties_impl(
        &self,
        graph: &str,
        id: EdgeId,
        properties: HashMap<String, Value>,
    ) -> Result<(), StorageError> {
        let mut conn = self.conn()?;
        if self.read_edge(conn.as_mut(), graph, id.0)?.is_none() {
            return Err(StorageError::EdgeNotFound { id: id.0 });
        }
        let props_json = props_to_json(&properties)?;
        redis::cmd("HSET")
            .arg(self.kp(key_edge(graph, id.0)))
            .arg("props")
            .arg(&props_json)
            .query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })
    }

    /// Remove a single property key from an edge.
    ///
    /// Reads the current edge, removes the specified key from its property map,
    /// and writes the remaining properties back.
    pub(crate) fn remove_edge_property_impl(
        &self,
        graph: &str,
        id: EdgeId,
        key: &str,
    ) -> Result<(), StorageError> {
        let mut conn = self.conn()?;
        let edge = self
            .read_edge(conn.as_mut(), graph, id.0)?
            .ok_or(StorageError::EdgeNotFound { id: id.0 })?;
        let mut props = edge.properties;
        props.remove(key);
        let props_json = props_to_json(&props)?;
        redis::cmd("HSET")
            .arg(self.kp(key_edge(graph, id.0)))
            .arg("props")
            .arg(&props_json)
            .query::<()>(conn.as_mut())
            .map_err(|e| StorageError::Backend { msg: e.to_string() })
    }

    /// Delete an edge by ID (public entry point that acquires its own connection).
    pub(crate) fn delete_edge_impl(&self, graph: &str, id: EdgeId) -> Result<(), StorageError> {
        let mut conn = self.conn()?;
        self.delete_edge_inner(conn.as_mut(), graph, id)
    }
}

#[cfg(test)]
mod tests {
    use gql_core::storage::{EdgeId, NodeId};
    use crate::keys::{encode_adj, decode_adj};

    // ---- Adjacency encoding used by edge write pipelines --------------------

    #[test]
    fn encode_adj_format_for_edge_pipeline() {
        // Edge writes construct adjacency members like "{eid}:{neighbor}:{rel_type}".
        let member = encode_adj(EdgeId(100), NodeId(200), "KNOWS");
        assert_eq!(member, "100:200:KNOWS");
    }

    #[test]
    fn encode_adj_round_trip_with_colon_in_rel_type() {
        // Relationship types may contain colons (e.g. namespaced types).
        let member = encode_adj(EdgeId(5), NodeId(10), "ns:HAS:ITEM");
        let decoded = decode_adj(&member).expect("should decode");
        assert_eq!(decoded.edge_id, EdgeId(5));
        assert_eq!(decoded.neighbor_id, NodeId(10));
        assert_eq!(decoded.rel_type, "ns:HAS:ITEM");
    }

    #[test]
    fn adjacency_member_components() {
        // Verify the adjacency member format that the SADD pipeline uses.
        let out_member = encode_adj(EdgeId(1), NodeId(2), "FOLLOWS");
        let in_member = encode_adj(EdgeId(1), NodeId(3), "FOLLOWS");
        // out: edge -> dst, in: edge -> src
        assert!(out_member.starts_with("1:2:"));
        assert!(in_member.starts_with("1:3:"));
    }

    // ---- extract_injected_id for edge IDs -----------------------------------

    #[test]
    fn extract_edge_id_from_properties() {
        use super::super::helpers::extract_injected_id;
        use gql_core::types::Value;
        use std::collections::HashMap;

        let mut props = HashMap::new();
        props.insert("_sd2_eid".to_string(), Value::Int(99));
        let result = extract_injected_id(&props, "_sd2_eid").unwrap();
        assert_eq!(result, Some(99));
    }

    #[test]
    fn extract_edge_id_negative_fails() {
        use super::super::helpers::extract_injected_id;
        use gql_core::types::Value;
        use std::collections::HashMap;

        let mut props = HashMap::new();
        props.insert("_sd2_eid".to_string(), Value::Int(-1));
        assert!(extract_injected_id(&props, "_sd2_eid").is_err());
    }
}
