//! Gremlin executor: `path()` step implementation.

use crate::error::{ExecutionError, GqlError};
use crate::planner::Operator;
use crate::types::Value;

use crate::executor::engine::{ExecContext, QueryExecutor, Row};

#[allow(unused_imports)]
use super::super::helpers::*;

impl<'a> QueryExecutor<'a> {
    /// Execute `GremlinPath` — reconstruct the traversal path from the
    /// tracked aliases and return it as a list.
    pub(crate) fn exec_gremlin_path(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::GremlinPath {
            child,
            path_aliases,
            by_keys,
            productive,
            from_label,
            to_label,
        } = op
        else {
            debug_assert!(false, "expected GremlinPath operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected GremlinPath operator".into(),
            }));
        };

        let input = self.exec_operator(cx, child)?;
        let mut out = Vec::with_capacity(input.len());
        for row in input {
            cx.budget.check_or_err()?;
            let full_path: Vec<(Option<&str>, Value)> =
                if let Some(Value::List(acc)) = row.get("__gremlin_path_acc") {
                    let mut p: Vec<(Option<&str>, Value)> = acc
                        .iter()
                        .filter(|v| !matches!(v, Value::Null))
                        .map(|v| (None, v.clone()))
                        .collect();
                    if let Some(last_alias) = path_aliases.last() {
                        if let Some(val) = row.get(last_alias.as_str()) {
                            if !matches!(val, Value::Null) {
                                p.push((None, val.clone()));
                            }
                        }
                    }
                    p
                } else {
                    path_aliases
                        .iter()
                        .filter_map(|alias| {
                            row.get(alias.as_str())
                                .cloned()
                                .map(|v| (Some(alias.as_str()), v))
                        })
                        .collect()
                };

            // Apply from/to slicing: select only the sub-path between
            // the from-label and to-label (inclusive).
            let raw_path: Vec<Value> = if from_label.is_some() || to_label.is_some() {
                let start = if let Some(fl) = from_label {
                    if let Some(alias) = self.resolve_label_alias(fl, path_aliases, &row) {
                        full_path
                            .iter()
                            .position(|(a, _)| *a == Some(alias.as_str()))
                    } else {
                        None
                    }
                } else {
                    Some(0)
                };
                let end = if let Some(tl) = to_label {
                    if let Some(alias) = self.resolve_label_alias(tl, path_aliases, &row) {
                        full_path
                            .iter()
                            .rposition(|(a, _)| *a == Some(alias.as_str()))
                            .map(|i| i + 1)
                    } else {
                        None
                    }
                } else {
                    Some(full_path.len())
                };
                match (start, end) {
                    (Some(s), Some(e)) if s <= e => {
                        full_path[s..e].iter().map(|(_, v)| v.clone()).collect()
                    }
                    _ => full_path.into_iter().map(|(_, v)| v).collect(),
                }
            } else {
                full_path.into_iter().map(|(_, v)| v).collect()
            };

            // Apply by-key modulation: each element uses by_keys[i % len].
            let path = if by_keys.is_empty() {
                raw_path
            } else {
                let transformed: Vec<Value> = raw_path
                    .into_iter()
                    .enumerate()
                    .map(|(i, val)| {
                        let key = &by_keys[i % by_keys.len()];
                        self.apply_path_by_key(cx, key, &val)
                    })
                    .collect();
                // Drop the traverser if any by-key resolution produced Null
                if !*productive && transformed.iter().any(|v| matches!(v, Value::Null)) {
                    continue;
                }
                transformed
            };

            // Carry forward input columns (preserves as() labels for
            // subsequent select() after path()).
            let mut result_row = row.clone();
            result_row.insert("path".to_string(), Value::List(path));
            out.push(result_row);
        }
        Ok(out)
    }
}
