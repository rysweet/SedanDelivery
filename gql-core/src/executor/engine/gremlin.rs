//! Gremlin traversal operator implementations.

use crate::ast::{Expr, Identifier};
use crate::error::{ExecutionError, GqlError};
use crate::planner::Operator;
use crate::types::Value;
use indexmap::IndexMap;

use super::helpers::inject_parameters;
use crate::executor::engine::{ExecContext, QueryExecutor, Row};

impl<'a> QueryExecutor<'a> {
    /// Execute `Repeat` — run `body` repeatedly, either a fixed number of
    /// `times` or until the `until` predicate produces results.  When `emit`
    /// is true, intermediate results from each iteration are included.
    pub(crate) fn exec_repeat(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Repeat {
            body,
            times,
            until,
            emit,
            emit_before,
            emit_predicate,
            remap,
            child,
        } = op
        else {
            debug_assert!(false, "expected Repeat operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected Repeat operator".into(),
            }));
        };

        let mut current = self.exec_operator(cx, child)?;
        let mut collected: Vec<Row> = Vec::new();

        let max_iterations = times.unwrap_or(256);
        for iteration in 0..max_iterations {
            cx.budget.check_or_err()?;

            // Pre-emit: collect traversers before body execution.
            // Only when emit_before is true (emit() appeared before repeat()).
            if *emit && *emit_before {
                self.collect_emitted(cx, &current, emit_predicate, &mut collected)?;
            }

            // Feed current rows through the body.
            let mut next = Vec::new();
            for row in &current {
                cx.budget.check_or_err()?;
                let mut seed_row = row.clone();
                if let Some((from, _)) = remap {
                    seed_row.remove(from.as_str());
                }
                self.foreach_seed.borrow_mut().replace(vec![seed_row]);
                let body_rows = self.exec_operator(cx, body)?;
                self.foreach_seed.borrow_mut().take();
                next.extend(body_rows);
            }

            if next.is_empty() {
                // Body produced no results — stop iterating.
                // For non-emit mode without until, don't add the pre-body
                // rows since the traversal terminated before completing all
                // iterations. (With until, results are collected separately.)
                if !*emit && until.is_none() {
                    return Ok(collected);
                }
                break;
            }

            // Remap body output alias back to seed alias for the next iteration.
            if let Some((from, to)) = remap {
                for row in &mut next {
                    if let Some(val) = row.get(from.as_str()).cloned() {
                        row.insert(to.clone(), val);
                    }
                }
            }

            current = next;

            // Post-emit: collect body results after execution.
            // Only when emit is true but emit_before is false (emit() after repeat()).
            if *emit && !*emit_before {
                self.collect_emitted(cx, &current, emit_predicate, &mut collected)?;
            }

            // Check the until predicate per-traverser: traversers that match
            // the until condition are collected and removed from further iteration.
            if let Some(until_op) = until {
                let mut continuing = Vec::new();
                for row in current.drain(..) {
                    self.foreach_seed.borrow_mut().replace(vec![row.clone()]);
                    let result = self.exec_operator(cx, until_op)?;
                    self.foreach_seed.borrow_mut().take();
                    if !result.is_empty() {
                        // This traverser matched until — collect it
                        collected.push(row);
                    } else {
                        continuing.push(row);
                    }
                }
                current = continuing;
                if current.is_empty() {
                    return Ok(collected);
                }
            }

            // For fixed-count mode, check if we've done enough iterations.
            if let Some(t) = times {
                if iteration + 1 >= *t {
                    break;
                }
            }
        }

        // For non-emit or pre-emit mode, add final results.
        // For post-emit mode, everything was already collected in the loop.
        if !*emit || *emit_before {
            collected.extend(current);
        }
        Ok(collected)
    }

    /// Collect rows into `collected`, optionally filtering by emit predicate.
    fn collect_emitted(
        &self,
        cx: &mut ExecContext<'_>,
        rows: &[Row],
        emit_predicate: &Option<Box<Operator>>,
        collected: &mut Vec<Row>,
    ) -> Result<(), GqlError> {
        if let Some(pred_op) = emit_predicate {
            for row in rows {
                self.foreach_seed.borrow_mut().replace(vec![row.clone()]);
                let result = self.exec_operator(cx, pred_op)?;
                self.foreach_seed.borrow_mut().take();
                if !result.is_empty() {
                    collected.push(row.clone());
                }
            }
        } else {
            collected.extend(rows.iter().cloned());
        }
        Ok(())
    }

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

        for row in input {
            cx.budget.check_or_err()?;

            // Evaluate predicate: if it produces rows, the condition is true.
            self.foreach_seed.borrow_mut().replace(vec![row.clone()]);
            let pred_result = self.exec_operator(cx, predicate)?;
            self.foreach_seed.borrow_mut().take();

            let is_true = !pred_result.is_empty();
            let branch = if is_true {
                if_true
            } else if let Some(f) = if_false {
                f
            } else {
                // No else branch — pass through unchanged.
                // Copy the seed alias value to the output alias so subsequent
                // steps can find the traverser under the unified alias.
                if let (Some(out_alias), Some(fa)) = (output_alias, false_alias) {
                    let mut prow = row;
                    if let Some(val) = prow.get(fa.0.as_str()).cloned() {
                        prow.insert(out_alias.0.to_string(), val);
                    }
                    out.push(prow);
                } else {
                    out.push(row);
                }
                continue;
            };

            self.foreach_seed.borrow_mut().replace(vec![row]);
            let branch_rows = self.exec_operator(cx, branch)?;
            self.foreach_seed.borrow_mut().take();

            // Copy branch-specific result column to the unified output alias.
            if let Some(out_alias) = output_alias {
                let src = if is_true { true_alias } else { false_alias };
                if let Some(src_alias) = src {
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
        Ok(out)
    }

    /// Execute `ChooseMap` — value-based dispatch.
    /// Evaluate the selector for each input row, compare the result against
    /// option values, and route to the matching branch.
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
        } = op
        else {
            debug_assert!(false, "expected ChooseMap operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected ChooseMap operator".into(),
            }));
        };

        let input = self.exec_operator(cx, child)?;
        let mut out = Vec::with_capacity(input.len());

        for row in input {
            cx.budget.check_or_err()?;

            // Evaluate the selector to get a dispatch value.
            self.foreach_seed.borrow_mut().replace(vec![row.clone()]);
            let sel_rows = self.exec_operator(cx, selector)?;
            self.foreach_seed.borrow_mut().take();

            // Extract the selector's result value using the known alias.
            // Unwrap single-element lists (e.g. labels() returns List(["person"])) to
            // a scalar so option matching works against plain string values.
            let sel_value = sel_rows
                .first()
                .and_then(|r| r.get(selector_alias.0.as_str()).cloned())
                .or_else(|| sel_rows.first().and_then(|r| r.values().next().cloned()))
                .map(|v| match v {
                    Value::List(ref items) if items.len() == 1 => items[0].clone(),
                    other => other,
                });

            // Find the matching option branch.
            let mut matched_branch = None;
            let mut default_branch = None;
            for (match_val, branch) in options {
                match match_val {
                    None => default_branch = Some(branch),
                    Some(v) => {
                        let matches = match (sel_value.as_ref(), Some(v)) {
                            (Some(Value::Int(a)), Some(Value::Int(b))) => a == b,
                            (Some(Value::Float(a)), Some(Value::Float(b))) => (a - b).abs() < 1e-9,
                            (Some(Value::Int(a)), Some(Value::Float(b)))
                            | (Some(Value::Float(b)), Some(Value::Int(a))) => {
                                (*a as f64 - b).abs() < 1e-9
                            }
                            // Predicate matching: option value encodes a predicate string
                            (Some(sel), Some(Value::Str(pred_str)))
                                if pred_str.starts_with("P.") =>
                            {
                                eval_gremlin_predicate(pred_str, sel)
                            }
                            (Some(a), Some(b)) => a == b,
                            _ => false,
                        };
                        if matches {
                            matched_branch = Some(branch);
                            break;
                        }
                    }
                }
            }

            if let Some(branch) = matched_branch.or(default_branch) {
                self.foreach_seed.borrow_mut().replace(vec![row]);
                let branch_rows = self.exec_operator(cx, branch)?;
                self.foreach_seed.borrow_mut().take();
                out.extend(branch_rows);
            } else {
                // No branch matches and no default → identity (pass through)
                out.push(row);
            }
        }
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
                    out.extend(alt_rows);
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

    /// Execute `GremlinSideEffect` — run `effect` for each input row (discard
    /// its output) and pass the original rows through unchanged.
    pub(crate) fn exec_gremlin_side_effect(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::GremlinSideEffect { effect, child } = op else {
            debug_assert!(false, "expected GremlinSideEffect operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected GremlinSideEffect operator".into(),
            }));
        };

        let input = self.exec_operator(cx, child)?;
        for row in &input {
            cx.budget.check_or_err()?;
            self.foreach_seed.borrow_mut().replace(vec![row.clone()]);
            let _discard = self.exec_operator(cx, effect)?;
            self.foreach_seed.borrow_mut().take();
        }
        Ok(input)
    }

    /// Execute `Cap` — emit accumulated side-effect values for the given keys.
    ///
    /// Executes the child plan (which should contain `GremlinAggregate` operators
    /// that populate `__gremlin_agg_<key>` columns) and then collects those
    /// accumulated values into a single result row.
    pub(crate) fn exec_cap(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Cap { keys, child } = op else {
            debug_assert!(false, "expected Cap operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected Cap operator".into(),
            }));
        };

        let child_rows = self.exec_operator(cx, child)?;

        // Collect aggregated values for each key from the child rows.
        // When aggregate runs in a global scope, every row carries the same
        // accumulated list, so taking the first is correct.  When aggregate
        // runs inside local(), each row carries a *partial* single-item list.
        // We detect the latter case and merge partial lists.
        // For group()/groupCount(), the value is a Map — maps are merged by
        // key with last-writer-wins.
        let mut row = Row::new();
        for key in keys {
            let agg_col = format!("__gremlin_agg_{key}");
            let agg_vals: Vec<&Value> = child_rows
                .iter()
                .filter_map(|r| r.get(&agg_col))
                .collect();
            if agg_vals.is_empty() {
                row.insert(key.clone(), Value::List(Vec::new()));
                continue;
            }
            // If there is only one distinct value (all rows have the same),
            // take it directly.  Otherwise merge partial lists/maps.
            let first = agg_vals[0];
            let all_same = agg_vals.iter().all(|v| *v == first);
            if all_same {
                row.insert(key.clone(), first.clone());
            } else {
                // Merge: for Lists, concatenate; for Maps, union by key.
                match first {
                    Value::List(_) => {
                        let mut merged = Vec::new();
                        for v in &agg_vals {
                            if let Value::List(items) = v {
                                merged.extend(items.iter().cloned());
                            }
                        }
                        row.insert(key.clone(), Value::List(merged));
                    }
                    Value::Map(_) => {
                        let mut merged = IndexMap::new();
                        for v in &agg_vals {
                            if let Value::Map(m) = v {
                                for (mk, mv) in m {
                                    merged.insert(mk.clone(), mv.clone());
                                }
                            }
                        }
                        row.insert(key.clone(), Value::Map(merged));
                    }
                    _ => {
                        row.insert(key.clone(), first.clone());
                    }
                }
            }
        }
        Ok(vec![row])
    }

    /// Execute `GremlinAggregate` — collect traverser values into a
    /// `__gremlin_agg_<key>` column for later retrieval by `Cap`.
    pub(crate) fn exec_gremlin_aggregate(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::GremlinAggregate {
            key,
            child,
            value_alias,
            by_keys,
            productive,
        } = op
        else {
            debug_assert!(false, "expected GremlinAggregate operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected GremlinAggregate operator".into(),
            }));
        };

        let mut input = self.exec_operator(cx, child)?;
        let agg_col = format!("__gremlin_agg_{key}");

        // Collect all values seen across all rows.
        // When by_keys is set, project properties from the traverser instead.
        let mut collected: Vec<Value> = Vec::new();
        for row in &input {
            if by_keys.len() >= 2 {
                // Multiple by() keys — collect a map of {key1: val1, key2: val2, ...}
                let mut map = IndexMap::new();
                for by_prop in by_keys {
                    let prop_expr = Expr::Property(
                        Box::new(Expr::Variable(value_alias.clone())),
                        smol_str::SmolStr::new(by_prop),
                    );
                    let ctx = self.build_filter_ctx(cx.graph, row)?;
                    let val = self
                        .eval_with_storage(cx.graph, &prop_expr, row, &ctx)
                        .unwrap_or(Value::Null);
                    map.insert(by_prop.clone().into_boxed_str(), val);
                }
                collected.push(Value::Map(map));
            } else if let Some(by_prop) = by_keys.first() {
                // Single by("prop") modulation: resolve property from the node/edge value
                let prop_expr = Expr::Property(
                    Box::new(Expr::Variable(value_alias.clone())),
                    smol_str::SmolStr::new(by_prop),
                );
                let ctx = self.build_filter_ctx(cx.graph, row)?;
                let projected = self
                    .eval_with_storage(cx.graph, &prop_expr, row, &ctx)
                    .unwrap_or(Value::Null);
                // Skip null values unless ProductiveByStrategy is active
                if *productive || !matches!(projected, Value::Null) {
                    collected.push(projected);
                }
            } else if let Some(val) = row.get(value_alias.as_str()) {
                collected.push(val.clone());
            }
        }

        // Stamp the accumulated list onto every output row.
        // If a row already carries a value for this agg column (e.g. from a
        // previous local(aggregate("a")) step), extend it rather than overwrite.
        if input.is_empty() {
            // Even with no input, the side-effect must be accessible via cap().
            let mut sentinel = Row::new();
            sentinel.insert(agg_col, Value::List(collected));
            return Ok(vec![sentinel]);
        }
        for row in &mut input {
            if let Some(Value::List(existing)) = row.get(&agg_col).cloned() {
                let mut merged = existing;
                merged.extend(collected.iter().cloned());
                row.insert(agg_col.clone(), Value::List(merged));
            } else {
                row.insert(agg_col.clone(), Value::List(collected.clone()));
            }
        }

        Ok(input)
    }

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
            let path: Vec<Value> = path_aliases
                .iter()
                .filter_map(|alias| row.get(alias.as_str()).cloned())
                .filter(|v| !matches!(v, Value::Null))
                .collect();
            let mut result_row = Row::new();
            result_row.insert("path".to_string(), Value::List(path));
            out.push(result_row);
        }
        Ok(out)
    }

    /// Execute `GremlinMatch` — declarative pattern matching with named bindings.
    ///
    /// For each input row, iterate through patterns. Each pattern is a
    /// sub-traversal that binds variables. We progressively join results
    /// by requiring consistent bindings across patterns.
    pub(crate) fn exec_gremlin_match(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::GremlinMatch {
            patterns,
            label_map,
            result_alias,
            child,
        } = op
        else {
            debug_assert!(false, "expected GremlinMatch operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected GremlinMatch operator".into(),
            }));
        };

        let input = self.exec_operator(cx, child)?;

        if patterns.is_empty() {
            return Ok(input);
        }

        // Build reverse map: internal_alias → label_name.
        let reverse_map: std::collections::HashMap<&str, &str> = label_map
            .iter()
            .map(|(label, alias)| (alias.0.as_str(), label.as_str()))
            .collect();

        // Normalize a row: replace internal aliases with label names.
        // This ensures consistent keys across patterns for merging.
        let normalize = |row: &Row| -> Row {
            let mut out = Row::new();
            for (k, v) in row {
                let key = if let Some(label) = reverse_map.get(k.as_str()) {
                    (*label).to_string()
                } else {
                    k.clone()
                };
                out.insert(key, v.clone());
            }
            out
        };

        // For each input row, execute all patterns and join on shared bindings.
        let mut out = Vec::new();
        for row in &input {
            cx.budget.check_or_err()?;

            // Normalize input row so label names are used as keys.
            let mut current_bindings = vec![normalize(row)];

            for pattern in patterns {
                let mut next_bindings = Vec::new();

                for binding in &current_bindings {
                    cx.budget.check_or_err()?;
                    // Seed: the pattern expects internal alias names, so we
                    // need to provide the binding with both label and internal
                    // alias keys. Build a denormalized seed row.
                    let mut seed = binding.clone();
                    for (label, alias) in label_map {
                        if let Some(v) = binding.get(label.as_str()) {
                            seed.insert(alias.0.to_string(), v.clone());
                        }
                    }

                    self.foreach_seed.borrow_mut().replace(vec![seed]);
                    let pattern_rows = self.exec_operator(cx, pattern)?;
                    self.foreach_seed.borrow_mut().take();

                    // Normalize pattern results, then merge on shared label
                    // keys, checking consistency.
                    for p_row in pattern_rows {
                        let norm = normalize(&p_row);
                        let mut merged = binding.clone();
                        let mut consistent = true;
                        for (k, v) in &norm {
                            if let Some(existing) = merged.get(k) {
                                if existing != v {
                                    consistent = false;
                                    break;
                                }
                            } else {
                                merged.insert(k.clone(), v.clone());
                            }
                        }
                        if consistent {
                            next_bindings.push(merged);
                        }
                    }
                }

                current_bindings = next_bindings;
                if current_bindings.is_empty() {
                    break;
                }
            }

            // Build output rows with label columns and a Map result.
            for binding in current_bindings {
                let mut result_row = Row::new();
                let mut map_entries = indexmap::IndexMap::new();
                for (k, v) in &binding {
                    // Include label columns and also re-insert internal alias
                    // keys so downstream steps (select, dedup) can find them.
                    result_row.insert(k.clone(), v.clone());
                    for (label, alias) in label_map {
                        if label == k {
                            result_row.insert(alias.0.to_string(), v.clone());
                            map_entries.insert(Box::from(label.as_str()), v.clone());
                        }
                    }
                }
                result_row.insert(result_alias.0.to_string(), Value::Map(map_entries));
                out.push(result_row);
            }
        }

        Ok(out)
    }

    /// Execute `GremlinSelect` — project labeled step values, skipping rows
    /// where any referenced label is not bound in the row.
    pub(crate) fn exec_gremlin_select(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::GremlinSelect { keys, child } = op else {
            debug_assert!(false, "expected GremlinSelect operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected GremlinSelect operator".into(),
            }));
        };

        let input = self.exec_operator(cx, child)?;
        let mut out = Vec::new();

        for row in &input {
            cx.budget.check_or_err()?;
            let ctx = self.build_filter_ctx(cx.graph, row)?;

            // Check that ALL referenced variables are present in the row.
            let mut all_found = true;
            let mut new_row = Row::new();

            for (src_var, output_key) in keys {
                let var_name = src_var.0.as_str();

                // Handle property-accessor pattern: __prop_PROPNAME__VARNAME
                if let Some(rest) = var_name.strip_prefix("__prop_") {
                    if let Some(sep_pos) = rest.find("__") {
                        let prop_name = &rest[..sep_pos];
                        let real_var = &rest[sep_pos + 2..];
                        let val = row.get(real_var);
                        match val {
                            Some(node_val) => {
                                // Resolve property from the node/edge
                                let prop_expr = Expr::Property(
                                    Box::new(Expr::Variable(Identifier::new(real_var))),
                                    smol_str::SmolStr::new(prop_name),
                                );
                                match self.eval_with_storage(cx.graph, &prop_expr, row, &ctx) {
                                    Ok(v) => {
                                        new_row.insert(output_key.clone(), v);
                                    }
                                    Err(_) => {
                                        // Property doesn't exist — use Null for maps
                                        if let Value::Map(m) = node_val {
                                            let v =
                                                m.get(prop_name).cloned().unwrap_or(Value::Null);
                                            new_row.insert(output_key.clone(), v);
                                        } else {
                                            new_row.insert(output_key.clone(), Value::Null);
                                        }
                                    }
                                }
                            }
                            None => {
                                all_found = false;
                                break;
                            }
                        }
                        continue;
                    }
                }

                // Try looking up in the row directly.
                let val = if let Some(v) = row.get(var_name) {
                    Some(v.clone())
                } else if let Some(v) =
                    row.get(&format!("__gremlin_agg_{}", output_key))
                {
                    // Fallback: aggregate sideeffect columns are stored
                    // with a __gremlin_agg_ prefix.
                    Some(v.clone())
                } else {
                    // For select() after group(): the row may contain a Map value.
                    // Try looking up output_key as a map key in any Map values in the row.
                    let mut found = None;
                    for (_k, v) in row.iter() {
                        if let Value::Map(m) = v {
                            if let Some(map_val) = m.get(output_key.as_str()) {
                                found = Some(map_val.clone());
                                break;
                            }
                        }
                    }
                    if found.is_none() {
                        // Try evaluating as expression (handles parameters, etc.)
                        let expr = Expr::Variable(src_var.clone());
                        crate::executor::filter::eval_expr(&expr, &ctx).ok()
                    } else {
                        found
                    }
                };

                match val {
                    Some(v) => {
                        new_row.insert(output_key.clone(), v);
                    }
                    None => {
                        all_found = false;
                        break;
                    }
                }
            }

            if all_found {
                // If multiple keys, produce a Map value (Gremlin select semantics)
                if keys.len() > 1 {
                    let mut map: indexmap::IndexMap<Box<str>, Value> = indexmap::IndexMap::new();
                    for (_, output_key) in keys {
                        if let Some(v) = new_row.get(output_key.as_str()) {
                            map.insert(output_key.clone().into(), v.clone());
                        }
                    }
                    let mut map_row = Row::new();
                    inject_parameters(&mut map_row, &self.parameters);
                    // Add individual key columns AND their internal alias
                    // columns so downstream where/filter can reference them.
                    for (src_var, output_key) in keys {
                        if let Some(v) = new_row.get(output_key.as_str()) {
                            map_row.insert(output_key.clone(), v.clone());
                            map_row.insert(src_var.0.to_string(), v.clone());
                        }
                    }
                    let map_alias = keys
                        .iter()
                        .map(|(_, k)| k.as_str())
                        .collect::<Vec<_>>()
                        .join("_");
                    map_row.insert(map_alias, Value::Map(map));
                    cx.budget.record_row();
                    out.push(map_row);
                } else {
                    // Single-key select: carry forward existing row columns
                    // so that as() labels remain accessible to downstream steps
                    // (needed for addE/from/to resolution after select/unfold).
                    let mut out_row = row.clone();
                    for (_, output_key) in keys {
                        if let Some(v) = new_row.get(output_key.as_str()) {
                            out_row.insert(output_key.clone(), v.clone());
                        }
                    }
                    inject_parameters(&mut out_row, &self.parameters);
                    cx.budget.record_row();
                    out.push(out_row);
                }
            }
        }

        Ok(out)
    }

    /// Execute `GremlinGroup` — group all input traversers into a single Map.
    ///
    /// For each input row, evaluates `key_expr` and `value_expr`. Groups
    /// values by key and applies the aggregation function. Returns a single
    /// row containing one Map value.
    pub(crate) fn exec_gremlin_group(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        use crate::planner::AggFunction;
        use indexmap::IndexMap;

        let Operator::GremlinGroup {
            key_expr,
            value_expr,
            agg_fn,
            alias,
            side_effect_key,
            child,
        } = op
        else {
            debug_assert!(false, "expected GremlinGroup operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected GremlinGroup operator".into(),
            }));
        };

        let input = self.exec_operator(cx, child)?;

        // Collect (key_str, value) pairs from input rows.
        // Use Box<str> keys since Value doesn't implement Hash.
        let mut groups: IndexMap<Box<str>, Vec<Value>> = IndexMap::new();

        // Check if value_expr differs from key_expr (i.e., a value-by modulation is present).
        let has_value_by = value_expr != key_expr;

        for row in &input {
            cx.budget.check_or_err()?;
            let ctx = self.build_filter_ctx(cx.graph, row)?;
            let key_val = self
                .eval_with_storage(cx.graph, key_expr, row, &ctx)
                .unwrap_or(Value::Null);
            // Skip rows where the group key evaluates to Null (unproductive by).
            // TinkerPop default: null keys are dropped unless ProductiveByStrategy.
            if matches!(key_val, Value::Null) {
                continue;
            }
            let val = self
                .eval_with_storage(cx.graph, value_expr, row, &ctx)
                .unwrap_or(Value::Null);
            let key_str = value_to_map_key(&key_val);
            let entry = groups.entry(key_str).or_default();
            // When a value-by projection is active, skip null values (unproductive by).
            if has_value_by && matches!(val, Value::Null) {
                // Key still gets created (via or_default) with empty vec
            } else {
                entry.push(val);
            }
        }

        // Apply aggregation function to each group.
        let mut result_map: IndexMap<Box<str>, Value> = IndexMap::new();
        for (key_str, values) in groups {
            let agg_result = match agg_fn {
                AggFunction::Count => Value::Int(values.len() as i64),
                AggFunction::Collect => Value::List(values),
                AggFunction::Sum => {
                    let mut total = 0i64;
                    let mut float_total = 0.0f64;
                    let mut use_float = false;
                    for v in &values {
                        match v {
                            Value::Int(i) => total += i,
                            Value::Float(f) => {
                                use_float = true;
                                float_total += f;
                            }
                            _ => {}
                        }
                    }
                    if use_float {
                        Value::Float(float_total + total as f64)
                    } else {
                        Value::Int(total)
                    }
                }
                _ => Value::List(values),
            };
            result_map.insert(key_str, agg_result);
        }

        // Named side-effect: store the Map on every input row and pass
        // traversers through instead of collapsing into a single Map row.
        if let Some(key) = side_effect_key {
            let agg_col = format!("__gremlin_agg_{key}");
            let map_val = Value::Map(result_map);
            let mut output = input;
            if output.is_empty() {
                // Even with no input, the side-effect must be accessible via cap().
                let mut sentinel = Row::new();
                sentinel.insert(agg_col, map_val);
                return Ok(vec![sentinel]);
            }
            for row in &mut output {
                row.insert(agg_col.clone(), map_val.clone());
            }
            return Ok(output);
        }

        let mut out_row = Row::new();
        out_row.insert(alias.0.clone(), Value::Map(result_map));
        cx.budget.record_row();
        Ok(vec![out_row])
    }

    /// Execute `GremlinMapSlice` — slice Map/List values in each row.
    pub(crate) fn exec_gremlin_map_slice(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        use indexmap::IndexMap;

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
            for alias in path_aliases {
                if let Some(val) = row.get(alias.as_str()) {
                    if matches!(val, Value::Null) {
                        continue;
                    }
                    // Use a discriminator key to detect duplicate path entries
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
            }
            if is_simple {
                out.push(row);
            }
        }
        Ok(out)
    }

    /// Execute `GremlinIndex` — pair each element with its positional index.
    pub(crate) fn exec_gremlin_index(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::GremlinIndex {
            child,
            alias,
            out_alias,
            with_map,
        } = op
        else {
            debug_assert!(false, "expected GremlinIndex operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected GremlinIndex operator".into(),
            }));
        };

        let input = self.exec_operator(cx, child)?;
        let mut out = Vec::with_capacity(input.len());

        // Check if input is a single row containing a List — index its elements
        if input.len() == 1 {
            if let Some(Value::List(items)) = input[0].get(alias.as_str()) {
                if *with_map {
                    // Produce a single Map {0: elem0, 1: elem1, ...}
                    let mut map = IndexMap::new();
                    for (i, item) in items.iter().enumerate() {
                        map.insert(format!("{i}").into_boxed_str(), item.clone());
                    }
                    let mut row = Row::new();
                    row.insert(out_alias.0.clone(), Value::Map(map));
                    out.push(row);
                } else {
                    // Produce [element, index] pairs
                    for (i, item) in items.iter().enumerate() {
                        let pair = Value::List(vec![item.clone(), Value::Int(i as i64)]);
                        let mut row = Row::new();
                        row.insert(out_alias.0.clone(), pair);
                        out.push(row);
                    }
                }
                return Ok(out);
            }
        }

        // Stream mode: index rows positionally
        if *with_map {
            let mut map = IndexMap::new();
            for (i, row) in input.iter().enumerate() {
                if let Some(val) = row.get(alias.as_str()) {
                    map.insert(format!("{i}").into_boxed_str(), val.clone());
                }
            }
            let mut row = Row::new();
            row.insert(out_alias.0.clone(), Value::Map(map));
            out.push(row);
        } else {
            for (i, row) in input.iter().enumerate() {
                cx.budget.check_or_err()?;
                let val = row.get(alias.as_str()).cloned().unwrap_or(Value::Null);
                let pair = Value::List(vec![val, Value::Int(i as i64)]);
                let mut new_row = Row::new();
                new_row.insert(out_alias.0.clone(), pair);
                out.push(new_row);
            }
        }
        Ok(out)
    }

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
            for (key, sub_op, out_alias) in entries {
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
                map.insert(key.clone().into_boxed_str(), val);
            }
            // Store the projected Map as both individual columns and a combined Map
            let map_val = Value::Map(map.clone());
            let mut out_row = Row::new();
            for (k, v) in &map {
                out_row.insert(k.to_string(), v.clone());
            }
            // Also set the result on the current_alias so downstream steps find it
            out_row.insert(current_alias.0.clone(), map_val);
            out.push(out_row);
        }
        Ok(out)
    }
}

/// Convert a Value to a string key for use in Gremlin group() Map output.
fn value_to_map_key(v: &Value) -> Box<str> {
    match v {
        Value::Str(s) => s.clone(),
        Value::Int(i) => format!("{i}").into_boxed_str(),
        Value::Float(f) => format!("{f}").into_boxed_str(),
        Value::Bool(b) => format!("{b}").into_boxed_str(),
        Value::Null => "null".to_string().into_boxed_str(),
        Value::Node(nid) => format!("v[{}]", nid.0).into_boxed_str(),
        Value::Edge(eid) => format!("e[{}]", eid.0).into_boxed_str(),
        _ => format!("{v:?}").into_boxed_str(),
    }
}

/// Evaluate a Gremlin predicate string (e.g., "P.between(26,30)") against a value.
fn eval_gremlin_predicate(pred_str: &str, value: &Value) -> bool {
    fn parse_num(s: &str) -> Option<f64> {
        let s = s.trim();
        if let Ok(i) = s.parse::<i64>() {
            return Some(i as f64);
        }
        s.parse::<f64>().ok()
    }
    fn value_as_f64(v: &Value) -> Option<f64> {
        match v {
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }

    let pred = pred_str.strip_prefix("P.").unwrap_or(pred_str);

    if let Some(rest) = pred.strip_prefix("between(") {
        if let Some(args) = rest.strip_suffix(')') {
            let parts: Vec<&str> = args.splitn(2, ',').collect();
            if parts.len() == 2 {
                if let (Some(lo), Some(hi), Some(v)) = (
                    parse_num(parts[0]),
                    parse_num(parts[1]),
                    value_as_f64(value),
                ) {
                    return v >= lo && v < hi;
                }
            }
        }
        return false;
    }

    // Single-arg predicates: P.lt(30), P.gt(30), P.lte(30), P.gte(30), P.eq(x), P.neq(x)
    for (prefix, cmp_fn) in &[
        ("lt(", (|v: f64, t: f64| v < t) as fn(f64, f64) -> bool),
        ("lte(", |v, t| v <= t),
        ("gt(", |v, t| v > t),
        ("gte(", |v, t| v >= t),
        ("eq(", |v, t| (v - t).abs() < 1e-9),
        ("neq(", |v, t| (v - t).abs() >= 1e-9),
    ] {
        if let Some(rest) = pred.strip_prefix(prefix) {
            if let Some(arg_str) = rest.strip_suffix(')') {
                // Try numeric comparison first
                if let (Some(threshold), Some(v)) = (parse_num(arg_str), value_as_f64(value)) {
                    return cmp_fn(v, threshold);
                }
                // For eq/neq, also try string comparison
                if *prefix == "eq(" {
                    let arg = arg_str.trim().trim_matches('"');
                    return match value {
                        Value::Str(s) => s.as_ref() == arg,
                        _ => false,
                    };
                }
                if *prefix == "neq(" {
                    let arg = arg_str.trim().trim_matches('"');
                    return match value {
                        Value::Str(s) => s.as_ref() != arg,
                        _ => true,
                    };
                }
            }
            return false;
        }
    }

    // P.neq(y) with string comparison
    if let Some(rest) = pred.strip_prefix("neq(") {
        if let Some(arg_str) = rest.strip_suffix(')') {
            let arg = arg_str.trim().trim_matches('"');
            match value {
                Value::Str(s) => return s.as_ref() != arg,
                _ => return true,
            }
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::ast::{Direction, Expr, Identifier, Label, RelType};
    use crate::executor::budget::ExecutionBudget;
    use crate::executor::engine::QueryExecutor;
    use crate::planner::Operator;
    use crate::storage::{GraphStore, InMemoryGraphStore};
    use crate::types::Value;

    fn make_store() -> InMemoryGraphStore {
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();
        store
    }

    /// Build a linear chain A->B->C and return the store.
    fn make_chain() -> InMemoryGraphStore {
        let store = make_store();
        let mut pa = HashMap::new();
        pa.insert("name".to_string(), Value::Str("A".into()));
        let a = store.create_node("g", vec!["N".to_string()], pa).unwrap();
        let mut pb = HashMap::new();
        pb.insert("name".to_string(), Value::Str("B".into()));
        let b = store.create_node("g", vec!["N".to_string()], pb).unwrap();
        let mut pc = HashMap::new();
        pc.insert("name".to_string(), Value::Str("C".into()));
        let _c = store.create_node("g", vec!["N".to_string()], pc).unwrap();
        store
            .create_edge("g", a, b, "NEXT".to_string(), HashMap::new())
            .unwrap();
        store
            .create_edge("g", b, _c, "NEXT".to_string(), HashMap::new())
            .unwrap();
        store
    }

    // ── Repeat tests ─────────────────────────────────────────────────────────

    #[test]
    fn repeat_fixed_times_traverses_levels() {
        let store = make_chain();
        let executor = QueryExecutor::new(&store);

        // Scan all :N nodes, then repeat out-expand once.
        // With times=1, each starting node expands one hop outward.
        // Use a remap so the body output "m" gets renamed back to "n" for iteration.
        let plan = Operator::Repeat {
            body: Box::new(Operator::Expand {
                direction: Direction::Outgoing,
                rel_types: vec![RelType::new("NEXT")],
                src_alias: Identifier::new("n"),
                rel_alias: None,
                dst_alias: Identifier::new("m"),
                child: Box::new(Operator::SingleRow),
            }),
            times: Some(1),
            until: None,
            emit: false,
            emit_before: false,
            emit_predicate: None,
            remap: Some(("m".to_string(), "n".to_string())),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("n"),
            }),
        };

        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        // A→B and B→C expand; C has no outgoing NEXT so it is dropped.
        assert_eq!(result.rows.len(), 2);
    }

    #[test]
    fn repeat_with_emit_includes_intermediates() {
        let store = make_chain();
        let executor = QueryExecutor::new(&store);

        let plan = Operator::Repeat {
            body: Box::new(Operator::Expand {
                direction: Direction::Outgoing,
                rel_types: vec![RelType::new("NEXT")],
                src_alias: Identifier::new("n"),
                rel_alias: None,
                dst_alias: Identifier::new("n"),
                child: Box::new(Operator::SingleRow),
            }),
            times: Some(2),
            until: None,
            emit: true,
            emit_before: true,
            emit_predicate: None,
            remap: None,
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("n"),
            }),
        };

        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        // emit=true collects intermediates: iter0 emits 3 (scan), iter1 emits 2 (expand),
        // plus final 1 (A->B->C). Total should be > 2.
        assert!(
            result.rows.len() > 2,
            "emit=true should produce more rows than without emit, got {}",
            result.rows.len()
        );
    }

    // ── Choose tests ─────────────────────────────────────────────────────────

    #[test]
    fn choose_routes_based_on_predicate() {
        let store = make_store();
        // Create two nodes: one labeled "Yes", one labeled "No".
        let mut py = HashMap::new();
        py.insert("tag".to_string(), Value::Str("yes".into()));
        store.create_node("g", vec!["Yes".to_string()], py).unwrap();
        let mut pn = HashMap::new();
        pn.insert("tag".to_string(), Value::Str("no".into()));
        store.create_node("g", vec!["No".to_string()], pn).unwrap();

        let executor = QueryExecutor::new(&store);

        // Predicate: scan for :Yes label (rows with a Yes node produce results).
        // if_true: project tag as "matched"
        // if_false: project tag as "unmatched"
        let plan = Operator::Choose {
            predicate: Box::new(Operator::NodeScan {
                label: Some(Label::new("Yes")),
                alias: Identifier::new("check"),
            }),
            if_true: Box::new(Operator::Project {
                items: vec![(
                    Expr::Str("routed_true".into()),
                    Some(Identifier::new("result")),
                )],
                child: Box::new(Operator::SingleRow),
            }),
            if_false: Some(Box::new(Operator::Project {
                items: vec![(
                    Expr::Str("routed_false".into()),
                    Some(Identifier::new("result")),
                )],
                child: Box::new(Operator::SingleRow),
            })),
            child: Box::new(Operator::NodeScan {
                label: None,
                alias: Identifier::new("n"),
            }),
            true_alias: None,
            false_alias: None,
            output_alias: None,
        };

        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        // 2 input nodes -> 2 output rows
        assert_eq!(result.rows.len(), 2);
        let results: Vec<_> = result.rows.iter().filter_map(|r| r.get("result")).collect();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn choose_without_else_passes_through() {
        let store = make_store();
        store
            .create_node("g", vec!["A".to_string()], HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);

        // Predicate that never matches (scan for label "NOPE").
        // No if_false -> row passes through unchanged.
        let plan = Operator::Choose {
            predicate: Box::new(Operator::NodeScan {
                label: Some(Label::new("NOPE")),
                alias: Identifier::new("check"),
            }),
            if_true: Box::new(Operator::Project {
                items: vec![(
                    Expr::Str("should_not_appear".into()),
                    Some(Identifier::new("x")),
                )],
                child: Box::new(Operator::SingleRow),
            }),
            if_false: None,
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("A")),
                alias: Identifier::new("n"),
            }),
            true_alias: None,
            false_alias: None,
            output_alias: None,
        };

        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
        // Should have "n" (the original scan) but not "x".
        assert!(result.rows[0].contains_key("n"));
        assert!(!result.rows[0].contains_key("x"));
    }

    // ── Coalesce tests ───────────────────────────────────────────────────────

    #[test]
    fn coalesce_returns_first_nonempty() {
        let store = make_store();
        store
            .create_node("g", vec!["A".to_string()], HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);

        // First alternative: scan for "NOPE" (empty).
        // Second alternative: project a literal (non-empty).
        let plan = Operator::Coalesce {
            alternatives: vec![
                Operator::NodeScan {
                    label: Some(Label::new("NOPE")),
                    alias: Identifier::new("miss"),
                },
                Operator::Project {
                    items: vec![(Expr::Str("fallback".into()), Some(Identifier::new("val")))],
                    child: Box::new(Operator::SingleRow),
                },
            ],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("A")),
                alias: Identifier::new("n"),
            }),
        };

        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(
            result.rows[0].get("val"),
            Some(&Value::Str("fallback".into()))
        );
    }

    #[test]
    fn coalesce_uses_first_match_when_multiple_succeed() {
        let store = make_store();
        store
            .create_node("g", vec!["A".to_string()], HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);

        let plan = Operator::Coalesce {
            alternatives: vec![
                Operator::Project {
                    items: vec![(Expr::Str("first".into()), Some(Identifier::new("val")))],
                    child: Box::new(Operator::SingleRow),
                },
                Operator::Project {
                    items: vec![(Expr::Str("second".into()), Some(Identifier::new("val")))],
                    child: Box::new(Operator::SingleRow),
                },
            ],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("A")),
                alias: Identifier::new("n"),
            }),
        };

        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].get("val"), Some(&Value::Str("first".into())));
    }

    // ── GremlinSideEffect tests ──────────────────────────────────────────────

    #[test]
    fn side_effect_passes_input_through() {
        let store = make_store();
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::Str("root".into()));
        store
            .create_node("g", vec!["A".to_string()], props)
            .unwrap();

        let executor = QueryExecutor::new(&store);

        // Side effect: project something (result is discarded).
        // Input rows should pass through unchanged.
        let plan = Operator::GremlinSideEffect {
            effect: Box::new(Operator::Project {
                items: vec![(
                    Expr::Str("ignored".into()),
                    Some(Identifier::new("discard")),
                )],
                child: Box::new(Operator::SingleRow),
            }),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("A")),
                alias: Identifier::new("n"),
            }),
        };

        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
        assert!(result.rows[0].contains_key("n"));
        assert!(!result.rows[0].contains_key("discard"));
    }

    // ── Cap tests ────────────────────────────────────────────────────────────

    #[test]
    fn cap_returns_empty_lists_for_keys_without_aggregate() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);

        let plan = Operator::Cap {
            keys: vec!["x".to_string(), "y".to_string()],
            child: Box::new(Operator::SingleRow),
        };

        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].get("x"), Some(&Value::List(vec![])));
        assert_eq!(result.rows[0].get("y"), Some(&Value::List(vec![])));
    }

    // ── Aggregate + Cap tests ────────────────────────────────────────────────

    #[test]
    fn aggregate_cap_collects_all_nodes() {
        let store = make_chain(); // A->B->C
        let executor = QueryExecutor::new(&store);

        let plan = Operator::Cap {
            keys: vec!["x".to_string()],
            child: Box::new(Operator::GremlinAggregate {
                key: "x".to_string(),
                child: Box::new(Operator::NodeScan {
                    label: Some(Label::new("N")),
                    alias: Identifier::new("n"),
                }),
                value_alias: Identifier::new("n"),
                by_keys: vec![],
                productive: false,
            }),
        };

        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
        if let Some(Value::List(items)) = result.rows[0].get("x") {
            assert_eq!(items.len(), 3, "should collect all 3 nodes");
        } else {
            panic!("expected list for cap key 'x'");
        }
    }

    #[test]
    fn aggregate_cap_collects_outgoing_neighbors() {
        let store = make_chain(); // A->B->C
        let executor = QueryExecutor::new(&store);

        let plan = Operator::Cap {
            keys: vec!["friends".to_string()],
            child: Box::new(Operator::GremlinAggregate {
                key: "friends".to_string(),
                child: Box::new(Operator::Expand {
                    direction: Direction::Outgoing,
                    rel_types: vec![RelType::new("NEXT")],
                    src_alias: Identifier::new("n"),
                    rel_alias: None,
                    dst_alias: Identifier::new("m"),
                    child: Box::new(Operator::NodeScan {
                        label: Some(Label::new("N")),
                        alias: Identifier::new("n"),
                    }),
                }),
                value_alias: Identifier::new("m"),
                by_keys: vec![],
                productive: false,
            }),
        };

        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
        if let Some(Value::List(items)) = result.rows[0].get("friends") {
            assert_eq!(items.len(), 2, "should collect 2 outgoing neighbors");
        } else {
            panic!("expected list for cap key 'friends'");
        }
    }

    #[test]
    fn aggregate_passes_rows_through() {
        let store = make_chain();
        let executor = QueryExecutor::new(&store);

        let plan = Operator::GremlinAggregate {
            key: "x".to_string(),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("n"),
            }),
            value_alias: Identifier::new("n"),
            by_keys: vec![],
                productive: false,
        };

        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(
            result.rows.len(),
            3,
            "aggregate should pass all 3 rows through"
        );
        for row in &result.rows {
            assert!(row.contains_key("__gremlin_agg_x"));
        }
    }

    #[test]
    fn cap_with_aggregate_multiple_keys() {
        let store = make_chain();
        let executor = QueryExecutor::new(&store);

        let plan = Operator::Cap {
            keys: vec!["x".to_string(), "y".to_string()],
            child: Box::new(Operator::GremlinAggregate {
                key: "x".to_string(),
                child: Box::new(Operator::NodeScan {
                    label: Some(Label::new("N")),
                    alias: Identifier::new("n"),
                }),
                value_alias: Identifier::new("n"),
                by_keys: vec![],
                productive: false,
            }),
        };

        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
        if let Some(Value::List(items)) = result.rows[0].get("x") {
            assert_eq!(items.len(), 3);
        } else {
            panic!("expected list for 'x'");
        }
        assert!(result.rows[0].contains_key("y"));
    }

    #[test]
    fn aggregate_cap_single_node() {
        let store = make_store();
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::Str("solo".into()));
        store
            .create_node("g", vec!["X".to_string()], props)
            .unwrap();

        let executor = QueryExecutor::new(&store);

        let plan = Operator::Cap {
            keys: vec!["all".to_string()],
            child: Box::new(Operator::GremlinAggregate {
                key: "all".to_string(),
                child: Box::new(Operator::NodeScan {
                    label: Some(Label::new("X")),
                    alias: Identifier::new("n"),
                }),
                value_alias: Identifier::new("n"),
                by_keys: vec![],
                productive: false,
            }),
        };

        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
        if let Some(Value::List(items)) = result.rows[0].get("all") {
            assert_eq!(items.len(), 1);
        } else {
            panic!("expected single-element list");
        }
    }

    // ── GremlinPath tests ────────────────────────────────────────────────────

    #[test]
    fn path_returns_traversal_elements() {
        let store = make_chain();
        let executor = QueryExecutor::new(&store);

        let plan = Operator::GremlinPath {
            child: Box::new(Operator::Expand {
                direction: Direction::Outgoing,
                rel_types: vec![RelType::new("NEXT")],
                src_alias: Identifier::new("n"),
                rel_alias: None,
                dst_alias: Identifier::new("m"),
                child: Box::new(Operator::NodeScan {
                    label: Some(Label::new("N")),
                    alias: Identifier::new("n"),
                }),
            }),
            path_aliases: vec![Identifier::new("n"), Identifier::new("m")],
        };

        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 2);
        for row in &result.rows {
            if let Some(Value::List(path)) = row.get("path") {
                assert_eq!(path.len(), 2, "each path should have 2 elements");
            } else {
                panic!("expected path column with list value");
            }
        }
    }

    #[test]
    fn path_three_hops() {
        let store = make_chain();
        let executor = QueryExecutor::new(&store);

        let plan = Operator::GremlinPath {
            child: Box::new(Operator::Expand {
                direction: Direction::Outgoing,
                rel_types: vec![RelType::new("NEXT")],
                src_alias: Identifier::new("m"),
                rel_alias: None,
                dst_alias: Identifier::new("o"),
                child: Box::new(Operator::Expand {
                    direction: Direction::Outgoing,
                    rel_types: vec![RelType::new("NEXT")],
                    src_alias: Identifier::new("n"),
                    rel_alias: None,
                    dst_alias: Identifier::new("m"),
                    child: Box::new(Operator::NodeScan {
                        label: Some(Label::new("N")),
                        alias: Identifier::new("n"),
                    }),
                }),
            }),
            path_aliases: vec![
                Identifier::new("n"),
                Identifier::new("m"),
                Identifier::new("o"),
            ],
        };

        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
        if let Some(Value::List(path)) = result.rows[0].get("path") {
            assert_eq!(path.len(), 3, "3-hop path should have 3 elements");
        } else {
            panic!("expected path column with list value");
        }
    }

    #[test]
    fn path_with_no_aliases_returns_empty_paths() {
        let store = make_store();
        store
            .create_node("g", vec!["A".to_string()], HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);

        let plan = Operator::GremlinPath {
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("A")),
                alias: Identifier::new("n"),
            }),
            path_aliases: vec![],
        };

        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].get("path"), Some(&Value::List(vec![])));
    }

    #[test]
    fn path_single_step() {
        let store = make_store();
        store
            .create_node("g", vec!["A".to_string()], HashMap::new())
            .unwrap();
        store
            .create_node("g", vec!["A".to_string()], HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);

        let plan = Operator::GremlinPath {
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("A")),
                alias: Identifier::new("n"),
            }),
            path_aliases: vec![Identifier::new("n")],
        };

        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 2);
        for row in &result.rows {
            if let Some(Value::List(path)) = row.get("path") {
                assert_eq!(path.len(), 1);
            } else {
                panic!("expected single-element path");
            }
        }
    }

    #[test]
    fn path_with_filtered_source() {
        let store = make_chain(); // A->B->C
        let executor = QueryExecutor::new(&store);

        // V(:N).has("name","A").out("NEXT").path()
        let plan = Operator::GremlinPath {
            child: Box::new(Operator::Expand {
                direction: Direction::Outgoing,
                rel_types: vec![RelType::new("NEXT")],
                src_alias: Identifier::new("n"),
                rel_alias: None,
                dst_alias: Identifier::new("m"),
                child: Box::new(Operator::Filter {
                    predicate: Expr::BinaryOp {
                        op: crate::ast::BinaryOp::Eq,
                        left: Box::new(Expr::Property(
                            Box::new(Expr::Variable(Identifier::new("n"))),
                            "name".into(),
                        )),
                        right: Box::new(Expr::Str("A".into())),
                    },
                    child: Box::new(Operator::NodeScan {
                        label: Some(Label::new("N")),
                        alias: Identifier::new("n"),
                    }),
                }),
            }),
            path_aliases: vec![Identifier::new("n"), Identifier::new("m")],
        };

        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1, "only A has an outgoing NEXT");
        if let Some(Value::List(path)) = result.rows[0].get("path") {
            assert_eq!(path.len(), 2);
        } else {
            panic!("expected path");
        }
    }

    // ── GremlinMatch tests ───────────────────────────────────────────────────

    #[test]
    fn match_single_pattern() {
        let store = make_chain();
        let executor = QueryExecutor::new(&store);

        let plan = Operator::GremlinMatch {
            label_map: vec![],
            result_alias: Identifier::new("__match_result"),
            patterns: vec![Operator::Expand {
                direction: Direction::Outgoing,
                rel_types: vec![RelType::new("NEXT")],
                src_alias: Identifier::new("n"),
                rel_alias: None,
                dst_alias: Identifier::new("m"),
                child: Box::new(Operator::SingleRow),
            }],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("n"),
            }),
        };

        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 2);
        for row in &result.rows {
            assert!(row.contains_key("n"));
            assert!(row.contains_key("m"));
        }
    }

    #[test]
    fn match_empty_patterns_passes_through() {
        let store = make_chain();
        let executor = QueryExecutor::new(&store);

        let plan = Operator::GremlinMatch {
            label_map: vec![],
            result_alias: Identifier::new("__match_result"),
            patterns: vec![],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("n"),
            }),
        };

        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 3);
    }

    #[test]
    fn match_two_patterns_join() {
        let store = make_chain(); // A->B->C
        let executor = QueryExecutor::new(&store);

        let plan = Operator::GremlinMatch {
            label_map: vec![],
            result_alias: Identifier::new("__match_result"),
            patterns: vec![
                Operator::Expand {
                    direction: Direction::Outgoing,
                    rel_types: vec![RelType::new("NEXT")],
                    src_alias: Identifier::new("a"),
                    rel_alias: None,
                    dst_alias: Identifier::new("b"),
                    child: Box::new(Operator::SingleRow),
                },
                Operator::Expand {
                    direction: Direction::Outgoing,
                    rel_types: vec![RelType::new("NEXT")],
                    src_alias: Identifier::new("b"),
                    rel_alias: None,
                    dst_alias: Identifier::new("c"),
                    child: Box::new(Operator::SingleRow),
                },
            ],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("a"),
            }),
        };

        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
        assert!(result.rows[0].contains_key("a"));
        assert!(result.rows[0].contains_key("b"));
        assert!(result.rows[0].contains_key("c"));
    }

    #[test]
    fn match_contradicting_patterns_empty() {
        let store = make_chain(); // A->B->C (linear)
        let executor = QueryExecutor::new(&store);

        // Match: a->b via NEXT, and a->b via NONEXISTENT (no such edges)
        let plan = Operator::GremlinMatch {
            label_map: vec![],
            result_alias: Identifier::new("__match_result"),
            patterns: vec![
                Operator::Expand {
                    direction: Direction::Outgoing,
                    rel_types: vec![RelType::new("NEXT")],
                    src_alias: Identifier::new("a"),
                    rel_alias: None,
                    dst_alias: Identifier::new("b"),
                    child: Box::new(Operator::SingleRow),
                },
                Operator::Expand {
                    direction: Direction::Outgoing,
                    rel_types: vec![RelType::new("NONEXISTENT")],
                    src_alias: Identifier::new("a"),
                    rel_alias: None,
                    dst_alias: Identifier::new("d"),
                    child: Box::new(Operator::SingleRow),
                },
            ],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("a"),
            }),
        };

        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(
            result.rows.len(),
            0,
            "no node has both NEXT and NONEXISTENT edges"
        );
    }

    #[test]
    fn match_no_input_rows() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);

        let plan = Operator::GremlinMatch {
            label_map: vec![],
            result_alias: Identifier::new("__match_result"),
            patterns: vec![Operator::Expand {
                direction: Direction::Outgoing,
                rel_types: vec![RelType::new("NEXT")],
                src_alias: Identifier::new("n"),
                rel_alias: None,
                dst_alias: Identifier::new("m"),
                child: Box::new(Operator::SingleRow),
            }],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("NOPE")),
                alias: Identifier::new("n"),
            }),
        };

        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 0);
    }
}
