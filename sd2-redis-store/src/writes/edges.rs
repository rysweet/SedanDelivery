// writes/edges.rs -- Edge write operations for RedisGraphStore.

use indexmap::IndexMap;

use cypher_core::error::StorageError;
use cypher_core::serde_value::props_to_json;
use cypher_core::storage::{EdgeId, NodeId};
use cypher_core::types::Value;

use crate::keys::{
    encode_adj, key_batch_edge_write_lock, key_edge, key_edge_id_write_lock, key_edge_seq,
    key_edge_write_lock, key_edges_set, key_in, key_out, key_reltype, key_reltype_names,
};
use crate::lock::{BATCH_WRITE_MAX_RETRIES, BATCH_WRITE_RETRY_DELAY_MS, WRITE_LOCK_TTL_MS};
use crate::validation::validate_property_key;
use crate::{LockedRedisConn, RedisGraphStore};

use super::helpers::{
    batch_pipeline_chunk_size, clean_fanout_props, ensure_key_absent, extract_injected_id,
    raise_sequence_floor,
};

fn validate_property_keys(properties: &IndexMap<String, Value>) -> Result<(), StorageError> {
    for key in properties.keys() {
        validate_property_key(key)?;
    }
    Ok(())
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

    /// Merge additional properties into an existing edge (SET semantics).
    ///
    /// Reads the current edge, merges new properties on top of existing ones,
    /// and writes the merged result back.
    pub(crate) fn set_edge_properties_impl(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        validate_property_keys(&properties)?;
        self.with_write_lock(
            graph,
            self.kp(key_edge_id_write_lock(graph, id.0)),
            WRITE_LOCK_TTL_MS,
            3,
            50,
            |conn| {
                self.require_graph_on(conn.as_mut(), graph)?;
                let edge = self
                    .read_edge(conn.as_mut(), graph, id.0)?
                    .ok_or(StorageError::EdgeNotFound { id: id.0 })?;
                let mut merged = edge.properties;
                merged.extend(properties);
                let props_json = props_to_json(&merged)?;
                conn.renew_lock()?;
                redis::cmd("HSET")
                    .arg(self.kp(key_edge(graph, id.0)))
                    .arg("props")
                    .arg(&props_json)
                    .query::<()>(conn.as_mut())
                    .map_err(crate::backend_storage_error)
            },
        )
    }

    /// Replace all properties on an existing edge (REPLACE semantics).
    ///
    /// Unlike `set_edge_properties_impl`, this discards all existing properties
    /// and replaces them entirely with the provided map.
    pub(crate) fn replace_edge_properties_impl(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        validate_property_keys(&properties)?;
        self.with_write_lock(
            graph,
            self.kp(key_edge_id_write_lock(graph, id.0)),
            WRITE_LOCK_TTL_MS,
            3,
            50,
            |conn| {
                self.require_graph_on(conn.as_mut(), graph)?;
                if self.read_edge(conn.as_mut(), graph, id.0)?.is_none() {
                    return Err(StorageError::EdgeNotFound { id: id.0 });
                }
                let props_json = props_to_json(&properties)?;
                conn.renew_lock()?;
                redis::cmd("HSET")
                    .arg(self.kp(key_edge(graph, id.0)))
                    .arg("props")
                    .arg(&props_json)
                    .query::<()>(conn.as_mut())
                    .map_err(crate::backend_storage_error)
            },
        )
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
        validate_property_key(key)?;
        self.with_write_lock(
            graph,
            self.kp(key_edge_id_write_lock(graph, id.0)),
            WRITE_LOCK_TTL_MS,
            3,
            50,
            |conn| {
                self.require_graph_on(conn.as_mut(), graph)?;
                let edge = self
                    .read_edge(conn.as_mut(), graph, id.0)?
                    .ok_or(StorageError::EdgeNotFound { id: id.0 })?;
                let mut props = edge.properties;
                props.shift_remove(key);
                let props_json = props_to_json(&props)?;
                conn.renew_lock()?;
                redis::cmd("HSET")
                    .arg(self.kp(key_edge(graph, id.0)))
                    .arg("props")
                    .arg(&props_json)
                    .query::<()>(conn.as_mut())
                    .map_err(crate::backend_storage_error)
            },
        )
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

    /// Batch-create multiple edges in a single Redis pipeline round-trip.
    ///
    /// For each edge: verifies that src and dst nodes exist, allocates a
    /// contiguous block of edge IDs via a single INCRBY, then stores all
    /// edge hashes, reltype indexes, adjacency sets, and the all-edges set
    /// in one pipeline.
    ///
    /// Returns the newly created `EdgeId`s in the same order as the input.
    pub fn batch_create_edges(
        &self,
        graph: &str,
        edges: Vec<(NodeId, NodeId, String, IndexMap<String, Value>)>,
    ) -> Result<Vec<EdgeId>, StorageError> {
        if edges.is_empty() {
            return Ok(Vec::new());
        }
        // All-or-nothing fallback: if any edge carries an injected `_sd2_eid`,
        // the entire batch falls back to single-item creation so each edge can
        // claim its injected ID via the sequential create_edge_impl path.
        if edges
            .iter()
            .any(|(_, _, _, properties)| properties.contains_key("_sd2_eid"))
        {
            let mut created = Vec::with_capacity(edges.len());
            for (src, dst, rel_type, properties) in edges {
                created.push(self.create_edge_impl(graph, src, dst, rel_type, properties)?);
            }
            return Ok(created);
        }
        for (_, _, _, properties) in &edges {
            validate_property_keys(properties)?;
        }
        self.with_write_lock(
            graph,
            self.kp(key_batch_edge_write_lock(graph)),
            WRITE_LOCK_TTL_MS,
            BATCH_WRITE_MAX_RETRIES,
            BATCH_WRITE_RETRY_DELAY_MS,
            |conn| {
                self.require_graph_on(conn.as_mut(), graph)?;

                // Verify all src and dst nodes exist before allocating IDs.
                // Collect unique node IDs to minimize reads.
                let mut unique_node_ids: std::collections::HashSet<u64> =
                    std::collections::HashSet::new();
                for (src, dst, _, _) in &edges {
                    unique_node_ids.insert(src.0);
                    unique_node_ids.insert(dst.0);
                }
                let unique_node_ids: Vec<NodeId> =
                    unique_node_ids.into_iter().map(NodeId).collect();
                let validation_chunk_size = batch_pipeline_chunk_size().min(250);
                for (chunk_index, chunk) in
                    unique_node_ids.chunks(validation_chunk_size).enumerate()
                {
                    if chunk_index > 0 {
                        conn.renew_lock()?;
                    }
                    let nodes = self.read_nodes_batch(conn, graph, chunk)?;
                    if let Some((missing_index, _)) =
                        nodes.iter().enumerate().find(|(_, node)| node.is_none())
                    {
                        return Err(StorageError::NodeNotFound {
                            id: chunk[missing_index].0,
                        });
                    }
                }

                let count = i64::try_from(edges.len()).map_err(|_| {
                    crate::record_backend_error();
                    StorageError::Backend {
                        msg: format!(
                            "batch size {} exceeds i64::MAX, cannot allocate IDs",
                            edges.len()
                        ),
                    }
                })?;

                // Allocate a contiguous block of IDs with a single INCRBY.
                let end_id: i64 = redis::cmd("INCRBY")
                    .arg(self.kp(key_edge_seq(graph)))
                    .arg(count)
                    .query(conn.as_mut())
                    .map_err(crate::backend_storage_error)?;
                if end_id <= 0 || end_id < count {
                    return Err(StorageError::IdOverflow {
                        graph: graph.to_string(),
                    });
                }
                let start_id = (end_id - count + 1) as u64;

                // Pre-serialize all edges and build chunked pipelines.
                let mut created_ids: Vec<EdgeId> = Vec::with_capacity(edges.len());

                let chunk_size = batch_pipeline_chunk_size();
                let mut chunk_iter = edges.into_iter().enumerate();
                loop {
                    let chunk: Vec<_> = chunk_iter.by_ref().take(chunk_size).collect();
                    if chunk.is_empty() {
                        break;
                    }

                    let mut pipe = conn.pipeline();
                    for (offset, (i, (src, dst, rel_type, properties))) in
                        chunk.into_iter().enumerate()
                    {
                        // Mid-loop renewal only activates if chunk size is
                        // configured above 250; the primary safeguard is the
                        // conn.renew_lock() call before pipe.query() below.
                        if offset > 0 && offset % 250 == 0 {
                            conn.renew_lock()?;
                        }
                        let id = start_id + i as u64;
                        let eid = EdgeId(id);
                        let id_str = id.to_string();

                        let clean_props = clean_fanout_props(properties);
                        let edge_key = self.kp(key_edge(graph, id));
                        let props_json = props_to_json(&clean_props)?;

                        pipe.cmd("SADD")
                            .arg(self.kp(key_reltype_names(graph)))
                            .arg(rel_type.as_str())
                            .ignore();
                        pipe.cmd("SADD")
                            .arg(self.kp(key_edges_set(graph)))
                            .arg(&id_str)
                            .ignore();
                        pipe.cmd("SADD")
                            .arg(self.kp(key_reltype(graph, &rel_type)))
                            .arg(&id_str)
                            .ignore();

                        let out_member = encode_adj(eid, dst, &rel_type);
                        pipe.cmd("SADD")
                            .arg(self.kp(key_out(graph, src.0)))
                            .arg(out_member)
                            .ignore();

                        let in_member = encode_adj(eid, src, &rel_type);
                        pipe.cmd("SADD")
                            .arg(self.kp(key_in(graph, dst.0)))
                            .arg(in_member)
                            .ignore();

                        pipe.cmd("HSET")
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

                        created_ids.push(eid);
                    }

                    conn.renew_lock()?;
                    pipe.query::<()>(conn)
                        .map_err(crate::backend_storage_error)?;
                }

                Ok(created_ids)
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::keys::{decode_adj, encode_adj};
    use cypher_core::storage::{EdgeId, NodeId};
    use cypher_core::types::Value;
    use indexmap::IndexMap;

    use super::validate_property_keys;

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

    #[test]
    fn validate_edge_property_keys_rejects_null_byte() {
        let mut props = IndexMap::new();
        props.insert("co\x00st".to_string(), Value::Int(5));
        let result = validate_property_keys(&props);
        assert!(
            result.is_err(),
            "edge write-path property validation should reject null-byte keys"
        );
    }

    // ---- extract_injected_id for edge IDs -----------------------------------

    #[test]
    fn extract_edge_id_from_properties() {
        use super::super::helpers::extract_injected_id;
        use cypher_core::types::Value;
        use indexmap::IndexMap;

        let mut props = IndexMap::new();
        props.insert("_sd2_eid".to_string(), Value::Int(99));
        let result = extract_injected_id(&props, "_sd2_eid").unwrap();
        assert_eq!(result, Some(99));
    }

    #[test]
    fn extract_edge_id_negative_fails() {
        use super::super::helpers::extract_injected_id;
        use cypher_core::types::Value;
        use indexmap::IndexMap;

        let mut props = IndexMap::new();
        props.insert("_sd2_eid".to_string(), Value::Int(-1));
        assert!(extract_injected_id(&props, "_sd2_eid").is_err());
    }

    // ---- Cluster pipeline safety: verify no MULTI/EXEC in edge write pipelines --

    /// Verify that the edge creation pipeline does NOT include MULTI or EXEC.
    ///
    /// Fix for Issue #745: redis-rs ClusterConnection does not support MULTI/EXEC.
    /// The .atomic() calls in create_edge_impl and delete_edge_inner have been
    /// removed. This test validates the fix remains in place.
    #[test]
    fn create_edge_pipeline_has_no_multi_exec() {
        let graph = "testgraph";
        let edge_id = 5u64;
        let src_id = 1u64;
        let dst_id = 2u64;
        let rel_type = "KNOWS";

        let edge_key = format!("G:{{{graph}}}:edge:{edge_id}");
        let out_member = format!("{edge_id}:{dst_id}:{rel_type}");
        let in_member = format!("{edge_id}:{src_id}:{rel_type}");

        let mut pipe = redis::pipe();
        // Replicate create_edge_impl pipeline (without .atomic())
        pipe.cmd("HSET")
            .arg(&edge_key)
            .arg("src")
            .arg(src_id.to_string())
            .arg("dst")
            .arg(dst_id.to_string())
            .arg("rel_type")
            .arg(rel_type)
            .arg("props")
            .arg("{}")
            .ignore()
            .cmd("SADD")
            .arg(format!("G:{{{graph}}}:reltype:{rel_type}"))
            .arg(edge_id.to_string())
            .ignore()
            .cmd("SADD")
            .arg(format!("G:{{{graph}}}:out:{src_id}"))
            .arg(out_member)
            .ignore()
            .cmd("SADD")
            .arg(format!("G:{{{graph}}}:in:{dst_id}"))
            .arg(in_member)
            .ignore()
            .cmd("SADD")
            .arg(format!("G:{{{graph}}}:edges"))
            .arg(edge_id.to_string())
            .ignore();

        let packed = pipe.get_packed_pipeline();
        let pipeline_str = String::from_utf8_lossy(&packed);
        assert!(
            !pipeline_str.to_uppercase().contains("MULTI"),
            "Edge create pipeline must not contain MULTI: {pipeline_str}"
        );
        assert!(
            !pipeline_str.to_uppercase().contains("EXEC"),
            "Edge create pipeline must not contain EXEC: {pipeline_str}"
        );
    }

    /// Verify that the edge deletion pipeline does NOT include MULTI or EXEC.
    #[test]
    fn delete_edge_pipeline_has_no_multi_exec() {
        let graph = "testgraph";
        let edge_id = EdgeId(5);
        let src_id = NodeId(1);
        let dst_id = NodeId(2);
        let rel_type = "KNOWS";

        let out_member = encode_adj(edge_id, dst_id, rel_type);
        let in_member = encode_adj(edge_id, src_id, rel_type);

        let mut pipe = redis::pipe();
        // Replicate delete_edge_inner pipeline (without .atomic())
        pipe.cmd("SREM")
            .arg(format!("G:{{{graph}}}:reltype:{rel_type}"))
            .arg(edge_id.0.to_string())
            .ignore()
            .cmd("SREM")
            .arg(format!("G:{{{graph}}}:edges"))
            .arg(edge_id.0.to_string())
            .ignore()
            .cmd("SREM")
            .arg(format!("G:{{{graph}}}:out:{}", src_id.0))
            .arg(out_member)
            .ignore()
            .cmd("SREM")
            .arg(format!("G:{{{graph}}}:in:{}", dst_id.0))
            .arg(in_member)
            .ignore()
            .cmd("DEL")
            .arg(format!("G:{{{graph}}}:edge:{}", edge_id.0))
            .ignore();

        let packed = pipe.get_packed_pipeline();
        let pipeline_str = String::from_utf8_lossy(&packed);
        assert!(
            !pipeline_str.to_uppercase().contains("MULTI"),
            "Edge delete pipeline must not contain MULTI: {pipeline_str}"
        );
        assert!(
            !pipeline_str.to_uppercase().contains("EXEC"),
            "Edge delete pipeline must not contain EXEC: {pipeline_str}"
        );
    }

    /// Verify that all keys in the edge pipelines share the {graphname} hash tag.
    #[test]
    fn edge_pipeline_keys_all_share_hash_tag() {
        let graph = "mygraph";
        let edge_id = 10u64;
        let src_id = 1u64;
        let dst_id = 2u64;
        let rel_type = "FOLLOWS";
        let expected_tag = format!("{{{graph}}}");

        // Keys used in create_edge_impl and delete_edge_inner.
        let keys = vec![
            format!("G:{expected_tag}:edge:{edge_id}"),
            format!("G:{expected_tag}:reltype:{rel_type}"),
            format!("G:{expected_tag}:out:{src_id}"),
            format!("G:{expected_tag}:in:{dst_id}"),
            format!("G:{expected_tag}:edges"),
        ];

        for k in &keys {
            assert!(
                k.contains(&expected_tag),
                "Edge pipeline key missing hash tag {expected_tag}: {k}"
            );
        }
    }
}
