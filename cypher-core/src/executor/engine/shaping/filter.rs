//! `Filter` operator: drops rows whose predicate does not evaluate to `true`.

use crate::error::GqlError;
use crate::planner::Operator;
use crate::types::Value;

use super::super::{ExecContext, QueryExecutor, Row};

impl<'a> QueryExecutor<'a> {
    pub(crate) fn exec_filter(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Filter { predicate, child } = op else {
            debug_assert!(false, "expected Filter operator");
            return Err(GqlError::Execution(
                crate::error::ExecutionError::UnsupportedFeature {
                    feature: "internal: expected Filter operator".into(),
                },
            ));
        };
        let input = self.exec_operator(cx, child)?;
        let mut out = Vec::with_capacity(input.len());
        for row in input {
            cx.budget.check_or_err()?;
            let ctx = self.build_filter_ctx(cx.graph, &row)?;
            // Use eval_with_storage so that property access on Node/Edge IDs
            // is resolved against the store rather than returning Null.
            let result = self.eval_with_storage(cx.graph, predicate, &row, &ctx)?;
            if result == Value::Bool(true) {
                out.push(row);
            }
        }
        Ok(out)
    }
}
