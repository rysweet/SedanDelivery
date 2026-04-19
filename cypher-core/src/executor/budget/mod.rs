//! Query execution budget — enforces time and row limits on every query.
//!
//! # Security rationale
//!
//! A GQL engine running in Redis's main thread (or a background thread via BlockClient)
//! MUST have a hard budget. Without it, a single `MATCH (n)-[*]->(m)` on a dense graph
//! hangs the Redis process indefinitely, causing a complete service outage.
//!
//! Two independent limits protect against different attack shapes:
//! 1. **Deadline** — wall-clock timeout. Protects against long-running traversals.
//! 2. **Row budget** — maximum intermediate rows. Protects against Cartesian product
//!    explosions (e.g., `MATCH (a)-[*1..5]->(b)` where each node has 100 neighbors:
//!    100^5 = 10^10 rows allocated before the deadline fires, causing OOM).
//!
//! Every operator's `next()` call MUST call [`ExecutionBudget::check_or_err`] as its
//! first statement.
//!
//! Default values are set by module config; these are compile-time fallbacks:
//! - [`DEFAULT_TIMEOUT_MS`] = 30_000 (30 seconds)
//! - [`DEFAULT_MAX_ROWS`] = `usize::MAX` (effectively unbounded by default)
//! - [`DEFAULT_VARLEN_MAX_DEPTH`] = 30 (default max for unbounded `[*]` patterns)
//!
//! # Module layout
//!
//! The implementation is split across topic-focused submodules; all public items
//! continue to live under `crate::executor::budget::*`:
//!
//! - [`time`] — deadline check ([`ExecutionBudget::check_or_err`]), `remaining`,
//!   `is_expired`.
//! - [`rows`] — row counting, hard `max_rows` accessors, and the soft `row_cap`
//!   used by LIMIT operators for early termination.
//! - [`profile`] — per-operator profiling support and [`OperatorProfileStats`].

use std::collections::HashMap;
use std::time::{Duration, Instant};

mod profile;
mod rows;
mod time;

#[cfg(test)]
mod tests;

pub use profile::OperatorProfileStats;

/// Default query timeout: 30 seconds. Can be overridden via `GQL.CONFIG SET query-timeout-ms`.
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// Default maximum intermediate rows. Can be overridden via:
/// - Environment variable `SD2_QUERY_MAX_ROWS` (e.g. `SD2_QUERY_MAX_ROWS=100000000`)
/// - `GQL.CONFIG SET query-max-rows`
pub const DEFAULT_MAX_ROWS: usize = usize::MAX;

/// Read max rows from environment, falling back to [`DEFAULT_MAX_ROWS`].
pub fn configured_max_rows() -> usize {
    std::env::var("SD2_QUERY_MAX_ROWS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_MAX_ROWS)
}

/// Default maximum depth for unbounded variable-length patterns `[*]`.
/// A query with `[*]` uses this as the effective upper bound.
/// Can be overridden via `GQL.CONFIG SET varlen-expand-default-max`.
pub const DEFAULT_VARLEN_MAX_DEPTH: u32 = 30;

/// Only call `Instant::now()` once every this many `check_or_err()` invocations.
/// Row-budget checks are always performed (they are a simple integer comparison).
/// Configurable via `GQL_BUDGET_CHECK_INTERVAL` env var (default `1024`).
pub fn check_interval() -> u64 {
    crate::config::global().budget_check_interval
}

/// Compile-time default used when the global config is not yet available (tests, etc.).
pub const CHECK_INTERVAL: u64 = 1024;

/// Execution budget attached to every query.
///
/// Created once per `GQL.QUERY` or `GQL.MUTATE` command invocation.
/// Passed by shared reference through the operator tree.
#[derive(Debug)]
pub struct ExecutionBudget {
    /// Absolute instant when this query expires.
    deadline: Instant,
    /// Maximum number of intermediate rows the executor may produce.
    max_rows: usize,
    /// Rows produced so far. Incremented by the executor on every yielded row.
    rows_produced: usize,
    /// The original timeout, for inclusion in error messages.
    timeout: Duration,
    /// Counter for amortizing `Instant::now()` calls. The expensive syscall is
    /// only performed every `CHECK_INTERVAL` invocations of `check_or_err()`.
    check_counter: u64,
    /// Optional soft row cap set by LIMIT operators for early termination.
    ///
    /// When set, leaf scan operators check [`ExecutionBudget::reached_row_cap`]
    /// and stop producing rows once the cap is reached. Unlike `max_rows`
    /// (which triggers an error), hitting the row cap is a normal "we have
    /// enough rows" signal — operators simply break out of their loops.
    ///
    /// The cap is expressed as an absolute count of leaf-level rows
    /// (tracked by [`ExecutionBudget::record_leaf_row`]).
    row_cap: Option<usize>,
    /// Counter of leaf-level rows produced. Only incremented by leaf scan
    /// operators (NodeScan, Expand, etc.) when a row cap is active. This
    /// is separate from `rows_produced` (which counts all intermediate rows)
    /// so that the cap is not inflated by intermediate operators like Project.
    leaf_rows_produced: usize,
    /// Optional request ID for distributed trace context propagation.
    /// When set, links executor spans to the originating request trace.
    request_id: Option<String>,
    /// When profiling is enabled, accumulates per-operator statistics.
    /// Keys are operator pointer addresses (as `usize`), values are
    /// `(rows_produced, db_hits)` pairs.
    profile_stats: Option<HashMap<usize, OperatorProfileStats>>,
}

impl ExecutionBudget {
    /// Create a budget with explicit timeout and row limit.
    pub fn new(timeout: Duration, max_rows: usize) -> Self {
        Self {
            deadline: Instant::now() + timeout,
            max_rows,
            rows_produced: 0,
            timeout,
            check_counter: 0,
            row_cap: None,
            leaf_rows_produced: 0,
            request_id: None,
            profile_stats: None,
        }
    }

    /// Create a budget with the default timeout and row limit.
    pub fn default_budget() -> Self {
        Self::new(Duration::from_millis(DEFAULT_TIMEOUT_MS), DEFAULT_MAX_ROWS)
    }

    /// Create a budget from an [`crate::config::EngineConfig`].
    ///
    /// Uses `config.query_timeout_ms` and `config.query_max_rows`.
    pub fn from_config(config: &crate::config::EngineConfig) -> Self {
        Self::new(
            Duration::from_millis(config.query_timeout_ms),
            config.query_max_rows,
        )
    }

    /// Create an unlimited budget for use in unit tests only.
    ///
    /// # Panics
    /// Panics if called outside of `#[cfg(test)]` contexts at runtime.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn unlimited() -> Self {
        Self::new(Duration::from_secs(3600), usize::MAX)
    }

    /// Set the request ID for trace context propagation.
    #[inline]
    pub fn set_request_id(&mut self, id: String) {
        self.request_id = Some(id);
    }

    /// Returns the request ID, if set.
    #[inline]
    pub fn request_id(&self) -> Option<&str> {
        self.request_id.as_deref()
    }
}
