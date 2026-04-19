//! `GremlinProject` — evaluate sub-traversals per key for each input row.

use indexmap::IndexMap;

use crate::error::{ExecutionError, GqlError};
use crate::executor::engine::{ExecContext, QueryExecutor, Row};
use crate::planner::Operator;
use crate::types::Value;

impl<'a> QueryExecutor<'a> {
    /// Execute `GremlinProject` — evaluate sub-traversals per key for each input row.
    pub(crate) fn exec_gremlin_project(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::GremlinProject {
            entries,
            child,
            current_alias,
            out_alias,
            productive,
        } = op
        else {
            debug_assert!(false, "expected GremlinProject operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected GremlinProject operator".into(),
            }));
        };

        let input = self.exec_operator(cx, child)?;
        let mut out = Vec::with_capacity(input.len());
        for row in &input {
            cx.budget.check_or_err()?;
            let mut map = IndexMap::new();
            for (key, sub_op, out_alias, by_is_identity) in entries {
                // Feed the current row as a seed and evaluate the sub-traversal
                self.foreach_seed.borrow_mut().replace(vec![row.clone()]);
                let sub_rows = self.exec_operator(cx, sub_op)?;
                self.foreach_seed.borrow_mut().take();
                // Take the first result value (project picks the first traverser)
                let val = if let Some(sub_row) = sub_rows.first() {
                    sub_row
                        .get(out_alias.as_str())
                        .or_else(|| {
                            // Fall back to current_alias if out_alias not found
                            sub_row.get(current_alias.as_str())
                        })
                        .cloned()
                        .unwrap_or(Value::Null)
                } else {
                    Value::Null
                };
                // Omit keys where by-modulation produced Null (property
                // doesn't exist), unless ProductiveByStrategy is active or
                // the by() is the identity (empty by()). When by_is_identity
                // is true, a null value means the traverser itself is null —
                // this is a legitimate null that must appear in the result map.
                if matches!(val, Value::Null) && !*productive && !by_is_identity {
                    continue;
                }
                map.insert(key.clone().into_boxed_str(), val);
            }
            // Store the projected Map as both individual columns and a combined Map
            let map_val = Value::Map(map.clone());
            let mut out_row = row.clone();
            for (k, v) in &map {
                out_row.insert(k.to_string(), v.clone());
            }
            out_row.insert(out_alias.0.clone(), map_val);
            out.push(out_row);
        }
        Ok(out)
    }
}
