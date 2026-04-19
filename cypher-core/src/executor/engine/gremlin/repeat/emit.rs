//! Emit-predicate handling for the `repeat` step.

use crate::error::GqlError;
use crate::executor::engine::{ExecContext, QueryExecutor, Row};
use crate::planner::Operator;

impl<'a> QueryExecutor<'a> {
    /// Collect rows into `collected`, optionally filtering by emit predicate.
    pub(super) fn collect_emitted(
        &self,
        cx: &mut ExecContext<'_>,
        rows: &[Row],
        emit_predicate: &Option<Box<Operator>>,
        collected: &mut Vec<Row>,
    ) -> Result<(), GqlError> {
        if let Some(pred_op) = emit_predicate {
            for row in rows {
                self.foreach_seed.borrow_mut().replace(vec![row.clone()]);
                let result = self.exec_operator(cx, pred_op)?;
                self.foreach_seed.borrow_mut().take();
                if !result.is_empty() {
                    collected.push(row.clone());
                }
            }
        } else {
            collected.extend(rows.iter().cloned());
        }
        Ok(())
    }
}
