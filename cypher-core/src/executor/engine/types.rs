//! Public types and per-call execution context used by the executor.

use std::collections::{HashMap, HashSet};

use crate::error::GqlError;
use crate::executor::budget::ExecutionBudget;
use crate::types::Value;

/// A single result row: owned map from column name to value.
pub type Row = HashMap<String, Value>;

/// Bundles the per-call execution context that is threaded through every
/// operator invocation, replacing the previous 4-param `(graph, budget,
/// outer_rows, stats)` pattern.
pub(crate) struct ExecContext<'a> {
    pub graph: &'a str,
    pub budget: &'a mut ExecutionBudget,
    pub outer_rows: &'a [Row],
    pub stats: &'a mut MutationStats,
}

impl<'a> ExecContext<'a> {
    /// Reborrow with different `outer_rows`, used by Apply / LeftOuterApply.
    pub fn with_outer<'b>(&'b mut self, outer_rows: &'b [Row]) -> ExecContext<'b> {
        ExecContext {
            graph: self.graph,
            budget: &mut *self.budget,
            outer_rows,
            stats: &mut *self.stats,
        }
    }
}

// ── Result and stats types ────────────────────────────────────────────────────

/// The final result of executing a query.
#[derive(Debug)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Row>,
    pub stats: MutationStats,
    /// When a per-row error occurs during streaming evaluation (e.g. division
    /// by zero on one row of an UNWIND), the successfully-computed rows are
    /// returned in `rows` and the error that terminated evaluation is stored
    /// here. This allows Bolt-level streaming: the server can emit RECORD
    /// messages for the partial rows and then emit FAILURE for the trailing
    /// error.
    pub trailing_error: Option<GqlError>,
}

/// Counters for write operations performed during query execution.
#[derive(Debug, Default, Clone)]
pub struct MutationStats {
    pub nodes_created: u64,
    pub nodes_deleted: u64,
    pub edges_created: u64,
    pub edges_deleted: u64,
    pub properties_set: u64,
    pub labels_added: u64,
    pub labels_removed: u64,
    /// Internal: distinct label types seen during this execution.
    pub(crate) labels_seen: HashSet<String>,
    /// Internal: label types that existed in the graph before this query.
    pub(crate) pre_existing_labels: HashSet<String>,
    /// Internal: distinct (is_edge, entity_raw_id, prop_key) triples seen.
    pub(crate) props_seen: HashSet<(bool, u64, String)>,
    /// Internal: IDs of nodes created in this query (for net-effect tracking).
    pub(crate) nodes_created_ids: HashSet<u64>,
    /// Internal: IDs of edges created in this query (for net-effect tracking).
    pub(crate) edges_created_ids: HashSet<u64>,
}

impl MutationStats {
    /// Finalise derived counters from the tracking sets.
    /// Must be called once at the end of `execute()`.
    pub(super) fn finalize(&mut self) {
        self.labels_added = self
            .labels_seen
            .difference(&self.pre_existing_labels)
            .count() as u64;
        self.properties_set = self.props_seen.len() as u64;
    }
}
