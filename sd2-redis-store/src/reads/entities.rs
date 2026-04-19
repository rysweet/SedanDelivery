// Single-item and batch reads for nodes and edges.

use cypher_core::error::StorageError;
use cypher_core::serde_value::json_to_props;
use cypher_core::storage::{EdgeData, EdgeId, NodeData, NodeId};

use crate::keys::key_edge;
use crate::RedisGraphStore;

impl RedisGraphStore {
    // ── Single-item reads ────────────────────────────────────────────────────

    pub(crate) fn get_node_impl(
        &self,
        graph: &str,
        id: NodeId,
    ) -> Result<Option<NodeData>, StorageError> {
        let mut conn = self.route_read_for_graph(graph)?;
        self.read_node(conn.as_mut(), graph, id.0)
    }

    pub(crate) fn get_edge_impl(
        &self,
        graph: &str,
        id: EdgeId,
    ) -> Result<Option<EdgeData>, StorageError> {
        let mut conn = self.route_read_for_graph(graph)?;
        self.read_edge(conn.as_mut(), graph, id.0)
    }

    // ── Batch reads ──────────────────────────────────────────────────────────

    pub(crate) fn get_nodes_batch_impl(
        &self,
        graph: &str,
        ids: &[NodeId],
    ) -> Result<Vec<Option<NodeData>>, StorageError> {
        let mut conn = self.route_read_for_graph(graph)?;
        self.read_nodes_batch(&mut conn, graph, ids)
    }

    pub(crate) fn get_edges_batch_impl(
        &self,
        graph: &str,
        ids: &[EdgeId],
    ) -> Result<Vec<Option<EdgeData>>, StorageError> {
        let mut conn = self.route_read_for_graph(graph)?;
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let mut pipe = conn.pipeline();
        for id in ids {
            pipe.cmd("HMGET")
                .arg(self.kp(key_edge(graph, id.0)))
                .arg("src")
                .arg("dst")
                .arg("rel_type")
                .arg("props");
        }
        let raw: Vec<Vec<Option<String>>> = pipe
            .query(&mut conn)
            .map_err(crate::backend_storage_error)?;
        let mut results = Vec::with_capacity(ids.len());
        for (idx, fields) in raw.into_iter().enumerate() {
            let id = ids[idx];
            if fields.len() >= 4 {
                if let (Some(src_s), Some(dst_s), Some(rel_type), Some(props_json)) =
                    (&fields[0], &fields[1], &fields[2], &fields[3])
                {
                    let src =
                        NodeId(
                            src_s
                                .parse::<u64>()
                                .map_err(|_| StorageError::Serialization {
                                    msg: "bad src".into(),
                                })?,
                        );
                    let dst =
                        NodeId(
                            dst_s
                                .parse::<u64>()
                                .map_err(|_| StorageError::Serialization {
                                    msg: "bad dst".into(),
                                })?,
                        );
                    let properties = json_to_props(props_json)?;
                    results.push(Some(EdgeData {
                        id,
                        rel_type: rel_type.clone(),
                        src,
                        dst,
                        properties,
                    }));
                    continue;
                }
            }
            results.push(None);
        }
        Ok(results)
    }
}
