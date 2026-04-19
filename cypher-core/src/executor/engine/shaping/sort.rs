//! `Sort` operator: orders rows by one or more keys, with optional
//! Gremlin-style comparison semantics.

use crate::error::GqlError;
use crate::planner::Operator;
use crate::types::Value;

use super::super::helpers::{cmp_values, gremlin_cmp_values};
use super::super::{ExecContext, QueryExecutor, Row};

impl<'a> QueryExecutor<'a> {
    pub(crate) fn exec_sort(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Sort {
            items,
            gremlin_order,
            child,
        } = op
        else {
            debug_assert!(false, "expected Sort operator");
            return Err(GqlError::Execution(
                crate::error::ExecutionError::UnsupportedFeature {
                    feature: "internal: expected Sort operator".into(),
                },
            ));
        };
        let input = self.exec_operator(cx, child)?;
        cx.budget.check_or_err()?;

        // Evaluate all sort keys up front to avoid repeated store lookups.
        let mut keyed: Vec<(Vec<Value>, Row)> = input
            .into_iter()
            .map(|row| {
                let ctx = self.build_filter_ctx(cx.graph, &row)?;
                let keys: Result<Vec<Value>, GqlError> = items
                    .iter()
                    .map(|(expr, _)| self.eval_with_storage(cx.graph, expr, &row, &ctx))
                    .collect();
                Ok((keys?, row))
            })
            .collect::<Result<Vec<_>, GqlError>>()?;

        let use_gremlin = *gremlin_order;
        keyed.sort_by(|(ka, _), (kb, _)| {
            for i in 0..items.len() {
                let va = &ka[i];
                let vb = &kb[i];
                let asc = items[i].1;
                let ord = if use_gremlin {
                    gremlin_cmp_values(va, vb)
                } else {
                    cmp_values(va, vb)
                };
                if ord != std::cmp::Ordering::Equal {
                    return if asc { ord } else { ord.reverse() };
                }
            }
            std::cmp::Ordering::Equal
        });

        Ok(keyed.into_iter().map(|(_, row)| row).collect())
    }
}
