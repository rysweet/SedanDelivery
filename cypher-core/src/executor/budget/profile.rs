//! Per-operator profiling support for [`ExecutionBudget`].
//!
//! When profiling is enabled (via [`ExecutionBudget::enable_profiling`]),
//! operators report their row counts and store accesses keyed by their
//! pointer address. The accumulated map can later be drained with
//! [`ExecutionBudget::take_profile_stats`] for inclusion in PROFILE output.

use std::collections::HashMap;

use super::ExecutionBudget;

/// Per-operator profiling statistics collected during PROFILE execution.
#[derive(Debug, Clone, Default)]
pub struct OperatorProfileStats {
    /// Number of rows produced by this operator.
    pub rows: usize,
    /// Number of store accesses (db hits) performed by this operator.
    pub db_hits: usize,
}

impl ExecutionBudget {
    /// Enable per-operator profiling. Call this before execution to collect
    /// per-operator row counts and db_hits.
    #[inline]
    pub fn enable_profiling(&mut self) {
        self.profile_stats = Some(HashMap::new());
    }

    /// Returns `true` if profiling is enabled.
    #[inline]
    pub fn is_profiling(&self) -> bool {
        self.profile_stats.is_some()
    }

    /// Record rows produced by the operator at the given pointer address.
    /// No-op if profiling is not enabled.
    #[inline]
    pub fn record_operator_rows(&mut self, op_addr: usize, rows: usize) {
        if let Some(ref mut stats) = self.profile_stats {
            let entry = stats.entry(op_addr).or_default();
            entry.rows += rows;
        }
    }

    /// Record a db hit for the operator at the given pointer address.
    /// No-op if profiling is not enabled.
    #[inline]
    pub fn record_operator_db_hits(&mut self, op_addr: usize, hits: usize) {
        if let Some(ref mut stats) = self.profile_stats {
            let entry = stats.entry(op_addr).or_default();
            entry.db_hits += hits;
        }
    }

    /// Take the accumulated profile stats, leaving `None` behind.
    /// Returns `None` if profiling was not enabled.
    #[inline]
    pub fn take_profile_stats(&mut self) -> Option<HashMap<usize, OperatorProfileStats>> {
        self.profile_stats.take()
    }

    /// Borrow the accumulated profile stats.
    #[inline]
    pub fn profile_stats(&self) -> Option<&HashMap<usize, OperatorProfileStats>> {
        self.profile_stats.as_ref()
    }
}
