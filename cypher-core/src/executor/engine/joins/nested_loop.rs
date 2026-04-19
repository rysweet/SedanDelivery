//! Nested-loop correlated join algorithms: `exec_left_outer_apply` and
//! `exec_apply`. Both iterate the outer (preserved) side once and re-evaluate
//! the inner (optional/correlated) sub-plan per outer row, with the outer row
//! exposed to the inner via `ExecContext::with_outer`.

use crate::error::GqlError;
use crate::planner::Operator;
use crate::types::Value;

use super::super::helpers::discover_operator_aliases;
use super::super::{ExecContext, QueryExecutor, Row};

impl<'a> QueryExecutor<'a> {
    pub(crate) fn exec_left_outer_apply(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::LeftOuterApply {
            preserved,
            optional_pattern,
        } = op
        else {
            debug_assert!(false, "expected LeftOuterApply operator");
            return Err(GqlError::Execution(
                crate::error::ExecutionError::UnsupportedFeature {
                    feature: "internal: expected LeftOuterApply operator".into(),
                },
            ));
        };
        let left_rows = self.exec_operator(cx, preserved)?;
        let mut out = Vec::with_capacity(left_rows.len());

        for left_row in left_rows {
            cx.budget.check_or_err()?;
            let right_rows = self.exec_operator(
                &mut cx.with_outer(std::slice::from_ref(&left_row)),
                optional_pattern,
            )?;

            let compatible_rows: Vec<Row> = right_rows
                .into_iter()
                .filter(|right_row| {
                    for (k, v) in right_row {
                        if k.starts_with("__anon") {
                            continue;
                        }
                        if let Some(left_v) = left_row.get(k) {
                            if left_v != v {
                                return false;
                            }
                        }
                    }
                    true
                })
                .collect();

            if compatible_rows.is_empty() {
                let mut result_row = left_row;
                let null_cols = discover_operator_aliases(optional_pattern);
                for col in null_cols {
                    result_row.entry(col).or_insert(Value::Null);
                }
                cx.budget.record_row();
                out.push(result_row);
            } else {
                for right_row in compatible_rows {
                    cx.budget.check_or_err()?;
                    let mut combined = left_row.clone();
                    for (k, v) in right_row {
                        combined.insert(k, v);
                    }
                    cx.budget.record_row();
                    out.push(combined);
                }
            }
        }
        Ok(out)
    }

    pub(crate) fn exec_apply(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Apply { outer, inner } = op else {
            debug_assert!(false, "expected Apply operator");
            return Err(GqlError::Execution(
                crate::error::ExecutionError::UnsupportedFeature {
                    feature: "internal: expected Apply operator".into(),
                },
            ));
        };
        let outer_rows = self.exec_operator(cx, outer)?;
        let mut out = Vec::with_capacity(outer_rows.len());

        for outer_row in &outer_rows {
            cx.budget.check_or_err()?;
            let inner_rows =
                self.exec_operator(&mut cx.with_outer(std::slice::from_ref(outer_row)), inner)?;

            for inner_row in inner_rows {
                cx.budget.check_or_err()?;
                let mut compatible = true;
                for (k, v) in &inner_row {
                    if k.starts_with("__anon") {
                        continue;
                    }
                    // Skip internal Gremlin columns that are intentionally
                    // updated by sub-traversals (sack, side-effect, aggregation).
                    if self.gremlin_mode
                        && (k == "__sack"
                            || k.starts_with("__side_effect_")
                            || k.starts_with("__gremlin_agg_"))
                    {
                        continue;
                    }
                    if let Some(outer_v) = outer_row.get(k) {
                        if outer_v != v {
                            compatible = false;
                            break;
                        }
                    }
                }
                if !compatible {
                    continue;
                }
                let mut combined = outer_row.clone();
                for (k, v) in inner_row {
                    combined.insert(k, v);
                }
                cx.budget.record_row();
                out.push(combined);
            }
        }
        // In Gremlin, aggregate() is a global side-effect.  When it runs
        // inside local()/Apply each outer-row produces its own partial list
        // in __gremlin_agg_<key>.  We must merge these partial lists and
        // redistribute so that downstream steps (and cap()) see the full
        // accumulated set even if some rows are later filtered out.
        if self.gremlin_mode && !out.is_empty() {
            let mut agg_accumulated: std::collections::HashMap<String, Vec<Value>> =
                std::collections::HashMap::new();
            for row in &out {
                for (k, v) in row {
                    if k.starts_with("__gremlin_agg_") {
                        if let Value::List(items) = v {
                            agg_accumulated
                                .entry(k.clone())
                                .or_default()
                                .extend(items.iter().cloned());
                        }
                    }
                }
            }
            if !agg_accumulated.is_empty() {
                for row in &mut out {
                    for (k, accumulated) in &agg_accumulated {
                        if let Some(stored) = self.gremlin_side_effects.borrow().get(k).cloned() {
                            row.insert(k.clone(), stored);
                        } else {
                            row.insert(k.clone(), Value::List(accumulated.clone()));
                        }
                    }
                }
            }
        }

        Ok(out)
    }
}
