// Label/type and property index reads.

use cypher_core::error::StorageError;
use cypher_core::serde_value::json_to_props;
use cypher_core::storage::{EdgeId, NodeId, RangeQuery};
use redis::ConnectionLike;

use crate::keys::{key_label, key_node, key_propidx, key_reltype, value_to_index_key, IDX_SEP};
use crate::RedisGraphStore;

use super::helpers::{smembers_or_scan, smembers_threshold};

impl RedisGraphStore {
    pub(crate) fn find_nodes_by_label_impl(
        &self,
        graph: &str,
        label: &str,
    ) -> Result<Vec<NodeId>, StorageError> {
        let mut conn = self.route_read_for_graph(graph)?;
        self.find_nodes_by_label_on(conn.as_mut(), graph, label)
    }

    pub(crate) fn find_nodes_by_label_on(
        &self,
        conn: &mut dyn ConnectionLike,
        graph: &str,
        label: &str,
    ) -> Result<Vec<NodeId>, StorageError> {
        let members = smembers_or_scan(
            conn,
            &self.kp(key_label(graph, label)),
            smembers_threshold(),
        )?;
        members
            .into_iter()
            .map(|s| {
                s.parse::<u64>()
                    .map(NodeId)
                    .map_err(|_| StorageError::Serialization {
                        msg: format!("bad node id in label index: {s}"),
                    })
            })
            .collect()
    }

    pub(crate) fn find_edges_by_type_on(
        &self,
        conn: &mut dyn ConnectionLike,
        graph: &str,
        rel_type: &str,
    ) -> Result<Vec<EdgeId>, StorageError> {
        let members = smembers_or_scan(
            conn,
            &self.kp(key_reltype(graph, rel_type)),
            smembers_threshold(),
        )?;
        members
            .into_iter()
            .map(|s| {
                s.parse::<u64>()
                    .map(EdgeId)
                    .map_err(|_| StorageError::Serialization {
                        msg: format!("bad edge id in reltype index: {s}"),
                    })
            })
            .collect()
    }

    pub(crate) fn find_nodes_by_label_and_property_impl(
        &self,
        graph: &str,
        label: &str,
        property: &str,
        range: RangeQuery<'_>,
    ) -> Result<Vec<NodeId>, StorageError> {
        let mut conn = self.route_read_for_graph(graph)?;

        // Fast path: use property index for exact-match queries when an index exists.
        if let RangeQuery::Eq(val) = &range {
            let indexes = self.active_indexes(conn.as_mut(), graph)?;
            if indexes.contains(&format!("{label}{IDX_SEP}{property}")) {
                let vkey = value_to_index_key(val);
                let members = smembers_or_scan(
                    conn.as_mut(),
                    &self.kp(key_propidx(graph, label, property, &vkey)),
                    smembers_threshold(),
                )?;
                return members
                    .into_iter()
                    .map(|s| {
                        s.parse::<u64>()
                            .map(NodeId)
                            .map_err(|_| StorageError::Serialization {
                                msg: format!("bad node id in propidx: {s}"),
                            })
                    })
                    .collect();
            }
        }

        // No declared property index — do a label scan + property filter in Redis.
        // This is slower than a property index but avoids the executor's fallback
        // which creates a cartesian product when used inside UNWIND + Apply.
        let label_members = smembers_or_scan(
            conn.as_mut(),
            &self.kp(key_label(graph, label)),
            smembers_threshold(),
        )?;

        // Parse member strings to u64 IDs, skip invalid ones.
        let node_ids: Vec<u64> = label_members
            .iter()
            .filter_map(|s| s.parse::<u64>().ok())
            .collect();

        // Pipeline HGET calls for the target property on all label members.
        // Use `conn.pipeline()` (not `redis::pipe()`) so that cluster
        // connections receive a `cluster_pipe()` capable of handling MOVED/ASK
        // redirects. All keys here share the `{graph}` hash tag and route to
        // the same shard, but the cluster pipeline wrapper is still required;
        // a standalone `redis::pipe()` against a `ClusterConnection` silently
        // returns no results, which manifested as missing label+property
        // matches in cluster mode (see #1090 follow-up: empty UNWIND-driven
        // MATCH results, missing edge-source/target nodes).
        let mut pipe = conn.pipeline();
        for &nid in &node_ids {
            pipe.cmd("HGET")
                .arg(self.kp(key_node(graph, nid)))
                .arg("props");
        }
        // Propagate pipeline errors instead of silently coercing to empty
        // results. The previous `unwrap_or_else(|_| vec![None; ...])` was
        // what allowed the cluster pipeline mismatch (#1276) to silently
        // return zero matches across the entire fleet for ~5 months.
        let props_results: Vec<Option<String>> = if node_ids.is_empty() {
            Vec::new()
        } else {
            pipe.query(&mut conn).map_err(|e| StorageError::Backend {
                msg: format!("pipelined HGET for label+property scan failed: {e}"),
            })?
        };

        let mut matched = Vec::new();
        for (&node_id, props_json) in node_ids.iter().zip(props_results.iter()) {
            if let Some(json) = props_json {
                if let Ok(props) = json_to_props(json) {
                    if let Some(val) = props.get(property) {
                        let matches = match &range {
                            RangeQuery::Eq(expected) => val == *expected,
                            _ => true, // Non-eq ranges: include all and let executor filter
                        };
                        if matches {
                            matched.push(NodeId(node_id));
                        }
                    }
                }
            }
        }
        Ok(matched)
    }

    pub(crate) fn find_edges_by_type_impl(
        &self,
        graph: &str,
        rel_type: &str,
    ) -> Result<Vec<EdgeId>, StorageError> {
        let mut conn = self.route_read_for_graph(graph)?;
        self.find_edges_by_type_on(conn.as_mut(), graph, rel_type)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use redis::{Arg, Cmd, RedisError, RedisResult, Value as RedisValue};
    use std::collections::VecDeque;

    #[derive(Default)]
    struct ScriptedConn {
        responses: VecDeque<(String, RedisResult<RedisValue>)>,
    }
    impl ScriptedConn {
        fn push(mut self, name: &str, v: RedisResult<RedisValue>) -> Self {
            self.responses.push_back((name.into(), v));
            self
        }
        fn ok_smembers(self, items: &[&str]) -> Self {
            self.push(
                "SMEMBERS",
                Ok(RedisValue::Array(
                    items
                        .iter()
                        .map(|s| RedisValue::BulkString(s.as_bytes().to_vec()))
                        .collect(),
                )),
            )
        }
        fn ok_scard(self, n: i64) -> Self {
            self.push("SCARD", Ok(RedisValue::Int(n)))
        }
        fn err(self, name: &str) -> Self {
            self.push(
                name,
                Err(RedisError::from((redis::ErrorKind::Io, "scripted"))),
            )
        }
    }
    impl ConnectionLike for ScriptedConn {
        fn req_packed_command(&mut self, _c: &[u8]) -> RedisResult<RedisValue> {
            panic!("packed not expected");
        }
        fn req_packed_commands(
            &mut self,
            _c: &[u8],
            _o: usize,
            _n: usize,
        ) -> RedisResult<Vec<RedisValue>> {
            panic!("packed not expected");
        }
        fn req_command(&mut self, cmd: &Cmd) -> RedisResult<RedisValue> {
            let name = cmd
                .args_iter()
                .next()
                .map(|a| match a {
                    Arg::Simple(b) => String::from_utf8_lossy(b).to_string(),
                    _ => String::new(),
                })
                .unwrap_or_default();
            let (expected, response) = self
                .responses
                .pop_front()
                .unwrap_or_else(|| panic!("no scripted response for {name}"));
            assert_eq!(name, expected, "got {name}, want {expected}");
            response
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

    fn store() -> RedisGraphStore {
        RedisGraphStore::new_for_testing()
    }

    #[test]
    fn find_nodes_by_label_on_returns_parsed_node_ids() {
        let s = store();
        let mut conn = ScriptedConn::default().ok_scard(2).ok_smembers(&["1", "2"]);
        let ids = s.find_nodes_by_label_on(&mut conn, "g", "Person").unwrap();
        assert_eq!(ids, vec![NodeId(1), NodeId(2)]);
    }

    #[test]
    fn find_nodes_by_label_on_returns_empty_when_set_empty() {
        let s = store();
        let mut conn = ScriptedConn::default().ok_scard(0).ok_smembers(&[]);
        let ids = s.find_nodes_by_label_on(&mut conn, "g", "Person").unwrap();
        assert!(ids.is_empty());
    }

    #[test]
    fn find_nodes_by_label_on_rejects_non_numeric_member_with_serialization_error() {
        let s = store();
        let mut conn = ScriptedConn::default().ok_scard(1).ok_smembers(&["bogus"]);
        let err = s
            .find_nodes_by_label_on(&mut conn, "g", "Person")
            .unwrap_err();
        assert!(matches!(err, StorageError::Serialization { .. }));
    }

    #[test]
    fn find_nodes_by_label_on_propagates_backend_error() {
        let s = store();
        let mut conn = ScriptedConn::default().err("SCARD");
        let err = s
            .find_nodes_by_label_on(&mut conn, "g", "Person")
            .unwrap_err();
        assert!(matches!(err, StorageError::Backend { .. }));
    }

    #[test]
    fn find_edges_by_type_on_returns_parsed_edge_ids() {
        let s = store();
        let mut conn = ScriptedConn::default()
            .ok_scard(3)
            .ok_smembers(&["10", "11", "12"]);
        let ids = s.find_edges_by_type_on(&mut conn, "g", "KNOWS").unwrap();
        assert_eq!(ids, vec![EdgeId(10), EdgeId(11), EdgeId(12)]);
    }

    #[test]
    fn find_edges_by_type_on_rejects_non_numeric_member_with_serialization_error() {
        let s = store();
        let mut conn = ScriptedConn::default().ok_scard(1).ok_smembers(&["nope"]);
        let err = s
            .find_edges_by_type_on(&mut conn, "g", "KNOWS")
            .unwrap_err();
        assert!(matches!(err, StorageError::Serialization { .. }));
    }

    #[test]
    fn find_edges_by_type_on_propagates_backend_error() {
        let s = store();
        let mut conn = ScriptedConn::default().err("SCARD");
        let err = s
            .find_edges_by_type_on(&mut conn, "g", "KNOWS")
            .unwrap_err();
        assert!(matches!(err, StorageError::Backend { .. }));
    }
}
