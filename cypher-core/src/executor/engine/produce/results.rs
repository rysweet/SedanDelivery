//! ProduceResults operator: final projection of result columns.

use crate::error::{ExecutionError, GqlError};
use crate::planner::Operator;

use crate::executor::engine::{ExecContext, QueryExecutor, Row};

impl<'a> QueryExecutor<'a> {
    pub(crate) fn exec_produce_results(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::ProduceResults { columns, child } = op else {
            debug_assert!(false, "expected ProduceResults operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected ProduceResults operator".into(),
            }));
        };
        let input = self.exec_operator(cx, child)?;

        if columns.is_empty() {
            return Ok(vec![]);
        }

        let mut out = Vec::with_capacity(input.len());

        let has_star = columns.iter().any(|(name, _)| name == "*");

        macro_rules! try_eval_streaming {
            ($expr_result:expr, $out:ident) => {
                match $expr_result {
                    Ok(val) => val,
                    Err(e) => {
                        if !$out.is_empty() {
                            *self.streaming_error.borrow_mut() = Some(e);
                            return Ok($out);
                        }
                        return Err(e);
                    }
                }
            };
        }

        for row in &input {
            cx.budget.check_or_err()?;
            let ctx = self.build_filter_ctx(cx.graph, row)?;
            let mut new_row = Row::new();

            if has_star {
                for (col_name, expr) in columns {
                    if col_name == "*" {
                        for (var_name, val) in row {
                            if !var_name.starts_with("__") {
                                new_row.insert(var_name.clone(), val.clone());
                            }
                        }
                    } else {
                        let val = try_eval_streaming!(
                            self.eval_with_storage(cx.graph, expr, row, &ctx),
                            out
                        );
                        new_row.insert(col_name.clone(), val);
                    }
                }
            } else {
                for (col_name, expr) in columns {
                    let val =
                        try_eval_streaming!(self.eval_with_storage(cx.graph, expr, row, &ctx), out);
                    new_row.insert(col_name.clone(), val);
                }
            }
            cx.budget.record_row();
            out.push(new_row);
        }
        Ok(out)
    }
}
