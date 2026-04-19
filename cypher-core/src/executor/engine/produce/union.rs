//! UNION operator: combines two sub-query result sets.

use crate::error::{ExecutionError, GqlError};
use crate::planner::Operator;

use crate::executor::engine::helpers::RowDedupSet;
use crate::executor::engine::{ExecContext, QueryExecutor, Row};

impl<'a> QueryExecutor<'a> {
    pub(crate) fn exec_union(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Union { left, right, all } = op else {
            debug_assert!(false, "expected Union operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected Union operator".into(),
            }));
        };
        let preserved_seed = self.foreach_seed.borrow().clone();
        let left_rows = self.exec_operator(cx, left)?;
        if let Some(seed) = preserved_seed.clone() {
            self.foreach_seed.borrow_mut().replace(seed);
        }
        let right_rows = self.exec_operator(cx, right)?;
        if let Some(seed) = preserved_seed {
            self.foreach_seed.borrow_mut().replace(seed);
        }

        let mut out = Vec::with_capacity(left_rows.len() + right_rows.len());
        if *all {
            out.extend(left_rows);
            out.extend(right_rows);
        } else {
            let mut seen = RowDedupSet::new();
            for row in left_rows.into_iter().chain(right_rows) {
                cx.budget.check_or_err()?;
                if seen.insert(&row) {
                    out.push(row);
                }
            }
        }
        Ok(out)
    }
}
