//! `GremlinMapSlice` — slice Map/List values in each row.

use indexmap::IndexMap;

use crate::error::{ExecutionError, GqlError};
use crate::executor::engine::{ExecContext, QueryExecutor, Row};
use crate::planner::Operator;
use crate::types::Value;

impl<'a> QueryExecutor<'a> {
    /// Execute `GremlinMapSlice` — slice Map/List values in each row.
    pub(crate) fn exec_gremlin_map_slice(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::GremlinMapSlice {
            alias,
            lo,
            hi,
            child,
        } = op
        else {
            debug_assert!(false, "expected GremlinMapSlice operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected GremlinMapSlice operator".into(),
            }));
        };
        let input = self.exec_operator(cx, child)?;
        let mut out = Vec::with_capacity(input.len());
        for mut row in input {
            cx.budget.check_or_err()?;
            if let Some(val) = row.get(&alias.0) {
                let sliced = match val {
                    Value::Map(m) => {
                        let new_map: IndexMap<Box<str>, Value> = m
                            .iter()
                            .skip(*lo)
                            .take(hi - lo)
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect();
                        Value::Map(new_map)
                    }
                    Value::List(l) => {
                        let new_list: Vec<Value> =
                            l.iter().skip(*lo).take(hi - lo).cloned().collect();
                        Value::List(new_list)
                    }
                    other => other.clone(),
                };
                row.insert(alias.0.clone(), sliced);
            }
            out.push(row);
        }
        Ok(out)
    }
}
