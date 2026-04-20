// Graph-registry lookups and key-prefix helpers used across the crate.

use cypher_core::error::StorageError;
use redis::ConnectionLike;

use crate::backend_storage_error;
use crate::keys::{key_graph_deleting, KEY_GRAPHS};

use super::RedisGraphStore;

impl RedisGraphStore {
    /// Check graph existence against the provided connection.
    pub(crate) fn graph_exists_on(
        &self,
        conn: &mut dyn ConnectionLike,
        g: &str,
    ) -> Result<bool, StorageError> {
        let registered: bool = redis::cmd("SISMEMBER")
            .arg(self.key_graphs())
            .arg(g)
            .query(conn)
            .map_err(backend_storage_error)?;
        if !registered {
            return Ok(false);
        }
        Ok(!self.graph_is_deleting_on(conn, g)?)
    }

    /// Return `true` while `delete_graph` cleanup is in progress for `g`.
    pub(crate) fn graph_is_deleting_on(
        &self,
        conn: &mut dyn ConnectionLike,
        g: &str,
    ) -> Result<bool, StorageError> {
        let deleting_key = self.kp(key_graph_deleting(g));
        let exists: i64 = redis::cmd("EXISTS")
            .arg(&deleting_key)
            .query(conn)
            .map_err(backend_storage_error)?;
        Ok(exists > 0)
    }

    /// Check that a graph exists on the provided connection; return error if not.
    pub(crate) fn require_graph_on(
        &self,
        conn: &mut dyn ConnectionLike,
        g: &str,
    ) -> Result<(), StorageError> {
        if !self.graph_exists_on(conn, g)? {
            return Err(StorageError::GraphNotFound {
                name: g.to_string(),
            });
        }
        Ok(())
    }

    /// Apply the configured key prefix to `key`.
    ///
    /// When `key_prefix` is empty (the default) this is a no-op clone.
    #[inline]
    pub(crate) fn kp(&self, key: String) -> String {
        if self.key_prefix.is_empty() {
            key
        } else {
            format!("{}{}", self.key_prefix, key)
        }
    }

    /// Return the prefixed graph-registry key.
    #[inline]
    pub(crate) fn key_graphs(&self) -> String {
        self.kp(KEY_GRAPHS.to_string())
    }
}

#[cfg(test)]
mod tests {
    //! Unit tests for graph-registry helpers using a `ConnectionLike` mock.

    use super::*;
    use redis::{Arg, Cmd, RedisError, RedisResult, Value as RedisValue};
    use std::collections::VecDeque;

    /// Mock that returns canned replies for the SISMEMBER/EXISTS commands
    /// issued by the registry helpers.
    struct ScriptedRegistryConn {
        sismember: VecDeque<RedisResult<RedisValue>>,
        exists: VecDeque<RedisResult<RedisValue>>,
    }

    impl ScriptedRegistryConn {
        fn new() -> Self {
            Self {
                sismember: VecDeque::new(),
                exists: VecDeque::new(),
            }
        }
        fn push_sismember(mut self, v: bool) -> Self {
            self.sismember
                .push_back(Ok(RedisValue::Int(if v { 1 } else { 0 })));
            self
        }
        fn push_sismember_err(mut self) -> Self {
            self.sismember
                .push_back(Err(RedisError::from((redis::ErrorKind::Io, "sismember"))));
            self
        }
        fn push_exists(mut self, v: i64) -> Self {
            self.exists.push_back(Ok(RedisValue::Int(v)));
            self
        }
        fn push_exists_err(mut self) -> Self {
            self.exists
                .push_back(Err(RedisError::from((redis::ErrorKind::Io, "exists"))));
            self
        }
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

    impl ConnectionLike for ScriptedRegistryConn {
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
                "SISMEMBER" => self.sismember.pop_front().expect("no SISMEMBER queued"),
                "EXISTS" => self.exists.pop_front().expect("no EXISTS queued"),
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
    fn graph_exists_on_returns_false_when_unregistered() {
        let s = store();
        let mut conn = ScriptedRegistryConn::new().push_sismember(false);
        // Unregistered short-circuits before EXISTS is ever queried.
        assert!(!s.graph_exists_on(&mut conn, "g").unwrap());
    }

    #[test]
    fn graph_exists_on_returns_true_when_registered_and_not_deleting() {
        let s = store();
        let mut conn = ScriptedRegistryConn::new()
            .push_sismember(true)
            .push_exists(0);
        assert!(s.graph_exists_on(&mut conn, "g").unwrap());
    }

    #[test]
    fn graph_exists_on_returns_false_when_registered_but_deleting() {
        let s = store();
        let mut conn = ScriptedRegistryConn::new()
            .push_sismember(true)
            .push_exists(1);
        assert!(!s.graph_exists_on(&mut conn, "g").unwrap());
    }

    #[test]
    fn graph_exists_on_propagates_sismember_error() {
        let s = store();
        let mut conn = ScriptedRegistryConn::new().push_sismember_err();
        let err = s.graph_exists_on(&mut conn, "g").unwrap_err();
        assert!(matches!(err, StorageError::Backend { .. }));
    }

    #[test]
    fn graph_exists_on_propagates_exists_error_during_delete_check() {
        let s = store();
        let mut conn = ScriptedRegistryConn::new()
            .push_sismember(true)
            .push_exists_err();
        let err = s.graph_exists_on(&mut conn, "g").unwrap_err();
        assert!(matches!(err, StorageError::Backend { .. }));
    }

    #[test]
    fn graph_is_deleting_on_true_when_marker_exists() {
        let s = store();
        let mut conn = ScriptedRegistryConn::new().push_exists(1);
        assert!(s.graph_is_deleting_on(&mut conn, "g").unwrap());
    }

    #[test]
    fn graph_is_deleting_on_false_when_marker_absent() {
        let s = store();
        let mut conn = ScriptedRegistryConn::new().push_exists(0);
        assert!(!s.graph_is_deleting_on(&mut conn, "g").unwrap());
    }

    #[test]
    fn graph_is_deleting_on_propagates_backend_error() {
        let s = store();
        let mut conn = ScriptedRegistryConn::new().push_exists_err();
        let err = s.graph_is_deleting_on(&mut conn, "g").unwrap_err();
        assert!(matches!(err, StorageError::Backend { .. }));
    }

    #[test]
    fn require_graph_on_ok_when_registered() {
        let s = store();
        let mut conn = ScriptedRegistryConn::new()
            .push_sismember(true)
            .push_exists(0);
        s.require_graph_on(&mut conn, "g").unwrap();
    }

    #[test]
    fn require_graph_on_returns_graph_not_found_when_missing() {
        let s = store();
        let mut conn = ScriptedRegistryConn::new().push_sismember(false);
        let err = s.require_graph_on(&mut conn, "g").unwrap_err();
        match err {
            StorageError::GraphNotFound { name } => assert_eq!(name, "g"),
            other => panic!("expected GraphNotFound, got {other:?}"),
        }
    }

    #[test]
    fn kp_returns_unprefixed_key_when_prefix_empty() {
        let s = store();
        assert_eq!(s.kp("foo".to_string()), "foo");
    }

    #[test]
    fn kp_prepends_prefix_when_set() {
        let mut s = store();
        s.key_prefix = "tenant:".to_string();
        assert_eq!(s.kp("foo".to_string()), "tenant:foo");
    }

    #[test]
    fn key_graphs_returns_prefixed_constant() {
        let s = store();
        let key = s.key_graphs();
        assert!(
            key.ends_with(KEY_GRAPHS) || key == KEY_GRAPHS,
            "key_graphs must end with the constant: got {key}"
        );
    }

    #[test]
    fn key_graphs_includes_prefix_when_present() {
        let mut s = store();
        s.key_prefix = "p:".to_string();
        let key = s.key_graphs();
        assert!(key.starts_with("p:"), "got {key}");
        assert!(key.ends_with(KEY_GRAPHS), "got {key}");
    }
}
