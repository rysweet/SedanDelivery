// Per-entity Redis hash readers used by both the GraphStore impl and the
// adjacency-walking helpers in `reads.rs`.
//
// These are intentionally narrow utilities: a single HMGET round trip per
// entity, decoded into the cypher-core domain types. They live separately
// from the rest of the store so that bug fixes to the on-disk hash layout
// stay localized to one file.

use cypher_core::error::StorageError;
use cypher_core::serde_value::json_to_props;
use cypher_core::storage::{AdjEntry, EdgeData, EdgeId, NodeData, NodeId};
use redis::ConnectionLike;

use crate::backend_storage_error;
use crate::connection::RedisConn;
use crate::error_metrics::record_backend_error;
use crate::keys::{decode_adj, key_edge, key_node};
use crate::RedisGraphStore;

impl RedisGraphStore {
    /// Read a node hash from Redis; returns `None` if the key is absent.
    pub(crate) fn read_node(
        &self,
        conn: &mut dyn ConnectionLike,
        g: &str,
        id: u64,
    ) -> Result<Option<NodeData>, StorageError> {
        let key = self.kp(key_node(g, id));
        let fields: Vec<Option<String>> = redis::cmd("HMGET")
            .arg(&key)
            .arg("labels")
            .arg("props")
            .query(conn)
            .map_err(backend_storage_error)?;
        match (&fields[0], &fields[1]) {
            (None, None) => Ok(None),
            (Some(labels_json), Some(props_json)) => {
                let labels: Vec<String> =
                    serde_json::from_str(labels_json).map_err(|e| StorageError::Serialization {
                        msg: format!("labels: {e}"),
                    })?;
                let properties = json_to_props(props_json)?;
                Ok(Some(NodeData {
                    id: NodeId(id),
                    labels,
                    properties,
                }))
            }
            _ => {
                tracing::error!(
                    node_id = id,
                    graph = g,
                    labels = ?fields[0],
                    props = ?fields[1],
                    "partial node data detected — possible data corruption"
                );
                record_backend_error();
                Err(StorageError::Backend {
                    msg: format!(
                        "corrupted node data: node {id} in graph '{g}' has partial fields"
                    ),
                })
            }
        }
    }

    pub(crate) fn read_nodes_batch(
        &self,
        conn: &mut RedisConn,
        graph: &str,
        ids: &[NodeId],
    ) -> Result<Vec<Option<NodeData>>, StorageError> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let mut pipe = conn.pipeline();
        for id in ids {
            pipe.cmd("HMGET")
                .arg(self.kp(key_node(graph, id.0)))
                .arg("labels")
                .arg("props");
        }
        let raw: Vec<Vec<Option<String>>> = pipe.query(conn).map_err(backend_storage_error)?;
        let mut results = Vec::with_capacity(ids.len());
        for (idx, fields) in raw.into_iter().enumerate() {
            let id = ids[idx];
            match (
                &fields.first().and_then(|field| field.as_ref()),
                &fields.get(1).and_then(|field| field.as_ref()),
            ) {
                (None, None) => results.push(None),
                (Some(labels_json), Some(props_json)) => {
                    let labels: Vec<String> = serde_json::from_str(labels_json).map_err(|e| {
                        StorageError::Serialization {
                            msg: format!("labels batch: {e}"),
                        }
                    })?;
                    let properties = json_to_props(props_json)?;
                    results.push(Some(NodeData {
                        id,
                        labels,
                        properties,
                    }));
                }
                _ => {
                    tracing::error!(
                        node_id = id.0,
                        graph = graph,
                        labels = ?fields.first().and_then(|field| field.as_ref()),
                        props = ?fields.get(1).and_then(|field| field.as_ref()),
                        "partial node data detected during batch read — treating entry as missing node to preserve sparse batch semantics"
                    );
                    record_backend_error();
                    results.push(None);
                }
            }
        }
        Ok(results)
    }

    /// Read an edge hash from Redis; returns `None` if the key is absent.
    pub(crate) fn read_edge(
        &self,
        conn: &mut dyn ConnectionLike,
        g: &str,
        id: u64,
    ) -> Result<Option<EdgeData>, StorageError> {
        let key = self.kp(key_edge(g, id));
        let fields: Vec<Option<String>> = redis::cmd("HMGET")
            .arg(&key)
            .arg("src")
            .arg("dst")
            .arg("rel_type")
            .arg("props")
            .query(conn)
            .map_err(backend_storage_error)?;
        match (&fields[0], &fields[1], &fields[2], &fields[3]) {
            (Some(src_s), Some(dst_s), Some(rel_type), Some(props_json)) => {
                let src =
                    NodeId(
                        src_s
                            .parse::<u64>()
                            .map_err(|_| StorageError::Serialization {
                                msg: "bad src id".into(),
                            })?,
                    );
                let dst =
                    NodeId(
                        dst_s
                            .parse::<u64>()
                            .map_err(|_| StorageError::Serialization {
                                msg: "bad dst id".into(),
                            })?,
                    );
                let properties = json_to_props(props_json)?;
                Ok(Some(EdgeData {
                    id: EdgeId(id),
                    rel_type: rel_type.clone(),
                    src,
                    dst,
                    properties,
                }))
            }
            (None, None, None, None) => Ok(None),
            _ => {
                tracing::error!(
                    edge_id = id,
                    graph = g,
                    src = ?fields[0],
                    dst = ?fields[1],
                    rel_type = ?fields[2],
                    props = ?fields[3],
                    "partial edge data detected — possible data corruption"
                );
                record_backend_error();
                Err(StorageError::Backend {
                    msg: format!(
                        "corrupted edge data: edge {id} in graph '{g}' has partial fields"
                    ),
                })
            }
        }
    }

    /// Parse adjacency set members into `AdjEntry` list.
    pub(crate) fn parse_adj_members(members: Vec<String>) -> Vec<AdjEntry> {
        let mut entries = Vec::new();
        for m in members {
            match decode_adj(&m) {
                Some(entry) => entries.push(entry),
                None => {
                    tracing::warn!(member = %m, "failed to decode adjacency member, skipping");
                }
            }
        }
        entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::encode_adj;

    #[test]
    fn parse_adj_members_valid_entries() {
        let members = vec!["1:2:KNOWS".to_string(), "3:4:LIKES".to_string()];
        let entries = RedisGraphStore::parse_adj_members(members);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].edge_id, EdgeId(1));
        assert_eq!(entries[0].neighbor_id, NodeId(2));
        assert_eq!(entries[0].rel_type, "KNOWS");
        assert_eq!(entries[1].edge_id, EdgeId(3));
        assert_eq!(entries[1].neighbor_id, NodeId(4));
        assert_eq!(entries[1].rel_type, "LIKES");
    }

    #[test]
    fn parse_adj_members_skips_malformed() {
        let members = vec![
            "1:2:KNOWS".to_string(),
            "bad_entry".to_string(), // malformed — should be filtered out
            "3:4:LIKES".to_string(),
        ];
        let entries = RedisGraphStore::parse_adj_members(members);
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn parse_adj_members_empty_input() {
        let entries = RedisGraphStore::parse_adj_members(vec![]);
        assert!(entries.is_empty());
    }

    #[test]
    fn parse_adj_members_all_malformed() {
        let members = vec!["bad".to_string(), "also_bad".to_string()];
        let entries = RedisGraphStore::parse_adj_members(members);
        assert!(entries.is_empty());
    }

    #[test]
    fn encode_then_parse_round_trip() {
        let encoded = vec![
            encode_adj(EdgeId(10), NodeId(20), "FOLLOWS"),
            encode_adj(EdgeId(11), NodeId(21), "BLOCKS"),
        ];
        let entries = RedisGraphStore::parse_adj_members(encoded);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].edge_id, EdgeId(10));
        assert_eq!(entries[0].neighbor_id, NodeId(20));
        assert_eq!(entries[0].rel_type, "FOLLOWS");
        assert_eq!(entries[1].edge_id, EdgeId(11));
        assert_eq!(entries[1].neighbor_id, NodeId(21));
        assert_eq!(entries[1].rel_type, "BLOCKS");
    }
}
