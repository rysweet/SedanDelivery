//! `SingleRow` source: produces a single empty row (or replays a FOREACH seed)
//! to drive downstream pipelines that need an "identity" input.

use crate::error::GqlError;
use crate::planner::Operator;

use super::super::{ExecContext, QueryExecutor, Row};

impl<'a> QueryExecutor<'a> {
    pub(crate) fn exec_single_row(
        &self,
        cx: &mut ExecContext<'_>,
        _op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        // If a FOREACH body injected a seed, return it instead of an empty row.
        if let Some(seed) = self.foreach_seed.borrow_mut().take() {
            for _ in &seed {
                cx.budget.record_row();
            }
            return Ok(seed);
        }
        cx.budget.record_row();
        let mut row = Row::new();
        // Parameters stay in executor-owned context so UNWIND/CREATE pipelines
        // do not clone the full parameter payload into every intermediate row.
        // Propagate outer row columns so sub-traversals (Apply, Choose, etc.)
        // can reference parent-scope aliases like v_1.
        if !cx.outer_rows.is_empty() {
            for (k, v) in &cx.outer_rows[0] {
                row.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
        Ok(vec![row])
    }
}
