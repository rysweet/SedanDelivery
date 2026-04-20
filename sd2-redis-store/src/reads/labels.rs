// Label and relationship-type registry reads (active names with empty-set filtering).

use cypher_core::error::StorageError;
use redis::ConnectionLike;

use crate::connection::RedisConn;
use crate::keys::{
    key_edge, key_edges_set, key_label, key_label_names, key_node, key_nodes_set, key_reltype,
    key_reltype_names,
};
use crate::RedisGraphStore;

use super::helpers::{smembers_or_scan, smembers_threshold};

impl RedisGraphStore {
    pub(crate) fn active_label_names(
        &self,
        conn: &mut RedisConn,
        graph: &str,
    ) -> Result<Vec<String>, StorageError> {
        self.active_label_names_on(conn.as_mut(), graph)
    }

    pub(crate) fn active_reltype_names(
        &self,
        conn: &mut RedisConn,
        graph: &str,
    ) -> Result<Vec<String>, StorageError> {
        self.active_reltype_names_on(conn.as_mut(), graph)
    }

    /// Cluster-safe active-label discovery using individual SCARD/HMGET calls.
    ///
    /// Uses individual SCARD calls instead of pipelines so the helper accepts a
    /// generic `&mut dyn ConnectionLike` (mockable for unit tests) and remains
    /// safe on `ClusterConnection`, which silently mishandles standalone
    /// `redis::pipe()` queries even when all keys share a hash tag (#1276).
    /// For typical label registries (≤100 entries) the extra round-trips are
    /// negligible.
    pub(crate) fn active_label_names_on(
        &self,
        conn: &mut dyn ConnectionLike,
        graph: &str,
    ) -> Result<Vec<String>, StorageError> {
        let registry_key = self.kp(key_label_names(graph));
        let labels: Vec<String> = redis::cmd("SMEMBERS")
            .arg(&registry_key)
            .query(conn)
            .map_err(crate::backend_storage_error)?;
        if !labels.is_empty() {
            let mut active: Vec<String> = Vec::with_capacity(labels.len());
            for label in &labels {
                let count: u64 = redis::cmd("SCARD")
                    .arg(self.kp(key_label(graph, label)))
                    .query(conn)
                    .map_err(crate::backend_storage_error)?;
                if count > 0 {
                    active.push(label.clone());
                }
            }
            active.sort_unstable();
            if !active.is_empty() {
                return Ok(active);
            }
        }

        let node_ids =
            smembers_or_scan(conn, &self.kp(key_nodes_set(graph)), smembers_threshold())?;
        if node_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut discovered = std::collections::HashSet::new();
        for nid_str in &node_ids {
            let Ok(nid) = nid_str.parse::<u64>() else {
                continue;
            };
            let json: Option<String> = redis::cmd("HGET")
                .arg(self.kp(key_node(graph, nid)))
                .arg("labels")
                .query(conn)
                .map_err(crate::backend_storage_error)?;
            if let Some(json) = json {
                let parsed: Vec<String> =
                    serde_json::from_str(&json).map_err(|e| StorageError::Serialization {
                        msg: format!("labels registry fallback: {e}"),
                    })?;
                discovered.extend(parsed);
            }
        }

        let mut labels: Vec<String> = discovered.into_iter().collect();
        labels.sort_unstable();
        Ok(labels)
    }

    /// Cluster-safe active-reltype discovery using individual SCARD/HGET calls.
    /// See [`Self::active_label_names_on`] for the rationale.
    pub(crate) fn active_reltype_names_on(
        &self,
        conn: &mut dyn ConnectionLike,
        graph: &str,
    ) -> Result<Vec<String>, StorageError> {
        let registry_key = self.kp(key_reltype_names(graph));
        let reltypes: Vec<String> = redis::cmd("SMEMBERS")
            .arg(&registry_key)
            .query(conn)
            .map_err(crate::backend_storage_error)?;
        if !reltypes.is_empty() {
            let mut active: Vec<String> = Vec::with_capacity(reltypes.len());
            for reltype in &reltypes {
                let count: u64 = redis::cmd("SCARD")
                    .arg(self.kp(key_reltype(graph, reltype)))
                    .query(conn)
                    .map_err(crate::backend_storage_error)?;
                if count > 0 {
                    active.push(reltype.clone());
                }
            }
            active.sort_unstable();
            if !active.is_empty() {
                return Ok(active);
            }
        }

        let edge_ids =
            smembers_or_scan(conn, &self.kp(key_edges_set(graph)), smembers_threshold())?;
        if edge_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut discovered = std::collections::HashSet::new();
        for eid_str in &edge_ids {
            let Ok(eid) = eid_str.parse::<u64>() else {
                continue;
            };
            let v: Option<String> = redis::cmd("HGET")
                .arg(self.kp(key_edge(graph, eid)))
                .arg("rel_type")
                .query(conn)
                .map_err(crate::backend_storage_error)?;
            if let Some(rel) = v {
                discovered.insert(rel);
            }
        }

        let mut reltypes: Vec<String> = discovered.into_iter().collect();
        reltypes.sort_unstable();
        Ok(reltypes)
    }
}

#[cfg(test)]
mod tests {
    //! Mock-driven coverage for active-label/reltype discovery. Each test
    //! drives the `_on` helper via a scripted `ConnectionLike` so the body
    //! exercises without a live Redis.

    use super::*;
    use redis::{Arg, Cmd, RedisError, RedisResult, Value as RedisValue};
    use std::collections::VecDeque;

    #[derive(Default)]
    struct ScriptedConn {
        responses: VecDeque<(String, RedisResult<RedisValue>)>,
    }

    impl ScriptedConn {
        fn push(mut self, name: &str, v: RedisResult<RedisValue>) -> Self {
            self.responses.push_back((name.to_string(), v));
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
        fn ok_hget(self, value: Option<&str>) -> Self {
            self.push(
                "HGET",
                Ok(match value {
                    Some(s) => RedisValue::BulkString(s.as_bytes().to_vec()),
                    None => RedisValue::Nil,
                }),
            )
        }
        fn err(self, name: &str) -> Self {
            self.push(
                name,
                Err(RedisError::from((redis::ErrorKind::Io, "scripted-error"))),
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
            assert_eq!(
                name, expected,
                "unexpected command: got {name}, want {expected}"
            );
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

    // ── active_label_names_on ────────────────────────────────────────────

    #[test]
    fn active_label_names_returns_active_subset_sorted() {
        let s = store();
        let mut conn = ScriptedConn::default()
            .ok_smembers(&["Person", "Movie", "Ghost"])
            .ok_scard(3) // Person → live
            .ok_scard(0) // Movie → empty
            .ok_scard(2); // Ghost → live
        let labels = s.active_label_names_on(&mut conn, "g").unwrap();
        assert_eq!(labels, vec!["Ghost".to_string(), "Person".into()]);
    }

    #[test]
    fn active_label_names_returns_empty_when_registry_empty_and_no_nodes() {
        let s = store();
        let mut conn = ScriptedConn::default()
            .ok_smembers(&[]) // registry empty
            .ok_scard(0) // smembers_or_scan: card
            .ok_smembers(&[]); // smembers_or_scan: members
        let labels = s.active_label_names_on(&mut conn, "g").unwrap();
        assert!(labels.is_empty());
    }

    #[test]
    fn active_label_names_falls_back_to_node_scan_when_all_label_sets_empty() {
        let s = store();
        let mut conn = ScriptedConn::default()
            .ok_smembers(&["StaleA"])
            .ok_scard(0) // StaleA empty triggers fallback
            .ok_scard(2) // smembers_or_scan SCARD on nodes set
            .ok_smembers(&["1", "2"]) // smembers on nodes set
            .ok_hget(Some("[\"Person\",\"Manager\"]"))
            .ok_hget(Some("[\"Person\"]"));
        let labels = s.active_label_names_on(&mut conn, "g").unwrap();
        assert_eq!(labels, vec!["Manager".to_string(), "Person".into()]);
    }

    #[test]
    fn active_label_names_skips_non_numeric_node_ids_in_fallback() {
        let s = store();
        let mut conn = ScriptedConn::default()
            .ok_smembers(&[]) // registry empty
            .ok_scard(2) // nodes set card
            .ok_smembers(&["bad", "7"]) // bad → skipped, 7 → HGET
            .ok_hget(Some("[\"Movie\"]"));
        let labels = s.active_label_names_on(&mut conn, "g").unwrap();
        assert_eq!(labels, vec!["Movie".to_string()]);
    }

    #[test]
    fn active_label_names_propagates_smembers_error() {
        let s = store();
        let mut conn = ScriptedConn::default().err("SMEMBERS");
        let err = s.active_label_names_on(&mut conn, "g").unwrap_err();
        assert!(matches!(err, StorageError::Backend { .. }));
    }

    #[test]
    fn active_label_names_propagates_scard_error_during_filtering() {
        let s = store();
        let mut conn = ScriptedConn::default()
            .ok_smembers(&["Person"])
            .err("SCARD");
        let err = s.active_label_names_on(&mut conn, "g").unwrap_err();
        assert!(matches!(err, StorageError::Backend { .. }));
    }

    #[test]
    fn active_label_names_propagates_hget_error_in_fallback() {
        let s = store();
        let mut conn = ScriptedConn::default()
            .ok_smembers(&[])
            .ok_scard(1)
            .ok_smembers(&["1"])
            .err("HGET");
        let err = s.active_label_names_on(&mut conn, "g").unwrap_err();
        assert!(matches!(err, StorageError::Backend { .. }));
    }

    #[test]
    fn active_label_names_surfaces_serialization_error_for_bad_labels_json() {
        let s = store();
        let mut conn = ScriptedConn::default()
            .ok_smembers(&[])
            .ok_scard(1)
            .ok_smembers(&["1"])
            .ok_hget(Some("not-json"));
        let err = s.active_label_names_on(&mut conn, "g").unwrap_err();
        assert!(matches!(err, StorageError::Serialization { .. }));
    }

    #[test]
    fn active_label_names_skips_nodes_with_missing_labels_field() {
        let s = store();
        let mut conn = ScriptedConn::default()
            .ok_smembers(&[])
            .ok_scard(2)
            .ok_smembers(&["1", "2"])
            .ok_hget(None) // node 1 missing labels
            .ok_hget(Some("[\"Tag\"]"));
        let labels = s.active_label_names_on(&mut conn, "g").unwrap();
        assert_eq!(labels, vec!["Tag".to_string()]);
    }

    // ── active_reltype_names_on ──────────────────────────────────────────

    #[test]
    fn active_reltype_names_returns_active_subset_sorted() {
        let s = store();
        let mut conn = ScriptedConn::default()
            .ok_smembers(&["KNOWS", "EMPTY", "WORKS_FOR"])
            .ok_scard(5)
            .ok_scard(0)
            .ok_scard(1);
        let rels = s.active_reltype_names_on(&mut conn, "g").unwrap();
        assert_eq!(rels, vec!["KNOWS".to_string(), "WORKS_FOR".into()]);
    }

    #[test]
    fn active_reltype_names_returns_empty_when_registry_and_edges_empty() {
        let s = store();
        let mut conn = ScriptedConn::default()
            .ok_smembers(&[])
            .ok_scard(0)
            .ok_smembers(&[]);
        let rels = s.active_reltype_names_on(&mut conn, "g").unwrap();
        assert!(rels.is_empty());
    }

    #[test]
    fn active_reltype_names_falls_back_to_edge_scan_when_registry_stale() {
        let s = store();
        let mut conn = ScriptedConn::default()
            .ok_smembers(&["GHOST"])
            .ok_scard(0) // GHOST empty → fallback
            .ok_scard(2)
            .ok_smembers(&["1", "2"])
            .ok_hget(Some("KNOWS"))
            .ok_hget(Some("KNOWS"));
        let rels = s.active_reltype_names_on(&mut conn, "g").unwrap();
        assert_eq!(rels, vec!["KNOWS".to_string()]);
    }

    #[test]
    fn active_reltype_names_skips_non_numeric_edge_ids_in_fallback() {
        let s = store();
        let mut conn = ScriptedConn::default()
            .ok_smembers(&[])
            .ok_scard(2)
            .ok_smembers(&["bogus", "9"])
            .ok_hget(Some("FRIEND"));
        let rels = s.active_reltype_names_on(&mut conn, "g").unwrap();
        assert_eq!(rels, vec!["FRIEND".to_string()]);
    }

    #[test]
    fn active_reltype_names_propagates_smembers_error() {
        let s = store();
        let mut conn = ScriptedConn::default().err("SMEMBERS");
        let err = s.active_reltype_names_on(&mut conn, "g").unwrap_err();
        assert!(matches!(err, StorageError::Backend { .. }));
    }

    #[test]
    fn active_reltype_names_propagates_scard_error_during_filtering() {
        let s = store();
        let mut conn = ScriptedConn::default().ok_smembers(&["KNOWS"]).err("SCARD");
        let err = s.active_reltype_names_on(&mut conn, "g").unwrap_err();
        assert!(matches!(err, StorageError::Backend { .. }));
    }

    #[test]
    fn active_reltype_names_propagates_hget_error_in_fallback() {
        let s = store();
        let mut conn = ScriptedConn::default()
            .ok_smembers(&[])
            .ok_scard(1)
            .ok_smembers(&["1"])
            .err("HGET");
        let err = s.active_reltype_names_on(&mut conn, "g").unwrap_err();
        assert!(matches!(err, StorageError::Backend { .. }));
    }

    #[test]
    fn active_reltype_names_skips_edges_with_missing_rel_type_field() {
        let s = store();
        let mut conn = ScriptedConn::default()
            .ok_smembers(&[])
            .ok_scard(2)
            .ok_smembers(&["1", "2"])
            .ok_hget(None)
            .ok_hget(Some("LIKES"));
        let rels = s.active_reltype_names_on(&mut conn, "g").unwrap();
        assert_eq!(rels, vec!["LIKES".to_string()]);
    }
}
