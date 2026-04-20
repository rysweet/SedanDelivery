//! Read-only scan helpers used by `audit_graph_metadata` to detect stale
//! Redis-only graph metadata before any repair lock is acquired.

use cypher_core::error::StorageError;
use redis::ConnectionLike;

use crate::keys::{
    key_edge_seq, key_edges_set, key_indexes, key_label, key_label_names, key_node_seq,
    key_nodes_set, key_propidx_manifest, key_propidx_prefix, key_reltype, key_reltype_names,
};
use crate::RedisGraphStore;

impl RedisGraphStore {
    pub(super) fn graph_registry_state_on(
        &self,
        conn: &mut dyn ConnectionLike,
        graph: &str,
    ) -> Result<(bool, bool), StorageError> {
        let registered: bool = redis::cmd("SISMEMBER")
            .arg(self.key_graphs())
            .arg(graph)
            .query(conn)
            .map_err(crate::backend_storage_error)?;
        let deleting = self.graph_is_deleting_on(conn, graph)?;
        Ok((registered, deleting))
    }

    pub(super) fn read_set_members(
        &self,
        conn: &mut dyn ConnectionLike,
        key: &str,
    ) -> Result<Vec<String>, StorageError> {
        redis::cmd("SMEMBERS")
            .arg(key)
            .query(conn)
            .map_err(crate::backend_storage_error)
    }

    pub(super) fn collect_stale_labels_on(
        &self,
        conn: &mut dyn ConnectionLike,
        graph: &str,
        members: &[String],
    ) -> Result<Vec<String>, StorageError> {
        let mut stale = Vec::new();
        for label in members {
            let count: u64 = redis::cmd("SCARD")
                .arg(self.kp(key_label(graph, label)))
                .query(conn)
                .map_err(crate::backend_storage_error)?;
            if count == 0 {
                stale.push(label.clone());
            }
        }
        Ok(stale)
    }

    pub(super) fn collect_stale_reltypes_on(
        &self,
        conn: &mut dyn ConnectionLike,
        graph: &str,
        members: &[String],
    ) -> Result<Vec<String>, StorageError> {
        let mut stale = Vec::new();
        for reltype in members {
            let count: u64 = redis::cmd("SCARD")
                .arg(self.kp(key_reltype(graph, reltype)))
                .query(conn)
                .map_err(crate::backend_storage_error)?;
            if count == 0 {
                stale.push(reltype.clone());
            }
        }
        Ok(stale)
    }

    pub(super) fn collect_stale_propidx_on(
        &self,
        conn: &mut dyn ConnectionLike,
        graph: &str,
        members: &[String],
    ) -> Result<Vec<String>, StorageError> {
        let mut stale = Vec::new();
        for member in members {
            if !self.propidx_member_matches_graph(graph, member) {
                stale.push(member.clone());
                continue;
            }

            let exists: i64 = redis::cmd("EXISTS")
                .arg(member)
                .query(conn)
                .map_err(crate::backend_storage_error)?;
            if exists == 0 {
                stale.push(member.clone());
            }
        }
        Ok(stale)
    }

    pub(super) fn graph_cleanup_roots_absent_on(
        &self,
        conn: &mut dyn ConnectionLike,
        graph: &str,
    ) -> Result<bool, StorageError> {
        for key in [
            self.kp(key_node_seq(graph)),
            self.kp(key_edge_seq(graph)),
            self.kp(key_nodes_set(graph)),
            self.kp(key_edges_set(graph)),
            self.kp(key_indexes(graph)),
            self.kp(key_label_names(graph)),
            self.kp(key_reltype_names(graph)),
            self.kp(key_propidx_manifest(graph)),
        ] {
            let exists: i64 = redis::cmd("EXISTS")
                .arg(&key)
                .query(conn)
                .map_err(crate::backend_storage_error)?;
            if exists > 0 {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(super) fn propidx_member_matches_graph(&self, graph: &str, member: &str) -> bool {
        member.starts_with(&self.kp(key_propidx_prefix(graph)))
    }
}

#[cfg(test)]
mod tests {
    //! Unit tests for the read-only audit scan helpers using a `ConnectionLike`
    //! mock. The lock-bound recheck helpers in `repair.rs` are exercised by
    //! the live-Redis suite in `tests/metadata_audit.rs`.

    use super::*;
    use crate::keys::{key_propidx_prefix, KEY_GRAPHS};
    use redis::{Arg, Cmd, RedisError, RedisResult, Value as RedisValue};
    use std::collections::VecDeque;

    /// Mock that drives any combination of SISMEMBER/EXISTS/SMEMBERS/SCARD.
    struct ScriptedAuditConn {
        sismember: VecDeque<RedisResult<RedisValue>>,
        exists: VecDeque<RedisResult<RedisValue>>,
        smembers: VecDeque<RedisResult<RedisValue>>,
        scard: VecDeque<RedisResult<RedisValue>>,
    }
    impl ScriptedAuditConn {
        fn new() -> Self {
            Self {
                sismember: VecDeque::new(),
                exists: VecDeque::new(),
                smembers: VecDeque::new(),
                scard: VecDeque::new(),
            }
        }
        fn sismember(mut self, b: bool) -> Self {
            self.sismember
                .push_back(Ok(RedisValue::Int(if b { 1 } else { 0 })));
            self
        }
        fn exists(mut self, n: i64) -> Self {
            self.exists.push_back(Ok(RedisValue::Int(n)));
            self
        }
        fn exists_err(mut self) -> Self {
            self.exists
                .push_back(Err(RedisError::from((redis::ErrorKind::Io, "exists"))));
            self
        }
        fn smembers(mut self, items: &[&str]) -> Self {
            self.smembers
                .push_back(Ok(RedisValue::Array(items.iter().map(bulk).collect())));
            self
        }
        fn smembers_err(mut self) -> Self {
            self.smembers
                .push_back(Err(RedisError::from((redis::ErrorKind::Io, "smembers"))));
            self
        }
        fn scard(mut self, n: i64) -> Self {
            self.scard.push_back(Ok(RedisValue::Int(n)));
            self
        }
        fn scard_err(mut self) -> Self {
            self.scard
                .push_back(Err(RedisError::from((redis::ErrorKind::Io, "scard"))));
            self
        }
    }
    fn bulk(s: &&str) -> RedisValue {
        RedisValue::BulkString(s.as_bytes().to_vec())
    }
    fn cmd_name(cmd: &Cmd) -> String {
        cmd.args_iter()
            .next()
            .map(|a| match a {
                Arg::Simple(b) => String::from_utf8_lossy(b).to_string(),
                _ => String::new(),
            })
            .unwrap_or_default()
    }
    impl ConnectionLike for ScriptedAuditConn {
        fn req_packed_command(&mut self, _cmd: &[u8]) -> RedisResult<RedisValue> {
            panic!("packed_command not expected");
        }
        fn req_packed_commands(
            &mut self,
            _cmd: &[u8],
            _offset: usize,
            _count: usize,
        ) -> RedisResult<Vec<RedisValue>> {
            panic!("packed_commands not expected");
        }
        fn req_command(&mut self, cmd: &Cmd) -> RedisResult<RedisValue> {
            match cmd_name(cmd).as_str() {
                "SISMEMBER" => self.sismember.pop_front().expect("SISMEMBER queue empty"),
                "EXISTS" => self.exists.pop_front().expect("EXISTS queue empty"),
                "SMEMBERS" => self.smembers.pop_front().expect("SMEMBERS queue empty"),
                "SCARD" => self.scard.pop_front().expect("SCARD queue empty"),
                other => panic!("unexpected command {other}"),
            }
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
    fn graph_registry_state_on_reports_registered_and_not_deleting() {
        let s = store();
        let mut conn = ScriptedAuditConn::new().sismember(true).exists(0);
        let (reg, deleting) = s.graph_registry_state_on(&mut conn, "g").unwrap();
        assert!(reg);
        assert!(!deleting);
    }

    #[test]
    fn graph_registry_state_on_reports_unregistered_but_deleting() {
        let s = store();
        let mut conn = ScriptedAuditConn::new().sismember(false).exists(1);
        let (reg, deleting) = s.graph_registry_state_on(&mut conn, "g").unwrap();
        assert!(!reg);
        assert!(deleting);
    }

    #[test]
    fn read_set_members_returns_smembers_payload() {
        let s = store();
        let mut conn = ScriptedAuditConn::new().smembers(&["a", "b"]);
        let members = s.read_set_members(&mut conn, "key").unwrap();
        assert_eq!(members, vec!["a".to_string(), "b".into()]);
    }

    #[test]
    fn read_set_members_propagates_smembers_error() {
        let s = store();
        let mut conn = ScriptedAuditConn::new().smembers_err();
        let err = s.read_set_members(&mut conn, "key").unwrap_err();
        assert!(matches!(err, StorageError::Backend { .. }));
    }

    #[test]
    fn collect_stale_labels_on_empty_set_marks_label_stale() {
        let s = store();
        let mut conn = ScriptedAuditConn::new().scard(0).scard(3);
        let stale = s
            .collect_stale_labels_on(&mut conn, "g", &["Ghost".into(), "Live".into()])
            .unwrap();
        assert_eq!(stale, vec!["Ghost".to_string()]);
    }

    #[test]
    fn collect_stale_labels_on_propagates_error() {
        let s = store();
        let mut conn = ScriptedAuditConn::new().scard_err();
        let err = s
            .collect_stale_labels_on(&mut conn, "g", &["X".into()])
            .unwrap_err();
        assert!(matches!(err, StorageError::Backend { .. }));
    }

    #[test]
    fn collect_stale_labels_on_empty_input_is_noop() {
        let s = store();
        let mut conn = ScriptedAuditConn::new();
        let stale = s.collect_stale_labels_on(&mut conn, "g", &[]).unwrap();
        assert!(stale.is_empty());
    }

    #[test]
    fn collect_stale_reltypes_on_marks_empty_reltypes() {
        let s = store();
        let mut conn = ScriptedAuditConn::new().scard(2).scard(0);
        let stale = s
            .collect_stale_reltypes_on(&mut conn, "g", &["LIVE".into(), "GHOST".into()])
            .unwrap();
        assert_eq!(stale, vec!["GHOST".to_string()]);
    }

    #[test]
    fn collect_stale_reltypes_on_propagates_error() {
        let s = store();
        let mut conn = ScriptedAuditConn::new().scard_err();
        let err = s
            .collect_stale_reltypes_on(&mut conn, "g", &["X".into()])
            .unwrap_err();
        assert!(matches!(err, StorageError::Backend { .. }));
    }

    #[test]
    fn collect_stale_propidx_on_marks_wrong_graph_member_stale_without_query() {
        let s = store();
        // member doesn't start with the graph's propidx prefix → stale
        // without an EXISTS query (no EXISTS queued).
        let mut conn = ScriptedAuditConn::new();
        let bogus = "G:{other}:propidx:Foo\x00bar".to_string();
        let stale = s
            .collect_stale_propidx_on(&mut conn, "g", std::slice::from_ref(&bogus))
            .unwrap();
        assert_eq!(stale, vec![bogus]);
    }

    #[test]
    fn collect_stale_propidx_on_marks_existing_propidx_member_alive() {
        let s = store();
        let prefix = key_propidx_prefix("g");
        let member = format!("{prefix}Person\x00name\x00s:Alice");
        let mut conn = ScriptedAuditConn::new().exists(1);
        let stale = s
            .collect_stale_propidx_on(&mut conn, "g", &[member])
            .unwrap();
        assert!(stale.is_empty());
    }

    #[test]
    fn collect_stale_propidx_on_marks_missing_propidx_member_stale() {
        let s = store();
        let prefix = key_propidx_prefix("g");
        let member = format!("{prefix}Person\x00name\x00s:Bob");
        let mut conn = ScriptedAuditConn::new().exists(0);
        let stale = s
            .collect_stale_propidx_on(&mut conn, "g", std::slice::from_ref(&member))
            .unwrap();
        assert_eq!(stale, vec![member]);
    }

    #[test]
    fn collect_stale_propidx_on_propagates_exists_error() {
        let s = store();
        let prefix = key_propidx_prefix("g");
        let member = format!("{prefix}X\x00y\x00s:z");
        let mut conn = ScriptedAuditConn::new().exists_err();
        let err = s
            .collect_stale_propidx_on(&mut conn, "g", &[member])
            .unwrap_err();
        assert!(matches!(err, StorageError::Backend { .. }));
    }

    #[test]
    fn graph_cleanup_roots_absent_on_returns_true_when_all_keys_missing() {
        let s = store();
        // 8 EXISTS calls all returning 0.
        let mut conn = ScriptedAuditConn::new();
        for _ in 0..8 {
            conn = conn.exists(0);
        }
        assert!(s.graph_cleanup_roots_absent_on(&mut conn, "g").unwrap());
    }

    #[test]
    fn graph_cleanup_roots_absent_on_returns_false_on_first_present_key() {
        let s = store();
        // First EXISTS returns 1 → short-circuit returns false.
        let mut conn = ScriptedAuditConn::new().exists(1);
        assert!(!s.graph_cleanup_roots_absent_on(&mut conn, "g").unwrap());
    }

    #[test]
    fn graph_cleanup_roots_absent_on_propagates_exists_error() {
        let s = store();
        let mut conn = ScriptedAuditConn::new().exists_err();
        let err = s.graph_cleanup_roots_absent_on(&mut conn, "g").unwrap_err();
        assert!(matches!(err, StorageError::Backend { .. }));
    }

    #[test]
    fn propidx_member_matches_graph_recognizes_own_prefix() {
        let s = store();
        let prefix = key_propidx_prefix("g");
        let own = format!("{prefix}Person\x00name\x00s:A");
        let other = "G:{other}:propidx:X".to_string();
        assert!(s.propidx_member_matches_graph("g", &own));
        assert!(!s.propidx_member_matches_graph("g", &other));
    }

    // Belt-and-suspenders sanity check: KEY_GRAPHS constant is non-empty,
    // since registry tests already cover its prefixed form.
    #[test]
    fn key_graphs_constant_is_non_empty() {
        assert!(!KEY_GRAPHS.is_empty());
    }
}
