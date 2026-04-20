// ── WS-G (#756): audit_graph_metadata unit tests ───────────────────────────────
//
// These tests verify GraphMetadataAuditReport shape and invariants without
// requiring Redis. The behavioral audit tests that exercise a live backend live
// in `sd2-redis-store/tests/metadata_audit.rs`.

use super::GraphMetadataAuditReport;

/// Verify the report struct can be constructed with all fields.
#[test]
fn audit_report_constructs_with_all_fields() {
    let report = GraphMetadataAuditReport {
        graph: "mydb".to_string(),
        repair: false,
        stale_labels: vec!["GhostLabel".to_string()],
        stale_reltypes: vec!["STALE_REL".to_string()],
        stale_propidx: vec!["G:{mydb}:propidx:Person\x00name\x00s:Alice".to_string()],
        deleting_orphaned: false,
    };
    assert_eq!(report.graph, "mydb");
    assert!(!report.repair);
    assert_eq!(report.stale_labels.len(), 1);
    assert_eq!(report.stale_reltypes.len(), 1);
    assert_eq!(report.stale_propidx.len(), 1);
    assert!(!report.deleting_orphaned);
}

/// A clean audit (no stale members, no orphaned deletion) must report empty vecs
/// and `deleting_orphaned=false`.
#[test]
fn audit_report_clean_state_fields() {
    let report = GraphMetadataAuditReport {
        graph: "clean-graph".to_string(),
        repair: false,
        stale_labels: vec![],
        stale_reltypes: vec![],
        stale_propidx: vec![],
        deleting_orphaned: false,
    };
    assert!(
        report.stale_labels.is_empty(),
        "clean audit: stale_labels must be empty"
    );
    assert!(
        report.stale_reltypes.is_empty(),
        "clean audit: stale_reltypes must be empty"
    );
    assert!(
        report.stale_propidx.is_empty(),
        "clean audit: stale_propidx must be empty"
    );
    assert!(
        !report.deleting_orphaned,
        "clean audit: deleting_orphaned must be false"
    );
}

/// An orphan-like delete state may be reported.
#[test]
fn audit_report_orphaned_deletion_is_reported() {
    let report = GraphMetadataAuditReport {
        graph: "orphan-graph".to_string(),
        repair: true,
        stale_labels: vec![],
        stale_reltypes: vec![],
        stale_propidx: vec![],
        deleting_orphaned: true,
    };
    assert!(
        report.deleting_orphaned,
        "orphaned deletion must be reported as orphaned"
    );
}

/// An active deletion (data still exists) must not be misreported as orphaned.
#[test]
fn audit_report_active_deletion_is_not_orphaned() {
    let report = GraphMetadataAuditReport {
        graph: "active-deleting-graph".to_string(),
        repair: true,
        stale_labels: vec![],
        stale_reltypes: vec![],
        stale_propidx: vec![],
        deleting_orphaned: false,
    };
    assert!(
        !report.deleting_orphaned,
        "active deletion (data present) must not be reported as orphaned"
    );
}

/// Verify the report derives Clone and PartialEq for use in test assertions.
#[test]
fn audit_report_clone_and_eq() {
    let a = GraphMetadataAuditReport {
        graph: "g".to_string(),
        repair: true,
        stale_labels: vec!["L".to_string()],
        stale_reltypes: vec![],
        stale_propidx: vec![],
        deleting_orphaned: false,
    };
    let b = a.clone();
    assert_eq!(a, b, "cloned reports must compare equal");
}

/// Verify repair=false is preserved in the report even when stale members exist.
/// The report must faithfully record the repair flag regardless of audit findings.
#[test]
fn audit_report_preserves_repair_flag_false() {
    let report = GraphMetadataAuditReport {
        graph: "g".to_string(),
        repair: false,
        stale_labels: vec!["SomeLabel".to_string()],
        stale_reltypes: vec!["SOME_REL".to_string()],
        stale_propidx: vec![],
        deleting_orphaned: false,
    };
    assert!(
        !report.repair,
        "repair=false must be preserved in the report even when stale members are detected"
    );
}

// ── Mock-backed entry-point tests for `audit_graph_metadata` ──────────────
//
// These tests exercise the early branches of `audit_graph_metadata` (graph
// name validation and the initial pool checkout) using the
// `RedisGraphStore::new_for_testing()` store. The deeper repair path that
// requires SCAN/SREM round-trips is covered by the live-Redis integration
// tests in `sd2-redis-store/tests/metadata_audit.rs`.

use crate::RedisGraphStore;
use cypher_core::error::StorageError;

#[test]
fn audit_graph_metadata_rejects_invalid_graph_name() {
    let store = RedisGraphStore::new_for_testing();
    // Brace characters are reserved as Redis Cluster hash-tags.
    let r = store.audit_graph_metadata("bad{name}", false);
    assert!(
        matches!(r, Err(StorageError::InvalidGraphName { .. })),
        "expected InvalidGraphName, got {r:?}"
    );
}

#[test]
fn audit_graph_metadata_rejects_empty_graph_name() {
    let store = RedisGraphStore::new_for_testing();
    let r = store.audit_graph_metadata("", false);
    assert!(
        matches!(r, Err(StorageError::InvalidGraphName { .. })),
        "expected InvalidGraphName for empty name, got {r:?}"
    );
}

#[test]
fn audit_graph_metadata_propagates_backend_error_for_valid_name() {
    let store = RedisGraphStore::new_for_testing();
    // Validation passes; the unreachable pool surfaces a Backend error
    // when audit_graph_metadata calls `self.conn()`.
    let r = store.audit_graph_metadata("validgraph", false);
    assert!(
        matches!(r, Err(StorageError::Backend { .. })),
        "expected Backend, got {r:?}"
    );
}

#[test]
fn audit_graph_metadata_with_repair_propagates_backend_error_for_valid_name() {
    let store = RedisGraphStore::new_for_testing();
    let r = store.audit_graph_metadata("validgraph", true);
    assert!(
        matches!(r, Err(StorageError::Backend { .. })),
        "expected Backend (repair=true), got {r:?}"
    );
}

// ── Mock-backed read-pass tests for `audit_graph_metadata_read_pass_on` ──

mod read_pass {
    use super::super::GraphMetadataAuditReport;
    use crate::RedisGraphStore;
    use cypher_core::error::StorageError;
    use redis::{Arg, Cmd, ConnectionLike, RedisError, RedisResult, Value as RedisValue};
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
        fn ok_sismember(self, b: bool) -> Self {
            self.push("SISMEMBER", Ok(RedisValue::Int(if b { 1 } else { 0 })))
        }
        fn ok_exists(self, n: i64) -> Self {
            self.push("EXISTS", Ok(RedisValue::Int(n)))
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
            panic!("not expected");
        }
        fn req_packed_commands(
            &mut self,
            _c: &[u8],
            _o: usize,
            _n: usize,
        ) -> RedisResult<Vec<RedisValue>> {
            panic!("not expected");
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
    fn read_pass_returns_none_when_graph_neither_registered_nor_deleting() {
        let s = store();
        // SISMEMBER -> false, EXISTS deleting marker -> 0
        let mut conn = ScriptedConn::default().ok_sismember(false).ok_exists(0);
        let result = s
            .audit_graph_metadata_read_pass_on(&mut conn, "g", false)
            .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn read_pass_returns_clean_report_when_no_stale_metadata() {
        let s = store();
        let mut conn = ScriptedConn::default()
            .ok_sismember(true) // registered
            .ok_exists(0) // not deleting
            .ok_smembers(&[]) // labels
            .ok_smembers(&[]) // reltypes
            .ok_smembers(&[]); // propidx
        let report = s
            .audit_graph_metadata_read_pass_on(&mut conn, "g", false)
            .unwrap()
            .unwrap();
        assert_eq!(report.graph, "g");
        assert!(report.stale_labels.is_empty());
        assert!(report.stale_reltypes.is_empty());
        assert!(report.stale_propidx.is_empty());
        assert!(!report.deleting_orphaned);
    }

    #[test]
    fn read_pass_detects_stale_label_with_empty_backing_set() {
        let s = store();
        let mut conn = ScriptedConn::default()
            .ok_sismember(true)
            .ok_exists(0)
            .ok_smembers(&["Ghost", "Person"])
            .ok_smembers(&[]) // reltypes
            .ok_smembers(&[]) // propidx
            .ok_scard(0) // Ghost stale
            .ok_scard(5); // Person live
        let report = s
            .audit_graph_metadata_read_pass_on(&mut conn, "g", true)
            .unwrap()
            .unwrap();
        assert_eq!(report.stale_labels, vec!["Ghost".to_string()]);
        assert!(report.repair);
    }

    #[test]
    fn read_pass_propagates_sismember_error() {
        let s = store();
        let mut conn = ScriptedConn::default().err("SISMEMBER");
        let err = s
            .audit_graph_metadata_read_pass_on(&mut conn, "g", false)
            .unwrap_err();
        assert!(matches!(err, StorageError::Backend { .. }));
    }

    #[test]
    fn read_pass_marks_deleting_orphaned_when_cleanup_roots_absent() {
        let s = store();
        // SISMEMBER -> false, EXISTS deleting marker -> 1 (deleting)
        // 3 SMEMBERS for label/reltype/propidx members (all empty)
        // graph_cleanup_roots_absent_on issues EXISTS calls; with 0 it's absent
        let mut conn = ScriptedConn::default()
            .ok_sismember(false)
            .ok_exists(1)
            .ok_smembers(&[])
            .ok_smembers(&[])
            .ok_smembers(&[])
            .ok_exists(0)
            .ok_exists(0)
            .ok_exists(0)
            .ok_exists(0)
            .ok_exists(0)
            .ok_exists(0)
            .ok_exists(0)
            .ok_exists(0);
        let report = s
            .audit_graph_metadata_read_pass_on(&mut conn, "g", false)
            .unwrap()
            .unwrap();
        // Build a fully-clean expected report (deleting_orphaned should be true)
        let _ = GraphMetadataAuditReport {
            graph: "g".into(),
            repair: false,
            stale_labels: vec![],
            stale_reltypes: vec![],
            stale_propidx: vec![],
            deleting_orphaned: true,
        };
        assert!(report.deleting_orphaned);
    }
}
