//! `coalesce()` — first-match alternative traversal.
//!
//! Tries each alternative in order and returns results from the first one
//! that produces non-empty output. Rows with no matching alternative are
//! dropped (standard Gremlin semantics).

use crate::error::{ExecutionError, GqlError};
use crate::planner::Operator;

use crate::executor::engine::{ExecContext, QueryExecutor, Row};

impl<'a> QueryExecutor<'a> {
    /// Execute `Coalesce` — try each alternative in order, return results from
    /// the first one that produces non-empty output.
    pub(crate) fn exec_coalesce(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Coalesce {
            alternatives,
            child,
        } = op
        else {
            debug_assert!(false, "expected Coalesce operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected Coalesce operator".into(),
            }));
        };

        let input = self.exec_operator(cx, child)?;
        let mut out = Vec::with_capacity(input.len());

        for row in input {
            cx.budget.check_or_err()?;
            let mut matched = false;

            for alt in alternatives {
                self.foreach_seed.borrow_mut().replace(vec![row.clone()]);
                let alt_rows = self.exec_operator(cx, alt)?;
                self.foreach_seed.borrow_mut().take();

                if !alt_rows.is_empty() {
                    for mut alt_row in alt_rows {
                        for (key, value) in &row {
                            alt_row.entry(key.clone()).or_insert_with(|| value.clone());
                        }
                        out.push(alt_row);
                    }
                    matched = true;
                    break;
                }
            }

            if !matched {
                // No alternative matched — drop the traverser.
            }
        }
        Ok(out)
    }
}
