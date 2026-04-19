//! Gremlin executor: `branch` step implementations.

use crate::error::{ExecutionError, GqlError};
use crate::planner::Operator;
use crate::types::Value;

use crate::executor::engine::{ExecContext, QueryExecutor, Row};

#[allow(unused_imports)]
use super::helpers::*;

impl<'a> QueryExecutor<'a> {
    /// Execute `Choose` — evaluate the predicate for each input row and route
    /// to `if_true` or `if_false` accordingly.
    pub(crate) fn exec_choose(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Choose {
            predicate,
            if_true,
            if_false,
            child,
            true_alias,
            false_alias,
            output_alias,
        } = op
        else {
            debug_assert!(false, "expected Choose operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected Choose operator".into(),
            }));
        };

        let input = self.exec_operator(cx, child)?;
        let mut out = Vec::with_capacity(input.len());

        // Phase 1: Classify each input row as true, false, or pass-through.
        let mut true_rows: Vec<Row> = Vec::new();
        let mut false_rows: Vec<Row> = Vec::new();

        for row in input {
            cx.budget.check_or_err()?;

            self.foreach_seed.borrow_mut().replace(vec![row.clone()]);
            let pred_result = self.exec_operator(cx, predicate)?;
            self.foreach_seed.borrow_mut().take();

            let is_true = !pred_result.is_empty();
            if is_true {
                true_rows.push(row);
            } else if if_false.is_some() {
                false_rows.push(row);
            } else {
                // No else branch — pass through unchanged.
                if let (Some(out_alias), Some(fa)) = (output_alias, false_alias) {
                    let mut prow = row;
                    if let Some(val) = prow.get(fa.0.as_str()).cloned() {
                        prow.insert(out_alias.0.to_string(), val);
                    }
                    out.push(prow);
                } else {
                    out.push(row);
                }
            }
        }

        // Phase 2: Batch-execute true branch with all true rows.
        if !true_rows.is_empty() {
            self.foreach_seed.borrow_mut().replace(true_rows);
            let branch_rows = self.exec_operator(cx, if_true)?;
            self.foreach_seed.borrow_mut().take();

            if let Some(out_alias) = output_alias {
                if let Some(src_alias) = true_alias {
                    for mut brow in branch_rows {
                        if let Some(val) = brow.get(src_alias.0.as_str()).cloned() {
                            brow.insert(out_alias.0.to_string(), val);
                        }
                        out.push(brow);
                    }
                } else {
                    out.extend(branch_rows);
                }
            } else {
                out.extend(branch_rows);
            }
        }

        // Phase 3: Batch-execute false branch with all false rows.
        if !false_rows.is_empty() {
            if let Some(f) = if_false {
                self.foreach_seed.borrow_mut().replace(false_rows);
                let branch_rows = self.exec_operator(cx, f)?;
                self.foreach_seed.borrow_mut().take();

                if let Some(out_alias) = output_alias {
                    if let Some(src_alias) = false_alias {
                        for mut brow in branch_rows {
                            if let Some(val) = brow.get(src_alias.0.as_str()).cloned() {
                                brow.insert(out_alias.0.to_string(), val);
                            }
                            out.push(brow);
                        }
                    } else {
                        out.extend(branch_rows);
                    }
                } else {
                    out.extend(branch_rows);
                }
            }
        }

        Ok(out)
    }

    /// Execute `ChooseMap` — value-based dispatch.
    /// Evaluate the selector for each input row, compare the result against
    /// option values, and route to the matching branch.
    ///
    /// Uses batch execution: all rows matching a given option are collected
    /// and executed together so that aggregating steps (fold, count, limit)
    /// operate on the collective stream.
    pub(crate) fn exec_choose_map(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::ChooseMap {
            selector,
            selector_alias,
            options,
            child,
            output_alias,
        } = op
        else {
            debug_assert!(false, "expected ChooseMap operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected ChooseMap operator".into(),
            }));
        };

        let input = self.exec_operator(cx, child)?;
        let mut out = Vec::with_capacity(input.len());

        // Phase 1: Classify each input row — determine which option(s) it matches.
        // For each row, store (matched_option_indices, is_unproductive).
        let mut option_batches: Vec<Vec<Row>> = vec![Vec::new(); options.len()];
        let mut default_rows: Vec<Row> = Vec::new();
        let mut unproductive_rows: Vec<Row> = Vec::new();
        let mut identity_rows: Vec<Row> = Vec::new();

        // Find default and unproductive option indices.
        let mut default_idx: Option<usize> = None;
        let mut unproductive_idx: Option<usize> = None;
        for (idx, (match_val, _, _)) in options.iter().enumerate() {
            match match_val {
                None if default_idx.is_none() => {
                    default_idx = Some(idx);
                }
                Some(Value::Str(s))
                    if (s.as_ref() == "Pick.unproductive" || s.as_ref() == "unproductive")
                        && unproductive_idx.is_none() =>
                {
                    unproductive_idx = Some(idx);
                }
                _ => {}
            }
        }

        for row in input {
            cx.budget.check_or_err()?;

            // Evaluate the selector to get a dispatch value.
            self.foreach_seed.borrow_mut().replace(vec![row.clone()]);
            let sel_rows = self.exec_operator(cx, selector)?;
            self.foreach_seed.borrow_mut().take();

            let selector_unproductive = sel_rows.is_empty()
                || sel_rows.first().is_none_or(|r| {
                    r.get(selector_alias.0.as_str())
                        .is_none_or(|v| matches!(v, Value::Null))
                        && r.values().all(|v| matches!(v, Value::Null))
                });

            let sel_value = sel_rows
                .first()
                .and_then(|r| r.get(selector_alias.0.as_str()).cloned())
                .or_else(|| sel_rows.first().and_then(|r| r.values().next().cloned()))
                .map(|v| match v {
                    Value::List(ref items) if items.len() == 1 => items[0].clone(),
                    other => other,
                });

            if selector_unproductive {
                if unproductive_idx.is_some() {
                    unproductive_rows.push(row);
                } else {
                    identity_rows.push(row);
                }
                continue;
            }

            // Find matching option(s).
            let mut matched_any = false;
            for (idx, (match_val, _, _)) in options.iter().enumerate() {
                match match_val {
                    None => {} // default handled below
                    Some(v) => {
                        if let Value::Str(s) = v {
                            if s.as_ref() == "Pick.unproductive" || s.as_ref() == "unproductive" {
                                continue;
                            }
                            if s.as_ref() == "Pick.any" || s.as_ref() == "any" {
                                option_batches[idx].push(row.clone());
                                matched_any = true;
                                continue;
                            }
                        }
                        let matches = match (sel_value.as_ref(), Some(v)) {
                            (Some(Value::Int(a)), Some(Value::Int(b))) => a == b,
                            (Some(Value::Float(a)), Some(Value::Float(b))) => (a - b).abs() < 1e-9,
                            (Some(Value::Int(a)), Some(Value::Float(b)))
                            | (Some(Value::Float(b)), Some(Value::Int(a))) => {
                                (*a as f64 - b).abs() < 1e-9
                            }
                            (Some(sel), Some(Value::Str(token))) if token.starts_with("__.") => {
                                self.eval_choose_map_traversal_option(cx.graph, token, sel)?
                            }
                            (Some(sel), Some(Value::Str(pred_str)))
                                if pred_str.starts_with("P.") =>
                            {
                                eval_gremlin_predicate(pred_str, sel)
                            }
                            (Some(a), Some(b)) => a == b,
                            _ => false,
                        };
                        if matches {
                            option_batches[idx].push(row.clone());
                            matched_any = true;
                            let is_predicate_match =
                                matches!(v, Value::Str(s) if s.starts_with("P."));
                            if is_predicate_match {
                                break;
                            }
                        }
                    }
                }
            }

            if !matched_any {
                default_rows.push(row);
            }
        }

        // Phase 2: Batch-execute each option branch.
        for (idx, (_, branch, branch_alias)) in options.iter().enumerate() {
            let batch = &option_batches[idx];
            if batch.is_empty() {
                continue;
            }
            self.foreach_seed.borrow_mut().replace(batch.clone());
            let branch_rows = self.exec_operator(cx, branch)?;
            self.foreach_seed.borrow_mut().take();
            for mut brow in branch_rows {
                if let (Some(out_a), Some(src_a)) = (output_alias, branch_alias.as_ref()) {
                    if let Some(val) = brow.get(src_a.0.as_str()).cloned() {
                        brow.insert(out_a.0.to_string(), val);
                    }
                }
                out.push(brow);
            }
        }

        // Execute unproductive rows through unproductive branch.
        if !unproductive_rows.is_empty() {
            if let Some(idx) = unproductive_idx {
                let (_, branch, branch_alias) = &options[idx];
                self.foreach_seed.borrow_mut().replace(unproductive_rows);
                let branch_rows = self.exec_operator(cx, branch)?;
                self.foreach_seed.borrow_mut().take();
                for mut brow in branch_rows {
                    if let (Some(out_a), Some(src_a)) = (output_alias, branch_alias.as_ref()) {
                        if let Some(val) = brow.get(src_a.0.as_str()).cloned() {
                            brow.insert(out_a.0.to_string(), val);
                        }
                    }
                    out.push(brow);
                }
            }
        }

        // Execute default rows through default branch.
        if !default_rows.is_empty() {
            if let Some(idx) = default_idx {
                let (_, branch, branch_alias) = &options[idx];
                self.foreach_seed.borrow_mut().replace(default_rows);
                let branch_rows = self.exec_operator(cx, branch)?;
                self.foreach_seed.borrow_mut().take();
                for mut brow in branch_rows {
                    if let (Some(out_a), Some(src_a)) = (output_alias, branch_alias.as_ref()) {
                        if let Some(val) = brow.get(src_a.0.as_str()).cloned() {
                            brow.insert(out_a.0.to_string(), val);
                        }
                    }
                    out.push(brow);
                }
            } else {
                // No default branch → identity (pass through)
                out.extend(default_rows);
            }
        }

        // Identity pass-through for unproductive rows without an unproductive branch.
        out.extend(identity_rows);

        Ok(out)
    }

    /// Execute `Coalesce` — try each alternative in order, return results from
    /// the first one that produces non-empty output.
    pub(crate) fn exec_coalesce(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Coalesce {
            alternatives,
            child,
        } = op
        else {
            debug_assert!(false, "expected Coalesce operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected Coalesce operator".into(),
            }));
        };

        let input = self.exec_operator(cx, child)?;
        let mut out = Vec::with_capacity(input.len());

        for row in input {
            cx.budget.check_or_err()?;
            let mut matched = false;

            for alt in alternatives {
                self.foreach_seed.borrow_mut().replace(vec![row.clone()]);
                let alt_rows = self.exec_operator(cx, alt)?;
                self.foreach_seed.borrow_mut().take();

                if !alt_rows.is_empty() {
                    for mut alt_row in alt_rows {
                        for (key, value) in &row {
                            alt_row.entry(key.clone()).or_insert_with(|| value.clone());
                        }
                        out.push(alt_row);
                    }
                    matched = true;
                    break;
                }
            }

            if !matched {
                // No alternative matched — drop the traverser.
            }
        }
        Ok(out)
    }

    fn eval_choose_map_traversal_option(
        &self,
        graph: &str,
        token: &str,
        value: &Value,
    ) -> Result<bool, GqlError> {
        let Some(step) = token.strip_prefix("__.") else {
            return Ok(false);
        };
        if step == "identity()" {
            return Ok(!matches!(value, Value::Null));
        }
        if let Some(args) = step
            .strip_prefix("hasLabel(")
            .and_then(|rest| rest.strip_suffix(')'))
        {
            let parts = parse_gremlin_option_args(args);
            let Some(label) = parts.first() else {
                return Ok(false);
            };
            let label = trim_gremlin_option_string(label);
            return match value {
                Value::Node(node_id) => Ok(self
                    .cached_get_node(graph, *node_id)?
                    .is_some_and(|node| node.labels.iter().any(|l| l == label))),
                _ => Ok(false),
            };
        }
        if let Some(args) = step
            .strip_prefix("has(")
            .and_then(|rest| rest.strip_suffix(')'))
        {
            let parts = parse_gremlin_option_args(args);
            if parts.is_empty() {
                return Ok(false);
            }
            let key = trim_gremlin_option_string(&parts[0]);
            let actual = match value {
                Value::Node(node_id) => self
                    .cached_get_node(graph, *node_id)?
                    .and_then(|node| node.properties.get(key).cloned()),
                Value::Edge(edge_id) => self
                    .cached_get_edge(graph, *edge_id)?
                    .and_then(|edge| edge.properties.get(key).cloned()),
                Value::Map(map) => map.get(key).cloned(),
                _ => None,
            };
            return if parts.len() == 1 {
                Ok(actual.is_some_and(|v| !matches!(v, Value::Null)))
            } else if let Some(actual) = actual {
                Ok(actual == parse_gremlin_option_value(&parts[1]))
            } else {
                Ok(false)
            };
        }
        Ok(false)
    }
}
