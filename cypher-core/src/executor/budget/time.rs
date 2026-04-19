//! Wall-clock deadline enforcement and the combined budget check.
//!
//! Houses [`ExecutionBudget::check_or_err`], which every operator's `next()`
//! implementation must call as its first statement, plus the helpers used to
//! inspect remaining time.

use std::time::{Duration, Instant};

use crate::error::{ExecutionError, GqlError};

use super::{check_interval, ExecutionBudget};

impl ExecutionBudget {
    /// Check both the deadline and row budget. Returns `Err` if either is exceeded.
    ///
    /// Call this as the FIRST STATEMENT in every operator's `next()` implementation.
    ///
    /// The row-budget check (a cheap integer comparison) runs on every call.
    /// The deadline check (which requires an `Instant::now()` syscall, ~20 ns)
    /// is amortized: it only executes once every [`super::CHECK_INTERVAL`] invocations.
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
