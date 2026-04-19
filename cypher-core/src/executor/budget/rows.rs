//! Row accounting and soft row caps for [`ExecutionBudget`].
//!
//! Tracks two independent counters:
//! - `rows_produced` — every intermediate row, used by the hard `max_rows` limit.
//! - `leaf_rows_produced` — only leaf scans, used by the soft `row_cap` set by
//!   LIMIT operators for early termination.

use super::ExecutionBudget;

impl ExecutionBudget {
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

    /// Set a soft row cap for early termination by LIMIT operators.
    ///
    /// `needed` is the number of leaf-level rows needed from this point
    /// forward. The cap is computed as `leaf_rows_produced + needed`.
    ///
    /// Returns the previous cap so the caller can restore it via
    /// [`ExecutionBudget::restore_row_cap`].
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
    /// [`ExecutionBudget::set_row_cap`] to restore the previous state.
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
}
