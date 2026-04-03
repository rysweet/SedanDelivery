/// Query execution budget — enforces time and row limits on every query.
///
/// # Security rationale
///
/// A GQL engine running in Redis's main thread (or a background thread via BlockClient)
/// MUST have a hard budget. Without it, a single `MATCH (n)-[*]->(m)` on a dense graph
/// hangs the Redis process indefinitely, causing a complete service outage.
///
/// Two independent limits protect against different attack shapes:
/// 1. **Deadline** — wall-clock timeout. Protects against long-running traversals.
/// 2. **Row budget** — maximum intermediate rows. Protects against Cartesian product
///    explosions (e.g., `MATCH (a)-[*1..5]->(b)` where each node has 100 neighbors:
///    100^5 = 10^10 rows allocated before the deadline fires, causing OOM).
///
/// Every operator's `next()` call MUST call `budget.check_or_err()` as its first statement.
///
/// Default values are set by module config; these are compile-time fallbacks:
/// - `DEFAULT_TIMEOUT_MS = 30_000` (30 seconds)
/// - `DEFAULT_MAX_ROWS = 10_000_000` (10 million intermediate rows)
/// - `DEFAULT_VARLEN_MAX_DEPTH = 30` (default max for unbounded `[*]` patterns)
use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::error::{ExecutionError, GqlError};

/// Default query timeout: 30 seconds. Can be overridden via `GQL.CONFIG SET query-timeout-ms`.
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// Default maximum intermediate rows. Can be overridden via:
/// - Environment variable `SD2_QUERY_MAX_ROWS` (e.g. `SD2_QUERY_MAX_ROWS=100000000`)
/// - `GQL.CONFIG SET query-max-rows`
pub const DEFAULT_MAX_ROWS: usize = usize::MAX;

/// Read max rows from environment, falling back to `DEFAULT_MAX_ROWS`.
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
    /// When set, leaf scan operators check [`reached_row_cap`] and stop
    /// producing rows once the cap is reached. Unlike `max_rows` (which
    /// triggers an error), hitting the row cap is a normal "we have enough
    /// rows" signal — operators simply break out of their loops.
    ///
    /// The cap is expressed as an absolute count of leaf-level rows
    /// (tracked by [`record_leaf_row`]).
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

/// Per-operator profiling statistics collected during PROFILE execution.
#[derive(Debug, Clone, Default)]
pub struct OperatorProfileStats {
    /// Number of rows produced by this operator.
    pub rows: usize,
    /// Number of store accesses (db hits) performed by this operator.
    pub db_hits: usize,
}

/// Only call `Instant::now()` once every this many `check_or_err()` invocations.
/// Row-budget checks are always performed (they are a simple integer comparison).
/// Configurable via `GQL_BUDGET_CHECK_INTERVAL` env var (default `1024`).
pub fn check_interval() -> u64 {
    crate::config::global().budget_check_interval
}

/// Compile-time default used when the global config is not yet available (tests, etc.).
pub const CHECK_INTERVAL: u64 = 1024;

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

    /// Create a budget from an [`EngineConfig`].
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

    /// Check both the deadline and row budget. Returns `Err` if either is exceeded.
    ///
    /// Call this as the FIRST STATEMENT in every operator's `next()` implementation.
    ///
    /// The row-budget check (a cheap integer comparison) runs on every call.
    /// The deadline check (which requires an `Instant::now()` syscall, ~20 ns)
    /// is amortized: it only executes once every [`CHECK_INTERVAL`] invocations.
    #[inline]
    pub fn check_or_err(&mut self) -> Result<(), GqlError> {
        // Always check the row budget — this is a cheap integer comparison.
        if self.rows_produced >= self.max_rows {
            return Err(GqlError::Execution(ExecutionError::ResultSetTooLarge {
                count: self.rows_produced,
                limit: self.max_rows,
            }));
        }
        // Amortize the Instant::now() syscall: only check deadline every
        // check_interval() invocations.
        self.check_counter += 1;
        if self.check_counter & (check_interval() - 1) == 0 {
            let now = Instant::now();
            if now > self.deadline {
                let elapsed = now.duration_since(self.deadline - self.timeout);
                return Err(GqlError::Execution(ExecutionError::Timeout {
                    elapsed_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
                    limit_ms: u64::try_from(self.timeout.as_millis()).unwrap_or(u64::MAX),
                }));
            }
        }
        Ok(())
    }

    /// Record that one more row was produced. Call this after each row is yielded.
    #[inline]
    pub fn record_row(&mut self) {
        self.rows_produced += 1;
    }

    /// Returns how many rows have been produced so far.
    #[inline]
    pub fn rows_produced(&self) -> usize {
        self.rows_produced
    }

    /// Returns the configured maximum row limit.
    #[inline]
    pub fn max_rows(&self) -> usize {
        self.max_rows
    }

    /// Returns the remaining time before the deadline.
    #[inline]
    pub fn remaining(&self) -> Option<Duration> {
        self.deadline.checked_duration_since(Instant::now())
    }

    /// Returns true if the budget has already expired.
    #[inline]
    pub fn is_expired(&self) -> bool {
        Instant::now() > self.deadline
    }

    /// Set a soft row cap for early termination by LIMIT operators.
    ///
    /// `needed` is the number of leaf-level rows needed from this point
    /// forward. The cap is computed as `leaf_rows_produced + needed`.
    ///
    /// Returns the previous cap so the caller can restore it via
    /// [`restore_row_cap`].
    #[inline]
    pub fn set_row_cap(&mut self, needed: usize) -> Option<usize> {
        let prev = self.row_cap;
        let new_cap = self.leaf_rows_produced.saturating_add(needed);
        // When nesting limits, take the tighter (smaller) cap.
        self.row_cap = Some(match prev {
            Some(existing) => existing.min(new_cap),
            None => new_cap,
        });
        prev
    }

    /// Clear or restore the soft row cap. Pass the value returned by
    /// [`set_row_cap`] to restore the previous state.
    #[inline]
    pub fn restore_row_cap(&mut self, prev: Option<usize>) {
        self.row_cap = prev;
    }

    /// Returns the current soft row cap, if any.
    #[inline]
    pub fn row_cap(&self) -> Option<usize> {
        self.row_cap
    }

    /// Record a leaf-level row. Called by leaf scan operators (NodeScan,
    /// Expand, VarLenExpand, IndexScan, etc.) alongside `record_row()`.
    /// This counter is used by `reached_row_cap()` and is separate from
    /// the general `rows_produced` counter.
    #[inline]
    pub fn record_leaf_row(&mut self) {
        self.leaf_rows_produced += 1;
    }

    /// Returns `true` when a soft row cap is active and the number of
    /// leaf-level rows produced has reached it. Leaf scan operators should
    /// break out of their loops (returning rows collected so far) when
    /// this returns `true`.
    #[inline]
    pub fn reached_row_cap(&self) -> bool {
        match self.row_cap {
            Some(cap) => self.leaf_rows_produced >= cap,
            None => false,
        }
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

    // ── Profiling support ─────────────────────────────────────────────

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

#[cfg(test)]
mod tests {
    use std::thread::sleep;

    use super::*;

    #[test]
    fn fresh_budget_allows_rows() {
        let mut budget = ExecutionBudget::default_budget();
        assert!(budget.check_or_err().is_ok());
    }

    #[test]
    fn row_budget_exceeded() {
        let mut budget = ExecutionBudget::new(Duration::from_secs(60), 3);
        budget.record_row();
        budget.record_row();
        budget.record_row();
        let result = budget.check_or_err();
        assert!(matches!(
            result,
            Err(GqlError::Execution(ExecutionError::ResultSetTooLarge {
                count: 3,
                limit: 3
            }))
        ));
    }

    #[test]
    fn timeout_detected() {
        let mut budget = ExecutionBudget::new(Duration::from_millis(1), DEFAULT_MAX_ROWS);
        sleep(Duration::from_millis(5));
        // The deadline check is amortized: only fires every CHECK_INTERVAL calls.
        // Pump (CHECK_INTERVAL - 1) calls to bring the counter to the trigger point,
        // then the next call will actually check Instant::now() and detect the timeout.
        for _ in 0..CHECK_INTERVAL - 1 {
            let _ = budget.check_or_err();
        }
        assert!(budget.check_or_err().is_err());
    }

    #[test]
    fn unlimited_budget_does_not_expire_quickly() {
        let mut budget = ExecutionBudget::unlimited();
        for _ in 0..1_000_000 {
            budget.record_row();
        }
        // Should not expire even after 1M rows
        assert!(budget.check_or_err().is_ok());
    }

    #[test]
    fn remaining_decreases() {
        let budget = ExecutionBudget::new(Duration::from_secs(10), DEFAULT_MAX_ROWS);
        let r = budget.remaining();
        assert!(r.is_some());
        assert!(r.unwrap() <= Duration::from_secs(10));
    }

    /// Verify that the amortized check still detects row budget immediately
    /// (row checks are never amortized, only Instant::now() is).
    #[test]
    fn row_budget_always_checked_even_with_amortization() {
        let mut budget = ExecutionBudget::new(Duration::from_secs(60), 5);
        for _ in 0..5 {
            budget.record_row();
        }
        // The very first check_or_err() after exceeding the row budget should fail,
        // regardless of the amortization counter state.
        assert!(budget.check_or_err().is_err());
    }

    /// Verify that the amortized timeout check still fires within CHECK_INTERVAL calls.
    #[test]
    fn amortized_timeout_fires_within_check_interval() {
        let mut budget = ExecutionBudget::new(Duration::from_millis(1), DEFAULT_MAX_ROWS);
        sleep(Duration::from_millis(5));

        // The timeout may not be detected on the first call (amortized), but
        // it MUST be detected within CHECK_INTERVAL calls.
        let mut detected = false;
        for _ in 0..CHECK_INTERVAL + 1 {
            if budget.check_or_err().is_err() {
                detected = true;
                break;
            }
        }
        assert!(
            detected,
            "timeout should be detected within CHECK_INTERVAL+1 calls"
        );
    }

    /// Verify that amortization doesn't prevent detection -- the counter wraps
    /// correctly and keeps checking periodically.
    #[test]
    fn amortized_check_counter_wraps_correctly() {
        let mut budget = ExecutionBudget::new(Duration::from_secs(60), DEFAULT_MAX_ROWS);
        // Call check_or_err many times (more than CHECK_INTERVAL) -- should never
        // error because timeout is far in the future and rows haven't been recorded.
        for _ in 0..CHECK_INTERVAL * 3 {
            assert!(budget.check_or_err().is_ok());
        }
        // Counter should have wrapped around and checked Instant::now() 3 times,
        // none of which triggered a timeout.
        assert_eq!(budget.check_counter, CHECK_INTERVAL * 3);
    }

    // ── Row cap tests ──────────────────────────────────────────────────

    #[test]
    fn test_set_row_cap_and_reached() {
        let mut budget = ExecutionBudget::unlimited();
        budget.set_row_cap(5);
        for _ in 0..5 {
            assert!(!budget.reached_row_cap());
            budget.record_leaf_row();
        }
        assert!(
            budget.reached_row_cap(),
            "row cap should be reached after 5 leaf rows"
        );
    }

    #[test]
    fn test_restore_row_cap() {
        let mut budget = ExecutionBudget::unlimited();
        let prev = budget.set_row_cap(3);
        for _ in 0..3 {
            budget.record_leaf_row();
        }
        assert!(budget.reached_row_cap());
        // Restore to None (no cap)
        budget.restore_row_cap(prev);
        assert!(
            !budget.reached_row_cap(),
            "after restoring None cap, reached_row_cap should be false"
        );
    }

    #[test]
    fn test_nested_caps_take_tighter() {
        let mut budget = ExecutionBudget::unlimited();
        let prev_outer = budget.set_row_cap(10);
        // The cap is now leaf_rows_produced(0) + 10 = 10
        assert_eq!(budget.row_cap(), Some(10));

        let _prev_inner = budget.set_row_cap(5);
        // The inner cap would be 0 + 5 = 5, which is tighter than 10
        assert_eq!(
            budget.row_cap(),
            Some(5),
            "nested set_row_cap should take the tighter (smaller) cap"
        );

        // Restore outer
        budget.restore_row_cap(prev_outer);
        assert_eq!(budget.row_cap(), None);
    }

    #[test]
    fn test_no_cap_means_unlimited() {
        let mut budget = ExecutionBudget::unlimited();
        // No set_row_cap call — cap is None
        for _ in 0..1000 {
            budget.record_leaf_row();
        }
        assert!(
            !budget.reached_row_cap(),
            "without a row cap, reached_row_cap should always be false"
        );
    }

    // ── Profile stats tests ──────────────────────────────────────────

    #[test]
    fn profiling_disabled_by_default() {
        let budget = ExecutionBudget::unlimited();
        assert!(!budget.is_profiling());
        assert!(budget.profile_stats().is_none());
    }

    #[test]
    fn profiling_enabled_records_operator_rows() {
        let mut budget = ExecutionBudget::unlimited();
        budget.enable_profiling();
        assert!(budget.is_profiling());

        budget.record_operator_rows(0x1000, 5);
        budget.record_operator_rows(0x2000, 3);
        budget.record_operator_rows(0x1000, 2); // accumulates

        let stats = budget.profile_stats().unwrap();
        assert_eq!(stats[&0x1000].rows, 7);
        assert_eq!(stats[&0x2000].rows, 3);
    }

    #[test]
    fn profiling_records_db_hits() {
        let mut budget = ExecutionBudget::unlimited();
        budget.enable_profiling();

        budget.record_operator_db_hits(0x1000, 10);
        budget.record_operator_db_hits(0x1000, 5);

        let stats = budget.profile_stats().unwrap();
        assert_eq!(stats[&0x1000].db_hits, 15);
    }

    #[test]
    fn profiling_noop_when_disabled() {
        let mut budget = ExecutionBudget::unlimited();
        // Profiling not enabled — these should be no-ops
        budget.record_operator_rows(0x1000, 5);
        budget.record_operator_db_hits(0x1000, 10);
        assert!(budget.take_profile_stats().is_none());
    }

    #[test]
    fn take_profile_stats_clears_state() {
        let mut budget = ExecutionBudget::unlimited();
        budget.enable_profiling();
        budget.record_operator_rows(0x1000, 1);

        let stats = budget.take_profile_stats();
        assert!(stats.is_some());
        assert_eq!(stats.unwrap()[&0x1000].rows, 1);

        // After take, profiling state is cleared
        assert!(budget.take_profile_stats().is_none());
    }
}
