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
        Self::decode_edge_batch_results(ids, raw)
    }

    /// Pure decoder: convert the raw HMGET(src, dst, rel_type, props) responses
    /// for a batch of edges into [`EdgeData`] values, returning `None` for any
    /// edge whose hash was missing or had fewer than four fields.
    pub(crate) fn decode_edge_batch_results(
        ids: &[EdgeId],
        raw: Vec<Vec<Option<String>>>,
    ) -> Result<Vec<Option<EdgeData>>, StorageError> {
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

#[cfg(test)]
mod decode_tests {
    use super::*;

    #[test]
    fn decode_edge_batch_returns_none_for_missing_hash() {
        let ids = vec![EdgeId(1), EdgeId(2)];
        let raw = vec![
            vec![
                Some("1".into()),
                Some("2".into()),
                Some("KNOWS".into()),
                Some("{}".into()),
            ],
            vec![None, None, None, None],
        ];
        let out = RedisGraphStore::decode_edge_batch_results(&ids, raw).unwrap();
        let edge = out[0].as_ref().unwrap();
        assert_eq!(edge.id, EdgeId(1));
        assert_eq!(edge.src, NodeId(1));
        assert_eq!(edge.rel_type, "KNOWS");
        assert!(out[1].is_none());
    }

    #[test]
    fn decode_edge_batch_returns_none_for_short_field_vec() {
        let ids = vec![EdgeId(7)];
        let raw = vec![vec![Some("1".into()), Some("2".into())]]; // only 2 fields
        let out = RedisGraphStore::decode_edge_batch_results(&ids, raw).unwrap();
        assert!(out[0].is_none());
    }

    #[test]
    fn decode_edge_batch_serialization_error_on_bad_src() {
        let ids = vec![EdgeId(1)];
        let raw = vec![vec![
            Some("not-a-number".into()),
            Some("2".into()),
            Some("R".into()),
            Some("{}".into()),
        ]];
        let err = RedisGraphStore::decode_edge_batch_results(&ids, raw).unwrap_err();
        assert!(matches!(err, StorageError::Serialization { msg } if msg == "bad src"));
    }

    #[test]
    fn decode_edge_batch_serialization_error_on_bad_dst() {
        let ids = vec![EdgeId(1)];
        let raw = vec![vec![
            Some("1".into()),
            Some("nope".into()),
            Some("R".into()),
            Some("{}".into()),
        ]];
        let err = RedisGraphStore::decode_edge_batch_results(&ids, raw).unwrap_err();
        assert!(matches!(err, StorageError::Serialization { msg } if msg == "bad dst"));
    }

    #[test]
    fn decode_edge_batch_serialization_error_on_bad_props() {
        let ids = vec![EdgeId(1)];
        let raw = vec![vec![
            Some("1".into()),
            Some("2".into()),
            Some("R".into()),
            Some("not-json".into()),
        ]];
        let err = RedisGraphStore::decode_edge_batch_results(&ids, raw).unwrap_err();
        assert!(matches!(err, StorageError::Serialization { .. }));
    }

    #[test]
    fn decode_edge_batch_handles_empty_input() {
        let out = RedisGraphStore::decode_edge_batch_results(&[], vec![]).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn decode_edge_batch_decodes_multiple_edges_in_order() {
        let ids = vec![EdgeId(10), EdgeId(20)];
        let raw = vec![
            vec![
                Some("1".into()),
                Some("2".into()),
                Some("A".into()),
                Some("{}".into()),
            ],
            vec![
                Some("3".into()),
                Some("4".into()),
                Some("B".into()),
                Some("{}".into()),
            ],
        ];
        let out = RedisGraphStore::decode_edge_batch_results(&ids, raw).unwrap();
        assert_eq!(out[0].as_ref().unwrap().rel_type, "A");
        assert_eq!(out[1].as_ref().unwrap().rel_type, "B");
        assert_eq!(out[0].as_ref().unwrap().id, EdgeId(10));
        assert_eq!(out[1].as_ref().unwrap().id, EdgeId(20));
    }
}
