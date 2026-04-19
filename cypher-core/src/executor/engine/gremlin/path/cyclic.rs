//! Gremlin executor: `cyclicPath()` step implementation.

use crate::error::{ExecutionError, GqlError};
use crate::planner::Operator;
use crate::types::Value;

use crate::executor::engine::{ExecContext, QueryExecutor, Row};

impl<'a> QueryExecutor<'a> {
    /// Execute `GremlinCyclicPath` — filter traversers whose path contains
    /// at least one repeated object (by node/edge identity, or by property
    /// value when `by_keys` is specified).
    pub(crate) fn exec_gremlin_cyclic_path(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::GremlinCyclicPath {
            child,
            path_aliases,
            by_keys,
            from_label,
            to_label,
        } = op
        else {
            debug_assert!(false, "expected GremlinCyclicPath operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected GremlinCyclicPath operator".into(),
            }));
        };

        let input = self.exec_operator(cx, child)?;
        let mut out = Vec::with_capacity(input.len());
        for row in input {
            cx.budget.check_or_err()?;
            let mut seen = std::collections::HashSet::new();
            let mut has_cycle = false;

            let path_values = self.collect_path_values(
                path_aliases,
                &row,
                from_label.as_deref(),
                to_label.as_deref(),
            );

            if by_keys.is_empty() {
                // Default: compare by object identity (node/edge ID).
                for val in &path_values {
                    let key = match val {
                        Value::Node(nid) => format!("n:{}", nid.0),
                        Value::Edge(eid) => format!("e:{}", eid.0),
                        other => format!("v:{other:?}"),
                    };
                    if !seen.insert(key) {
                        has_cycle = true;
                        break;
                    }
                }
            } else {
                // by() modulation: compare by property value.
                let mut skip_row = false;
                for (i, val) in path_values.iter().enumerate() {
                    let by_key = &by_keys[i % by_keys.len()];
                    let projected = self.apply_path_by_key(cx, by_key, val);
                    if matches!(projected, Value::Null) {
                        // Drop traverser when property is missing.
                        skip_row = true;
                        break;
                    }
                    let key = format!("{projected:?}");
                    if !seen.insert(key) {
                        has_cycle = true;
                        break;
                    }
                }
                if skip_row {
                    continue;
                }
            }
            if has_cycle {
                out.push(row);
            }
        }
        Ok(out)
    }
}
