//! Gremlin executor: `path` step implementations.

use indexmap::IndexMap;

use crate::ast::Identifier;
use crate::error::{ExecutionError, GqlError};
use crate::planner::Operator;
use crate::types::Value;

use crate::executor::engine::{ExecContext, QueryExecutor, Row};

#[allow(unused_imports)]
use super::helpers::*;

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
                // Find indices matching from/to labels by checking row aliases
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

    /// Execute `GremlinTree` — build a tree structure from traversal paths.
    pub(crate) fn exec_gremlin_tree(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::GremlinTree {
            child,
            path_aliases,
            side_effect_key,
            alias,
        } = op
        else {
            debug_assert!(false, "expected GremlinTree operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected GremlinTree operator".into(),
            }));
        };

        let mut input = self.exec_operator(cx, child)?;

        let agg_col = side_effect_key
            .as_ref()
            .map(|k| format!("__gremlin_agg_{k}"));

        // Retrieve existing tree from side-effect store (accumulation in local()).
        let mut tree: IndexMap<Box<str>, Value> = if let Some(col) = &agg_col {
            let se = self.gremlin_side_effects.borrow();
            if let Some(Value::Map(existing)) = se.get(col) {
                existing.clone()
            } else {
                IndexMap::new()
            }
        } else {
            IndexMap::new()
        };

        for row in &input {
            cx.budget.check_or_err()?;
            let path: Vec<Value> = path_aliases
                .iter()
                .filter_map(|a| row.get(a.as_str()).cloned())
                .filter(|v| !matches!(v, Value::Null))
                .collect();
            Self::insert_tree_path(&mut tree, &path);
        }

        let tree_value = Value::Map(tree);

        if let Some(_key) = side_effect_key {
            // Side-effect mode: store tree and pass traversers through.
            let col = agg_col.unwrap();
            self.gremlin_side_effects
                .borrow_mut()
                .insert(col.clone(), tree_value.clone());
            for row in &mut input {
                row.insert(col.clone(), tree_value.clone());
            }
            Ok(input)
        } else {
            // Barrier mode: return single row with the tree Map.
            let mut row = Row::new();
            row.insert(alias.as_str().to_string(), tree_value);
            Ok(vec![row])
        }
    }

    /// Insert a path into a nested Map tree structure.
    fn insert_tree_path(tree: &mut IndexMap<Box<str>, Value>, path: &[Value]) {
        if path.is_empty() {
            return;
        }
        let key: Box<str> = format!("{}", path[0]).into();
        let subtree = tree
            .entry(key)
            .or_insert_with(|| Value::Map(IndexMap::new()));
        if let Value::Map(children) = subtree {
            Self::insert_tree_path(children, &path[1..]);
        }
    }

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
                    let projected = self.apply_path_by_key(cx, by_key, val);
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

    /// Collect path values from a row, optionally slicing between from/to labels.
    fn collect_path_values(
        &self,
        path_aliases: &[Identifier],
        row: &Row,
        from_label: Option<&str>,
        to_label: Option<&str>,
    ) -> Vec<Value> {
        // Build full path with alias tags for from/to slicing.
        let full_path: Vec<(&str, Value)> =
            if let Some(Value::List(acc)) = row.get("__gremlin_path_acc") {
                let mut p: Vec<(&str, Value)> = acc
                    .iter()
                    .filter(|v| !matches!(v, Value::Null))
                    .map(|v| ("", v.clone()))
                    .collect();
                if let Some(last_alias) = path_aliases.last() {
                    if let Some(val) = row.get(last_alias.as_str()) {
                        if !matches!(val, Value::Null) {
                            p.push((last_alias.as_str(), val.clone()));
                        }
                    }
                }
                p
            } else {
                path_aliases
                    .iter()
                    .filter_map(|alias| {
                        row.get(alias.as_str())
                            .filter(|v| !matches!(v, Value::Null))
                            .map(|v| (alias.as_str(), v.clone()))
                    })
                    .collect()
            };

        if from_label.is_none() && to_label.is_none() {
            return full_path.into_iter().map(|(_, v)| v).collect();
        }

        // Find the index of a label in path_aliases. The label is an
        // internal alias (resolved at compile time), so direct comparison
        // against path_aliases entries works.
        let find_idx = |label: &str, from_end: bool| -> Option<usize> {
            // Direct match against path_aliases (works for non-accumulator paths)
            if from_end {
                path_aliases.iter().rposition(|a| a.as_str() == label)
            } else {
                path_aliases.iter().position(|a| a.as_str() == label)
            }
        };

        let start = from_label.and_then(|l| find_idx(l, false)).unwrap_or(0);
        let end = to_label
            .and_then(|l| find_idx(l, true))
            .map(|i| i + 1)
            .unwrap_or(full_path.len());

        if start <= end && end <= full_path.len() {
            full_path[start..end]
                .iter()
                .map(|(_, v)| v.clone())
                .collect()
        } else {
            full_path.into_iter().map(|(_, v)| v).collect()
        }
    }
}
