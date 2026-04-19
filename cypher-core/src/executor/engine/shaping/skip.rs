//! `Skip` operator: drops the first `n` rows from the input.

use crate::error::GqlError;
use crate::planner::Operator;

use super::super::helpers::eval_count_expr;
use super::super::{ExecContext, QueryExecutor, Row};

impl<'a> QueryExecutor<'a> {
    pub(crate) fn exec_skip(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Skip { count, child } = op else {
            debug_assert!(false, "expected Skip operator");
            return Err(GqlError::Execution(
                crate::error::ExecutionError::UnsupportedFeature {
                    feature: "internal: expected Skip operator".into(),
                },
            ));
        };
        let input = self.exec_operator(cx, child)?;
        cx.budget.check_or_err()?;
        let n = eval_count_expr(count, &self.parameters)?;
        Ok(input.into_iter().skip(n).collect())
    }
}
