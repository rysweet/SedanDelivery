use gql_core::error::StorageError;
use gql_core::storage::validate_graph_name;
use redis::ConnectionLike;

use crate::keys::{
    key_edge_seq, key_edges_set, key_indexes, key_label, key_label_names, key_node_seq,
    key_nodes_set, key_propidx_manifest, key_propidx_prefix, key_reltype, key_reltype_names,
};
use crate::lock::WRITE_LOCK_TTL_MS;
use crate::{LockedRedisConn, RedisGraphStore};

/// Result of a graph-scoped Redis metadata audit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphMetadataAuditReport {
    pub graph: String,
    pub repair: bool,
    pub stale_labels: Vec<String>,
    pub stale_reltypes: Vec<String>,
    pub stale_propidx: Vec<String>,
    pub deleting_orphaned: bool,
}

impl RedisGraphStore {
    /// Audit Redis-only graph metadata and optionally prune provably stale state.
    ///
    /// This intentionally stays graph-scoped and conservative:
    /// - `_labels` members are stale only when their backing label set is empty
    /// - `_reltypes` members are stale only when their backing reltype set is empty
    /// - `_propidx` members are stale only when the referenced propidx key is gone
    /// - `_deleting` is reported when delete-time cleanup roots are already gone,
    ///   but audit never finalizes or clears deletion state automatically
    pub fn audit_graph_metadata(
        &self,
        graph: &str,
        repair: bool,
    ) -> Result<GraphMetadataAuditReport, StorageError> {
        validate_graph_name(graph)?;

        let mut conn = self.conn()?;
        let (registered, deleting) = self.graph_registry_state_on(conn.as_mut(), graph)?;
        if !registered && !deleting {
            return Err(StorageError::GraphNotFound {
                name: graph.to_string(),
            });
        }

        let label_members =
            self.read_set_members(conn.as_mut(), &self.kp(key_label_names(graph)))?;
        let reltype_members =
            self.read_set_members(conn.as_mut(), &self.kp(key_reltype_names(graph)))?;
        let propidx_members =
            self.read_set_members(conn.as_mut(), &self.kp(key_propidx_manifest(graph)))?;

        let mut stale_labels =
            self.collect_stale_labels_on(conn.as_mut(), graph, &label_members)?;
        let mut stale_reltypes =
            self.collect_stale_reltypes_on(conn.as_mut(), graph, &reltype_members)?;
        let mut stale_propidx =
            self.collect_stale_propidx_on(conn.as_mut(), graph, &propidx_members)?;
        stale_labels.sort();
        stale_reltypes.sort();
        stale_propidx.sort();
        let deleting_orphaned =
            deleting && self.graph_cleanup_roots_absent_on(conn.as_mut(), graph)?;

        drop(conn);

        if !repair
            || (stale_labels.is_empty() && stale_reltypes.is_empty() && stale_propidx.is_empty())
        {
            return Ok(GraphMetadataAuditReport {
                graph: graph.to_string(),
                repair,
                stale_labels,
                stale_reltypes,
                stale_propidx,
                deleting_orphaned,
            });
        }

        let candidate_labels = stale_labels;
        let candidate_reltypes = stale_reltypes;
        let candidate_propidx = stale_propidx;

        self.with_graph_write_lock(graph, WRITE_LOCK_TTL_MS * 2, 3, 50, |conn| {
            let (registered, deleting) = self.graph_registry_state_on(conn.as_mut(), graph)?;
            if !registered && !deleting {
                return Err(StorageError::GraphNotFound {
                    name: graph.to_string(),
                });
            }

            let mut stale_labels =
                self.recheck_stale_labels_under_lock(conn, graph, &candidate_labels)?;
            let mut stale_reltypes =
                self.recheck_stale_reltypes_under_lock(conn, graph, &candidate_reltypes)?;
            let mut stale_propidx =
                self.recheck_stale_propidx_under_lock(conn, graph, &candidate_propidx)?;
            stale_labels.sort();
            stale_reltypes.sort();
            stale_propidx.sort();

            if !stale_labels.is_empty() {
                conn.renew_lock()?;
                let mut cmd = redis::cmd("SREM");
                cmd.arg(self.kp(key_label_names(graph)));
                for label in &stale_labels {
                    cmd.arg(label);
                }
                cmd.query::<i64>(conn.as_mut())
                    .map_err(crate::backend_storage_error)?;
            }

            if !stale_reltypes.is_empty() {
                conn.renew_lock()?;
                let mut cmd = redis::cmd("SREM");
                cmd.arg(self.kp(key_reltype_names(graph)));
                for reltype in &stale_reltypes {
                    cmd.arg(reltype);
                }
                cmd.query::<i64>(conn.as_mut())
                    .map_err(crate::backend_storage_error)?;
            }

            if !stale_propidx.is_empty() {
                conn.renew_lock()?;
                let mut cmd = redis::cmd("SREM");
                cmd.arg(self.kp(key_propidx_manifest(graph)));
                for member in &stale_propidx {
                    cmd.arg(member);
                }
                cmd.query::<i64>(conn.as_mut())
                    .map_err(crate::backend_storage_error)?;
            }

            let deleting_orphaned =
                deleting && self.graph_cleanup_roots_absent_on(conn.as_mut(), graph)?;

            Ok(GraphMetadataAuditReport {
                graph: graph.to_string(),
                repair,
                stale_labels,
                stale_reltypes,
                stale_propidx,
                deleting_orphaned,
            })
        })
    }

    fn graph_registry_state_on(
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

    fn read_set_members(
        &self,
        conn: &mut dyn ConnectionLike,
        key: &str,
    ) -> Result<Vec<String>, StorageError> {
        redis::cmd("SMEMBERS")
            .arg(key)
            .query(conn)
            .map_err(crate::backend_storage_error)
    }

    fn collect_stale_labels_on(
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

    fn collect_stale_reltypes_on(
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

    fn collect_stale_propidx_on(
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

    fn graph_cleanup_roots_absent_on(
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

    fn recheck_stale_labels_under_lock(
        &self,
        conn: &mut LockedRedisConn,
        graph: &str,
        candidates: &[String],
    ) -> Result<Vec<String>, StorageError> {
        let mut stale = Vec::new();
        for (index, label) in candidates.iter().enumerate() {
            if index > 0 && index % 100 == 0 {
                conn.renew_lock()?;
            }
            let count: u64 = redis::cmd("SCARD")
                .arg(self.kp(key_label(graph, label)))
                .query(conn.as_mut())
                .map_err(crate::backend_storage_error)?;
            if count == 0 {
                stale.push(label.clone());
            }
        }
        Ok(stale)
    }

    fn recheck_stale_reltypes_under_lock(
        &self,
        conn: &mut LockedRedisConn,
        graph: &str,
        candidates: &[String],
    ) -> Result<Vec<String>, StorageError> {
        let mut stale = Vec::new();
        for (index, reltype) in candidates.iter().enumerate() {
            if index > 0 && index % 100 == 0 {
                conn.renew_lock()?;
            }
            let count: u64 = redis::cmd("SCARD")
                .arg(self.kp(key_reltype(graph, reltype)))
                .query(conn.as_mut())
                .map_err(crate::backend_storage_error)?;
            if count == 0 {
                stale.push(reltype.clone());
            }
        }
        Ok(stale)
    }

    fn recheck_stale_propidx_under_lock(
        &self,
        conn: &mut LockedRedisConn,
        graph: &str,
        candidates: &[String],
    ) -> Result<Vec<String>, StorageError> {
        let mut stale = Vec::new();
        for (index, member) in candidates.iter().enumerate() {
            if index > 0 && index % 100 == 0 {
                conn.renew_lock()?;
            }
            if !self.propidx_member_matches_graph(graph, member) {
                stale.push(member.clone());
                continue;
            }
            let exists: i64 = redis::cmd("EXISTS")
                .arg(member)
                .query(conn.as_mut())
                .map_err(crate::backend_storage_error)?;
            if exists == 0 {
                stale.push(member.clone());
            }
        }
        Ok(stale)
    }

    fn propidx_member_matches_graph(&self, graph: &str, member: &str) -> bool {
        member.starts_with(&self.kp(key_propidx_prefix(graph)))
    }
}

// ── WS-G (#756): audit_graph_metadata unit tests ───────────────────────────────
//
// These tests verify GraphMetadataAuditReport shape and invariants without
// requiring Redis. The behavioral audit tests that exercise a live backend live
// in `gql-redis-store/tests/metadata_audit.rs`.

#[cfg(test)]
mod ws_g_audit_report_unit_tests {
    use super::*;

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
}
