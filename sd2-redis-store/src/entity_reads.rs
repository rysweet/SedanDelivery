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

    // ── Mock-backed read_node / read_edge tests ──────────────────────────
    //
    // These exercise the decode + error paths of read_node / read_edge
    // without requiring a live Redis.  We implement a minimal ConnectionLike
    // that returns a canned `Vec<Value>` response for the next HMGET.

    use redis::{Arg, Cmd, RedisResult, Value as RedisValue};

    #[derive(Default)]
    struct FakeHmgetConn {
        response: Vec<RedisValue>,
    }

    impl FakeHmgetConn {
        fn cmd_name(cmd: &Cmd) -> String {
            cmd.args_iter()
                .next()
                .map(|a| match a {
                    Arg::Simple(b) => String::from_utf8_lossy(b).to_string(),
                    Arg::Cursor => "<cursor>".to_string(),
                    _ => String::new(),
                })
                .unwrap_or_default()
        }
    }

    impl redis::ConnectionLike for FakeHmgetConn {
        fn req_packed_command(&mut self, _cmd: &[u8]) -> RedisResult<RedisValue> {
            panic!("packed_command not expected in unit tests")
        }
        fn req_packed_commands(
            &mut self,
            _cmd: &[u8],
            _offset: usize,
            _count: usize,
        ) -> RedisResult<Vec<RedisValue>> {
            panic!("packed_commands not expected in unit tests")
        }
        fn req_command(&mut self, cmd: &Cmd) -> RedisResult<RedisValue> {
            assert_eq!(Self::cmd_name(cmd), "HMGET");
            Ok(RedisValue::Array(self.response.clone()))
        }
        fn get_db(&self) -> i64 {
            0
        }
        fn check_connection(&mut self) -> bool {
            true
        }
        fn is_open(&self) -> bool {
            true
        }
    }

    fn bulk(s: &str) -> RedisValue {
        RedisValue::BulkString(s.as_bytes().to_vec())
    }

    fn store_for_tests() -> crate::RedisGraphStore {
        crate::RedisGraphStore::new_for_testing()
    }

    #[test]
    fn read_node_returns_none_for_missing_key() {
        let mut conn = FakeHmgetConn {
            response: vec![RedisValue::Nil, RedisValue::Nil],
        };
        let store = store_for_tests();
        let result = store.read_node(&mut conn, "g", 1).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn read_node_decodes_complete_hash() {
        let mut conn = FakeHmgetConn {
            response: vec![bulk(r#"["Person","User"]"#), bulk(r#"{"age":30}"#)],
        };
        let store = store_for_tests();
        let node = store.read_node(&mut conn, "g", 42).unwrap().unwrap();
        assert_eq!(node.id, NodeId(42));
        assert_eq!(node.labels, vec!["Person".to_string(), "User".to_string()]);
        assert!(node.properties.contains_key("age"));
    }

    #[test]
    fn read_node_detects_partial_corruption() {
        // labels present, props missing -> corruption error
        let mut conn = FakeHmgetConn {
            response: vec![bulk(r#"["X"]"#), RedisValue::Nil],
        };
        let store = store_for_tests();
        let err = store.read_node(&mut conn, "g", 7).unwrap_err();
        match err {
            StorageError::Backend { msg } => {
                assert!(msg.contains("corrupted") || msg.contains("backend"));
            }
            other => panic!("expected Backend error, got {other:?}"),
        }
    }

    #[test]
    fn read_node_bad_labels_json_serialization_error() {
        let mut conn = FakeHmgetConn {
            response: vec![bulk("not-a-json-array"), bulk(r#"{}"#)],
        };
        let store = store_for_tests();
        let err = store.read_node(&mut conn, "g", 1).unwrap_err();
        assert!(matches!(err, StorageError::Serialization { .. }));
    }

    #[test]
    fn read_edge_returns_none_for_missing_key() {
        let mut conn = FakeHmgetConn {
            response: vec![
                RedisValue::Nil,
                RedisValue::Nil,
                RedisValue::Nil,
                RedisValue::Nil,
            ],
        };
        let store = store_for_tests();
        let result = store.read_edge(&mut conn, "g", 1).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn read_edge_decodes_complete_hash() {
        let mut conn = FakeHmgetConn {
            response: vec![
                bulk("10"),
                bulk("20"),
                bulk("KNOWS"),
                bulk(r#"{"since":2020}"#),
            ],
        };
        let store = store_for_tests();
        let edge = store.read_edge(&mut conn, "g", 5).unwrap().unwrap();
        assert_eq!(edge.id, EdgeId(5));
        assert_eq!(edge.src, NodeId(10));
        assert_eq!(edge.dst, NodeId(20));
        assert_eq!(edge.rel_type, "KNOWS");
    }

    #[test]
    fn read_edge_bad_src_id_gives_serialization_error() {
        let mut conn = FakeHmgetConn {
            response: vec![bulk("not-a-number"), bulk("20"), bulk("R"), bulk(r#"{}"#)],
        };
        let store = store_for_tests();
        let err = store.read_edge(&mut conn, "g", 1).unwrap_err();
        match err {
            StorageError::Serialization { msg } => assert!(msg.contains("src")),
            other => panic!("expected Serialization, got {other:?}"),
        }
    }

    #[test]
    fn read_edge_bad_dst_id_gives_serialization_error() {
        let mut conn = FakeHmgetConn {
            response: vec![bulk("1"), bulk("xx"), bulk("R"), bulk(r#"{}"#)],
        };
        let store = store_for_tests();
        let err = store.read_edge(&mut conn, "g", 1).unwrap_err();
        match err {
            StorageError::Serialization { msg } => assert!(msg.contains("dst")),
            other => panic!("expected Serialization, got {other:?}"),
        }
    }

    #[test]
    fn read_edge_partial_data_returns_backend_error() {
        // only src present -> partial corruption
        let mut conn = FakeHmgetConn {
            response: vec![bulk("1"), RedisValue::Nil, RedisValue::Nil, RedisValue::Nil],
        };
        let store = store_for_tests();
        let err = store.read_edge(&mut conn, "g", 9).unwrap_err();
        assert!(matches!(err, StorageError::Backend { .. }));
    }
}
