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
/// - `DEFAULT_TIMEOUT_MS = 120000` (120 seconds)
/// - `DEFAULT_MAX_ROWS = 1_000_000` (1 million intermediate rows)
/// - `DEFAULT_VARLEN_MAX_DEPTH = 10` (default max for unbounded `[*]` patterns)
use std::time::{Duration, Instant};

use crate::error::{ExecutionError, GqlError};

/// Default query timeout: 30 seconds. Can be overridden via `GQL.CONFIG SET query-timeout-ms`.
pub const DEFAULT_TIMEOUT_MS: u64 = 300_000;

/// Default maximum intermediate rows. Can be overridden via `GQL.CONFIG SET query-max-rows`.
pub const DEFAULT_MAX_ROWS: usize = 10_000_000;

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
}

impl ExecutionBudget {
    /// Create a budget with explicit timeout and row limit.
    pub fn new(timeout: Duration, max_rows: usize) -> Self {
        Self {
            deadline: Instant::now() + timeout,
            max_rows,
            rows_produced: 0,
            timeout,
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
    #[inline]
    pub fn check_or_err(&self) -> Result<(), GqlError> {
        let now = Instant::now();
        if now > self.deadline {
            let elapsed = now.duration_since(self.deadline - self.timeout);
            return Err(GqlError::Execution(ExecutionError::Timeout {
                elapsed_ms: elapsed.as_millis() as u64,
                limit_ms: self.timeout.as_millis() as u64,
            }));
        }
        if self.rows_produced >= self.max_rows {
            return Err(GqlError::Execution(ExecutionError::ResultSetTooLarge {
                count: self.rows_produced,
                limit: self.max_rows,
            }));
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
}

#[cfg(test)]
mod tests {
    use std::thread::sleep;

    use super::*;

    #[test]
    fn fresh_budget_allows_rows() {
        let budget = ExecutionBudget::default_budget();
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
        let budget = ExecutionBudget::new(Duration::from_millis(1), DEFAULT_MAX_ROWS);
        sleep(Duration::from_millis(5));
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
}
