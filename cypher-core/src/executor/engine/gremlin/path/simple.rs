//! Gremlin executor: `simplePath()` step implementation.

use crate::error::{ExecutionError, GqlError};
use crate::planner::Operator;
use crate::types::Value;

use crate::executor::engine::{ExecContext, QueryExecutor, Row};

impl<'a> QueryExecutor<'a> {
    /// Execute `GremlinSimplePath` — filter traversers whose path contains
    /// repeated objects (by node/edge identity).
    pub(crate) fn exec_gremlin_simple_path(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::GremlinSimplePath {
            child,
            path_aliases,
            by_keys,
            from_label,
            to_label,
        } = op
        else {
            debug_assert!(false, "expected GremlinSimplePath operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected GremlinSimplePath operator".into(),
            }));
        };

        let input = self.exec_operator(cx, child)?;
        let mut out = Vec::with_capacity(input.len());
        for row in input {
            cx.budget.check_or_err()?;
            let mut seen = std::collections::HashSet::new();
            let mut is_simple = true;

            let path_values = self.collect_path_values(
                path_aliases,
                &row,
                from_label.as_deref(),
                to_label.as_deref(),
            );

            if by_keys.is_empty() {
                for val in &path_values {
                    let key = match val {
                        Value::Node(nid) => format!("n:{}", nid.0),
                        Value::Edge(eid) => format!("e:{}", eid.0),
                        other => format!("v:{other:?}"),
                    };
                    if !seen.insert(key) {
                        is_simple = false;
                        break;
                    }
                }
            } else {
                let mut skip_row = false;
                for (i, val) in path_values.iter().enumerate() {
                    let by_key = &by_keys[i % by_keys.len()];
                    let projected = self.apply_path_by_key(cx, by_key, val)?;
                    if matches!(projected, Value::Null) {
                        skip_row = true;
                        break;
                    }
                    let key = format!("{projected:?}");
                    if !seen.insert(key) {
                        is_simple = false;
                        break;
                    }
                }
                if skip_row {
                    is_simple = false;
                }
            }
            if is_simple {
                out.push(row);
            }
        }
        Ok(out)
    }
}
