// writes/edges/batch.rs -- Batch edge creation for RedisGraphStore.

use indexmap::IndexMap;

use cypher_core::error::StorageError;
use cypher_core::serde_value::props_to_json;
use cypher_core::storage::{EdgeId, NodeId};
use cypher_core::types::Value;

use crate::keys::{
    encode_adj, key_batch_edge_write_lock, key_edge, key_edge_seq, key_edges_set, key_in, key_out,
    key_reltype, key_reltype_names,
};
use crate::lock::{BATCH_WRITE_MAX_RETRIES, BATCH_WRITE_RETRY_DELAY_MS, WRITE_LOCK_TTL_MS};
use crate::writes::helpers::{batch_pipeline_chunk_size, clean_fanout_props};
use crate::RedisGraphStore;

use super::validate_property_keys;

impl RedisGraphStore {
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
                            .key(self.kp(key_reltype_names(graph)))
                            .arg(rel_type.as_str())
                            .ignore();
                        pipe.cmd("SADD")
                            .key(self.kp(key_edges_set(graph)))
                            .arg(&id_str)
                            .ignore();
                        pipe.cmd("SADD")
                            .key(self.kp(key_reltype(graph, &rel_type)))
                            .arg(&id_str)
                            .ignore();

                        let out_member = encode_adj(eid, dst, &rel_type);
                        pipe.cmd("SADD")
                            .key(self.kp(key_out(graph, src.0)))
                            .arg(out_member)
                            .ignore();

                        let in_member = encode_adj(eid, src, &rel_type);
                        pipe.cmd("SADD")
                            .key(self.kp(key_in(graph, dst.0)))
                            .arg(in_member)
                            .ignore();

                        pipe.cmd("HSET")
                            .key(edge_key.clone())
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
