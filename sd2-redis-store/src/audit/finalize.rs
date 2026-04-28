//! Operator-driven finalization of a stuck `_deleting` marker (F4 in the
//! cluster consistency hardening package, issue #1769).
//!
//! When `delete_graph` runs to completion the `G:{g}:_deleting` marker is
//! cleared and the graph name is removed from the global registry. If the
//! cleanup pass crashes part-way through — leaving cleanup roots gone but
//! the marker still set — the graph name remains reserved indefinitely
//! until an operator intervenes.
//!
//! [`RedisGraphStore::finalize_delete`] is the operator-facing endpoint
//! that completes the cleanup safely:
//!
//! 1. Run a fresh audit pass under the per-graph write lock so a concurrent
//!    re-creation does not race a stale finalize call.
//! 2. Refuse the operation unless `report.deleting_orphaned == true`.
//! 3. Delete the `_deleting` marker key.
//! 4. Remove the graph name from the global registry.
//!
//! See `docs/cluster-write-safety.md` for the runbook entry and
//! `docs/admin-api.md` for the REST contract.

use cypher_core::error::StorageError;
use cypher_core::storage::validate_graph_name;

use crate::keys::key_graph_deleting;
use crate::lock::WRITE_LOCK_TTL_MS;
use crate::RedisGraphStore;

/// Result of a successful [`RedisGraphStore::finalize_delete`] call.
///
/// `deleted_keys` lists the Redis keys that were actually `DEL`'d (e.g. the
/// `_deleting` marker hash). `registry_removed_from` is the registry set
/// the graph name was `SREM`'d from — the registry key itself was NOT
/// deleted, only the graph's membership in it. Keeping the two surfaces
/// distinct prevents an operator from misreading the report as "the global
/// registry was deleted."
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalizeDeleteReport {
    pub graph: String,
    pub finalized: bool,
    /// Redis keys that were `DEL`'d outright by this finalize pass.
    pub deleted_keys: Vec<String>,
    /// Registry set key the graph membership was `SREM`'d from, or `None`
    /// if the registry membership was already absent.
    pub registry_removed_from: Option<String>,
}

impl RedisGraphStore {
    /// Finalize a stuck `_deleting` marker and free the graph name.
    ///
    /// Returns [`StorageError::ConstraintViolation`] if the graph is not in
    /// the orphaned-deleting state — callers must surface this as HTTP 409
    /// Conflict at the API layer. Returns [`StorageError::GraphNotFound`]
    /// if neither the registry membership nor the `_deleting` marker
    /// exists.
    pub fn finalize_delete(&self, graph: &str) -> Result<FinalizeDeleteReport, StorageError> {
        validate_graph_name(graph)?;

        self.with_graph_write_lock(graph, WRITE_LOCK_TTL_MS * 2, 3, 50, |conn| {
            let report = self
                .audit_graph_metadata_read_pass_on(conn.as_mut(), graph, false)?
                .ok_or_else(|| StorageError::GraphNotFound {
                    name: graph.to_string(),
                })?;

            if !report.deleting_orphaned {
                return Err(StorageError::ConstraintViolation {
                    msg: format!(
                        "graph '{graph}' is not in the orphaned-deleting state — \
                         finalize-delete refuses to run; use the regular delete-graph \
                         path or wait for the in-flight cleanup to complete"
                    ),
                });
            }

            conn.renew_lock()?;

            let deleting_key = self.kp(key_graph_deleting(graph));
            let registry_key = self.key_graphs();
            let removed: i64 = redis::cmd("SREM")
                .arg(&registry_key)
                .arg(graph)
                .query(conn.as_mut())
                .map_err(crate::backend_storage_error)?;
            redis::cmd("DEL")
                .arg(&deleting_key)
                .query::<i64>(conn.as_mut())
                .map_err(crate::backend_storage_error)?;

            Ok(FinalizeDeleteReport {
                graph: graph.to_string(),
                finalized: true,
                deleted_keys: vec![deleting_key],
                registry_removed_from: if removed > 0 {
                    Some(registry_key)
                } else {
                    None
                },
            })
        })
    }
}
