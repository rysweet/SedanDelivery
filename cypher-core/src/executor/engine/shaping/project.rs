//! `Project` operator: evaluates a list of expressions into a new row,
//! with Gremlin-mode tolerance for missing carry-forward variables.

use crate::ast::Expr;
use crate::error::GqlError;
use crate::planner::Operator;
use crate::types::Value;

use super::super::helpers::{expr_to_column_name, inject_parameters};
use super::super::{ExecContext, QueryExecutor, Row};

impl<'a> QueryExecutor<'a> {
    pub(crate) fn exec_project(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Project { items, child } = op else {
            debug_assert!(false, "expected Project operator");
            return Err(GqlError::Execution(
                crate::error::ExecutionError::UnsupportedFeature {
                    feature: "internal: expected Project operator".into(),
                },
            ));
        };
        let input = self.exec_operator(cx, child)?;
        let mut out = Vec::with_capacity(input.len());

        for row in &input {
            cx.budget.check_or_err()?;
            let ctx = self.build_filter_ctx(cx.graph, row)?;
            let mut new_row = Row::new();
            for (expr, alias) in items {
                match self.eval_with_storage(cx.graph, expr, row, &ctx) {
                    Ok(val) => {
                        let col_name = match alias {
                            Some(id) => id.0.clone(),
                            None => expr_to_column_name(expr),
                        };
                        new_row.insert(col_name, val);
                    }
                    Err(e) => {
                        // In Gremlin mode, label_carry_items() may reference
                        // step-alias variables (v_N, e_N) that are not present
                        // in all row contexts (e.g., after multi-key select).
                        // Skip these carry-forward items silently rather than
                        // erroring — they are auxiliary and not critical.
                        if self.gremlin_mode {
                            if let Expr::Variable(id) = expr {
                                let name = id.0.as_str();
                                // loops() outside repeat context: default to 0
                                if name == "__gremlin_loop_count"
                                    || name.starts_with("__gremlin_loop_count_")
                                {
                                    let col_name = match alias {
                                        Some(a) => a.0.clone(),
                                        None => name.to_string(),
                                    };
                                    new_row.insert(col_name, Value::Int(0));
                                    continue;
                                }
                                // path_acc outside repeat context: skip silently.
                                // The path executor handles missing path_acc by
                                // falling back to path_aliases.
                                if name == "__gremlin_path_acc" {
                                    continue;
                                }
                                if name.starts_with("__side_effect_") {
                                    continue;
                                }
                                if alias.as_ref() == Some(id) {
                                    let is_step_alias = name
                                        .rsplit('_')
                                        .next()
                                        .map(|s| {
                                            !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())
                                        })
                                        .unwrap_or(false);
                                    if is_step_alias {
                                        continue;
                                    }
                                }
                            }
                        }
                        // If we already produced at least one row, treat
                        // this as a streaming error: return the partial
                        // rows and stash the error for the caller.
                        if !out.is_empty() {
                            *self.streaming_error.borrow_mut() = Some(e);
                            return Ok(out);
                        }
                        // No partial rows yet: propagate the error
                        // immediately (standard behaviour).
                        return Err(e);
                    }
                }
            }
            // In Gremlin mode, preserve the __sack column through
            // intermediate projections so sack() can read it later.
            if self.gremlin_mode {
                if let Some(v) = row.get("__sack") {
                    if !new_row.contains_key("__sack") {
                        new_row.insert("__sack".to_string(), v.clone());
                    }
                }
            }
            // Preserve query parameters through scope changes.
            inject_parameters(&mut new_row, &self.parameters);
            cx.budget.record_row();
            out.push(new_row);
        }
        Ok(out)
    }
}
