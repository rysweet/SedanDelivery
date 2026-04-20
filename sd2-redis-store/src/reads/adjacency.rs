// Adjacency reads: out/in edges and the batched adjacency lookup.

use cypher_core::error::StorageError;
use cypher_core::storage::{AdjEntry, EdgeId, NodeId};
use redis::ConnectionLike;

use crate::keys::{key_in, key_out};
use crate::RedisGraphStore;

use super::helpers::{smembers_or_scan, smembers_threshold, sscan_all_members};

impl RedisGraphStore {
    pub(crate) fn out_edges_impl(
        &self,
        graph: &str,
        node_id: NodeId,
    ) -> Result<Vec<AdjEntry>, StorageError> {
        let mut conn = self.route_read_for_graph(graph)?;
        self.out_edges_on(conn.as_mut(), graph, node_id)
    }

    pub(crate) fn in_edges_impl(
        &self,
        graph: &str,
        node_id: NodeId,
    ) -> Result<Vec<AdjEntry>, StorageError> {
        let mut conn = self.route_read_for_graph(graph)?;
        self.in_edges_on(conn.as_mut(), graph, node_id)
    }

    pub(crate) fn out_edges_on(
        &self,
        conn: &mut dyn ConnectionLike,
        graph: &str,
        node_id: NodeId,
    ) -> Result<Vec<AdjEntry>, StorageError> {
        let members = smembers_or_scan(
            conn,
            &self.kp(key_out(graph, node_id.0)),
            smembers_threshold(),
        )?;
        Ok(Self::parse_adj_members(members))
    }

    pub(crate) fn in_edges_on(
        &self,
        conn: &mut dyn ConnectionLike,
        graph: &str,
        node_id: NodeId,
    ) -> Result<Vec<AdjEntry>, StorageError> {
        let members = smembers_or_scan(
            conn,
            &self.kp(key_in(graph, node_id.0)),
            smembers_threshold(),
        )?;
        Ok(Self::parse_adj_members(members))
    }

    pub(crate) fn out_edges_by_type_impl(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        let all = self.out_edges_impl(graph, node_id)?;
        if rel_types.is_empty() {
            return Ok(all);
        }
        Ok(all
            .into_iter()
            .filter(|e| rel_types.contains(&e.rel_type.as_str()))
            .collect())
    }

    #[allow(dead_code)]
    pub(crate) fn out_edges_by_type_on(
        &self,
        conn: &mut dyn ConnectionLike,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        let all = self.out_edges_on(conn, graph, node_id)?;
        if rel_types.is_empty() {
            return Ok(all);
        }
        Ok(all
            .into_iter()
            .filter(|e| rel_types.contains(&e.rel_type.as_str()))
            .collect())
    }

    #[allow(dead_code)]
    pub(crate) fn in_edges_by_type_on(
        &self,
        conn: &mut dyn ConnectionLike,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        let all = self.in_edges_on(conn, graph, node_id)?;
        if rel_types.is_empty() {
            return Ok(all);
        }
        Ok(all
            .into_iter()
            .filter(|e| rel_types.contains(&e.rel_type.as_str()))
            .collect())
    }

    pub(crate) fn in_edges_by_type_impl(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        let all = self.in_edges_impl(graph, node_id)?;
        if rel_types.is_empty() {
            return Ok(all);
        }
        Ok(all
            .into_iter()
            .filter(|e| rel_types.contains(&e.rel_type.as_str()))
            .collect())
    }

    pub(crate) fn out_edges_by_type_batch_impl(
        &self,
        graph: &str,
        node_ids: &[NodeId],
        rel_types: &[&str],
    ) -> Result<Vec<Vec<AdjEntry>>, StorageError> {
        self.adjacency_batch_impl(graph, node_ids, false, rel_types)
    }

    pub(crate) fn in_edges_by_type_batch_impl(
        &self,
        graph: &str,
        node_ids: &[NodeId],
        rel_types: &[&str],
    ) -> Result<Vec<Vec<AdjEntry>>, StorageError> {
        self.adjacency_batch_impl(graph, node_ids, true, rel_types)
    }

    fn adjacency_batch_impl(
        &self,
        graph: &str,
        node_ids: &[NodeId],
        incoming: bool,
        rel_types: &[&str],
    ) -> Result<Vec<Vec<AdjEntry>>, StorageError> {
        if node_ids.is_empty() {
            return Ok(vec![]);
        }

        let mut conn = self.route_read_for_graph(graph)?;
        let threshold = smembers_threshold();
        let keys: Vec<String> = node_ids
            .iter()
            .map(|node_id| {
                let key = if incoming {
                    key_in(graph, node_id.0)
                } else {
                    key_out(graph, node_id.0)
                };
                self.kp(key)
            })
            .collect();

        let mut scard_pipe = conn.pipeline();
        for key in &keys {
            scard_pipe.cmd("SCARD").arg(key);
        }
        let cards: Vec<u64> = scard_pipe
            .query(&mut conn)
            .map_err(crate::backend_storage_error)?;

        let mut results = vec![Vec::new(); node_ids.len()];
        let mut small_indexes = Vec::new();
        let mut small_pipe = conn.pipeline();
        for (idx, (key, card)) in keys.iter().zip(&cards).enumerate() {
            if *card <= threshold {
                small_indexes.push(idx);
                small_pipe.cmd("SMEMBERS").arg(key);
            }
        }

        if !small_indexes.is_empty() {
            let raw_batches: Vec<Vec<String>> = small_pipe
                .query(&mut conn)
                .map_err(crate::backend_storage_error)?;
            for (batch_idx, row_idx) in small_indexes.into_iter().enumerate() {
                let mut entries = Self::parse_adj_members(raw_batches[batch_idx].clone());
                if !rel_types.is_empty() {
                    entries.retain(|entry| rel_types.contains(&entry.rel_type.as_str()));
                }
                results[row_idx] = entries;
            }
        }

        for (idx, (key, card)) in keys.iter().zip(&cards).enumerate() {
            if *card > threshold {
                let mut entries =
                    Self::parse_adj_members(sscan_all_members(conn.as_mut(), key, *card as usize)?);
                if !rel_types.is_empty() {
                    entries.retain(|entry| rel_types.contains(&entry.rel_type.as_str()));
                }
                results[idx] = entries;
            }
        }

        Ok(results)
    }

    pub(crate) fn out_edge_ids_impl(
        &self,
        graph: &str,
        node_id: NodeId,
    ) -> Result<Vec<EdgeId>, StorageError> {
        Ok(self
            .out_edges_impl(graph, node_id)?
            .into_iter()
            .map(|e| e.edge_id)
            .collect())
    }

    pub(crate) fn in_edge_ids_impl(
        &self,
        graph: &str,
        node_id: NodeId,
    ) -> Result<Vec<EdgeId>, StorageError> {
        Ok(self
            .in_edges_impl(graph, node_id)?
            .into_iter()
            .map(|e| e.edge_id)
            .collect())
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

    fn encoded(edge_id: u64, neighbor: u64, rt: &str) -> String {
        format!("{edge_id}:{neighbor}:{rt}")
    }

    #[test]
    fn out_edges_on_returns_parsed_entries() {
        let s = store();
        let mut conn = ScriptedConn::default()
            .ok_scard(2)
            .ok_smembers(&[&encoded(1, 7, "KNOWS"), &encoded(2, 8, "LIKES")]);
        let edges = s.out_edges_on(&mut conn, "g", NodeId(1)).unwrap();
        assert_eq!(edges.len(), 2);
        assert_eq!(edges[0].rel_type, "KNOWS");
        assert_eq!(edges[0].edge_id, EdgeId(1));
        assert_eq!(edges[0].neighbor_id, NodeId(7));
    }

    #[test]
    fn out_edges_on_returns_empty_for_isolated_node() {
        let s = store();
        let mut conn = ScriptedConn::default().ok_scard(0).ok_smembers(&[]);
        let edges = s.out_edges_on(&mut conn, "g", NodeId(1)).unwrap();
        assert!(edges.is_empty());
    }

    #[test]
    fn out_edges_on_propagates_backend_error() {
        let s = store();
        let mut conn = ScriptedConn::default().err("SCARD");
        let err = s.out_edges_on(&mut conn, "g", NodeId(1)).unwrap_err();
        assert!(matches!(err, StorageError::Backend { .. }));
    }

    #[test]
    fn in_edges_on_returns_parsed_entries() {
        let s = store();
        let mut conn = ScriptedConn::default()
            .ok_scard(1)
            .ok_smembers(&[&encoded(9, 5, "KNOWS")]);
        let edges = s.in_edges_on(&mut conn, "g", NodeId(1)).unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].neighbor_id, NodeId(5));
        assert_eq!(edges[0].edge_id, EdgeId(9));
    }

    #[test]
    fn in_edges_on_propagates_backend_error() {
        let s = store();
        let mut conn = ScriptedConn::default().err("SCARD");
        let err = s.in_edges_on(&mut conn, "g", NodeId(1)).unwrap_err();
        assert!(matches!(err, StorageError::Backend { .. }));
    }

    #[test]
    fn out_edges_by_type_on_returns_all_when_filter_empty() {
        let s = store();
        let mut conn = ScriptedConn::default()
            .ok_scard(2)
            .ok_smembers(&[&encoded(1, 2, "A"), &encoded(2, 3, "B")]);
        let edges = s
            .out_edges_by_type_on(&mut conn, "g", NodeId(1), &[])
            .unwrap();
        assert_eq!(edges.len(), 2);
    }

    #[test]
    fn out_edges_by_type_on_filters_by_type() {
        let s = store();
        let mut conn = ScriptedConn::default().ok_scard(3).ok_smembers(&[
            &encoded(1, 2, "KNOWS"),
            &encoded(2, 3, "LIKES"),
            &encoded(3, 4, "KNOWS"),
        ]);
        let edges = s
            .out_edges_by_type_on(&mut conn, "g", NodeId(1), &["KNOWS"])
            .unwrap();
        assert_eq!(edges.len(), 2);
        assert!(edges.iter().all(|e| e.rel_type == "KNOWS"));
    }

    #[test]
    fn out_edges_by_type_on_returns_empty_when_no_type_matches() {
        let s = store();
        let mut conn = ScriptedConn::default()
            .ok_scard(1)
            .ok_smembers(&[&encoded(1, 2, "KNOWS")]);
        let edges = s
            .out_edges_by_type_on(&mut conn, "g", NodeId(1), &["MISSING"])
            .unwrap();
        assert!(edges.is_empty());
    }
}
