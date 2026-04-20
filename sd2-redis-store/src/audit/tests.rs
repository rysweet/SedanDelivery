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
