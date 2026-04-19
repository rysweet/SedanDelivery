//! Graph-scoped Redis metadata audit and conservative repair.
//!
//! Submodules:
//! - [`scan`]: read-only helpers that collect candidate stale members
//! - [`repair`]: lock-bound rechecks performed during the repair pass
//! - [`tests`]: unit tests for [`GraphMetadataAuditReport`] (no Redis required)

use cypher_core::error::StorageError;
use cypher_core::storage::validate_graph_name;

use crate::keys::{key_label_names, key_propidx_manifest, key_reltype_names};
use crate::lock::WRITE_LOCK_TTL_MS;
use crate::RedisGraphStore;

mod repair;
mod scan;

#[cfg(test)]
mod tests;

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
}
