//! UNWIND operator: expands list values into rows.

use crate::error::{ExecutionError, GqlError};
use crate::planner::Operator;
use crate::types::Value;

use crate::executor::engine::{ExecContext, QueryExecutor, Row};

impl<'a> QueryExecutor<'a> {
    pub(crate) fn exec_unwind(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Unwind {
            list_expr,
            alias,
            child,
        } = op
        else {
            debug_assert!(false, "expected Unwind operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected Unwind operator".into(),
            }));
        };
        let input = self.exec_operator(cx, child)?;
        let mut out = Vec::with_capacity(input.len());

        for row in input {
            cx.budget.check_or_err()?;
            let ctx = self.build_filter_ctx(cx.graph, &row)?;
            let list_val = self.eval_with_storage(cx.graph, list_expr, &row, &ctx)?;
            match list_val {
                Value::List(items) => {
                    for item in items {
                        cx.budget.check_or_err()?;
                        let mut new_row = row.clone();
                        new_row.insert(alias.0.clone(), item);
                        cx.budget.record_row();
                        out.push(new_row);
                    }
                }
                Value::Null => {
                    // UNWIND null produces zero rows (Cypher semantics)
                }
                other => {
                    // Gremlin unfold: treat Map as list of entries, scalar as single-element list
                    match other {
                        Value::Map(m) => {
                            for (k, v) in m.iter() {
                                cx.budget.check_or_err()?;
                                let mut entry = indexmap::IndexMap::new();
                                entry.insert(k.clone(), v.clone());
                                let mut new_row = row.clone();
                                new_row.insert(alias.0.clone(), Value::Map(entry));
                                cx.budget.record_row();
                                out.push(new_row);
                            }
                        }
                        Value::Str(_)
                        | Value::Int(_)
                        | Value::Float(_)
                        | Value::Bool(_)
                        | Value::Node(_)
                        | Value::Edge(_) => {
                            let mut new_row = row.clone();
                            new_row.insert(alias.0.clone(), other);
                            cx.budget.record_row();
                            out.push(new_row);
                        }
                        _ => {
                            return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                                expected: "List for UNWIND".to_string(),
                                actual: "unknown type".to_string(),
                            }));
                        }
                    }
                }
            }
        }
        Ok(out)
    }
}
