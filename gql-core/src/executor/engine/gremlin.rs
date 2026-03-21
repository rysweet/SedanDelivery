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
            times_before,
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
        let mut body_exhausted = false;

        // Determine the seed alias name from remap (used for path tracking).
        let seed_key = remap.as_ref().map(|(_, to)| to.as_str());

        // When the repeat body contains a Distinct operator, enable shared
        // dedup state so that values seen in iteration N are remembered in
        // iteration N+1 (cross-iteration dedup semantics).
        let body_has_distinct = operator_contains_distinct(body);
        let prev_dedup_state = if body_has_distinct {
            let prev = self.repeat_dedup_state.borrow_mut().take();
            *self.repeat_dedup_state.borrow_mut() = Some(std::collections::HashSet::new());
            prev
        } else {
            None
        };

        // Do-while semantics: when times() appears AFTER repeat(), always
        // execute the body at least once, even for times(0).
        let max_iterations = match *times {
            Some(0) if !*times_before => 1,
            Some(n) => n,
            None => 256,
        };

        for iteration in 0..max_iterations {
            cx.budget.check_or_err()?;

            // Inject loop counter into current rows so emit predicates and
            // until conditions that reference loops() can see the value.
            for row in &mut current {
                row.insert(
                    "__gremlin_loop_count".to_string(),
                    Value::Int(iteration as i64),
                );
            }

            // Pre-emit: collect traversers before body execution.
            // Only when emit_before is true (emit() appeared before repeat()).
            if *emit && *emit_before {
                self.collect_emitted(cx, &current, emit_predicate, &mut collected)?;
            }

            // Batch mode: prepare all seed rows, then execute body once.
            // This gives correct Gremlin semantics for limit/sample/dedup
            // inside the repeat body (they apply per-iteration, not per-traverser).
            let mut seeds: Vec<Row> = Vec::with_capacity(current.len());
            for row in &current {
                let mut seed_row = row.clone();
                // Inject loop iteration counter so loops() can read it.
                seed_row.insert(
                    "__gremlin_loop_count".to_string(),
                    Value::Int(iteration as i64),
                );
                // Accumulate path history: push the current seed value
                // before the body modifies it.
                if let Some(key) = seed_key {
                    if let Some(seed_val) = row.get(key).cloned() {
                        let mut acc = match row.get("__gremlin_path_acc") {
                            Some(Value::List(l)) => l.clone(),
                            _ => vec![],
                        };
                        acc.push(seed_val);
                        seed_row.insert("__gremlin_path_acc".to_string(), Value::List(acc));
                    }
                }
                if let Some((from, _)) = remap {
                    seed_row.remove(from.as_str());
                }
                seeds.push(seed_row);
            }

            self.foreach_seed.borrow_mut().replace(seeds);
            let body_rows = self.exec_operator(cx, body)?;
            self.foreach_seed.borrow_mut().take();

            // Process body results: separate sentinel rows (side-effects)
            // from regular traverser rows.
            let mut next: Vec<Row> = Vec::new();
            let mut iteration_agg_maps: std::collections::HashMap<
                String,
                indexmap::IndexMap<Box<str>, Value>,
            > = std::collections::HashMap::new();
            for body_row in body_rows {
                if let Some(key) = seed_key {
                    if !body_row.contains_key(key)
                        && remap
                            .as_ref()
                            .is_none_or(|(from, _)| !body_row.contains_key(from.as_str()))
                    {
                        // Sentinel row without traverser alias — merge
                        // side-effect agg columns.
                        for (k, v) in &body_row {
                            if k.starts_with("__gremlin_agg_") {
                                if let Value::Map(m) = v {
                                    let acc = iteration_agg_maps.entry(k.clone()).or_default();
                                    for (mk, mv) in m {
                                        let e = acc.entry(mk.clone()).or_insert(Value::Int(0));
                                        match (e.clone(), mv) {
                                            (Value::Int(a), Value::Int(b)) => {
                                                *e = Value::Int(a + b)
                                            }
                                            (Value::Float(a), Value::Float(b)) => {
                                                *e = Value::Float(a + b)
                                            }
                                            (_, new_v) => *e = new_v.clone(),
                                        }
                                    }
                                }
                                for nr in &mut next {
                                    nr.entry(k.clone()).or_insert_with(|| v.clone());
                                }
                            }
                        }
                        continue;
                    }
                }
                next.push(body_row);
            }

            // Collect agg maps from the single batch body-run.
            if let Some(first) = next.first() {
                for (k, v) in first {
                    if k.starts_with("__gremlin_agg_") {
                        if let Value::Map(m) = v {
                            let acc = iteration_agg_maps.entry(k.clone()).or_default();
                            for (mk, mv) in m {
                                let e = acc.entry(mk.clone()).or_insert(Value::Int(0));
                                match (e.clone(), mv) {
                                    (Value::Int(a), Value::Int(b)) => *e = Value::Int(a + b),
                                    (Value::Float(a), Value::Float(b)) => *e = Value::Float(a + b),
                                    (_, new_v) => *e = new_v.clone(),
                                }
                            }
                        }
                    }
                }
            }

            // Stamp the merged side-effect maps on all rows so every
            // traverser carries the full accumulated state.
            if !iteration_agg_maps.is_empty() {
                for row in &mut next {
                    for (k, merged) in &iteration_agg_maps {
                        row.insert(k.clone(), Value::Map(merged.clone()));
                    }
                }
            }

            if next.is_empty() {
                body_exhausted = true;
                // Body produced no results — stop iterating.
                // When emit is false, remaining traversers are never output:
                // - No until: they simply exhausted the body.
                // - With until: they never satisfied the exit condition.
                if !*emit {
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

            // Update loop count: after body execution in iteration N, the
            // traverser has completed N+1 loops. Gremlin's loops() function
            // returns the number of completed body executions.
            for row in &mut current {
                row.insert(
                    "__gremlin_loop_count".to_string(),
                    Value::Int((iteration + 1) as i64),
                );
            }

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

        // Add final traversers when appropriate:
        // - Non-emit mode with times (no until): add final current (loop count satisfied)
        // - Non-emit mode with until: don't add (traversers never matched exit condition)
        // - Pre-emit with times (body not exhausted): add final current
        // - Post-emit or body exhausted: don't add (already collected in loop)
        let add_final = if *emit {
            *emit_before && !body_exhausted
        } else {
            // With an until condition, only add remaining if times is also set
            // (traversers completed the required iterations even without matching until).
            until.is_none() || times.is_some()
        };
        if add_final {
            collected.extend(current);
        }

        // Restore previous dedup state (or clear it).
        if body_has_distinct {
            *self.repeat_dedup_state.borrow_mut() = prev_dedup_state;
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

        for row in input {
            cx.budget.check_or_err()?;

            // Evaluate the selector to get a dispatch value.
            self.foreach_seed.borrow_mut().replace(vec![row.clone()]);
            let sel_rows = self.exec_operator(cx, selector)?;
            self.foreach_seed.borrow_mut().take();

            // Check if the selector was unproductive (returned no rows or only nulls).
            let selector_unproductive = sel_rows.is_empty()
                || sel_rows.first().is_none_or(|r| {
                    r.get(selector_alias.0.as_str())
                        .is_none_or(|v| matches!(v, Value::Null))
                        && r.values().all(|v| matches!(v, Value::Null))
                });

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

            // When the selector is unproductive (e.g. values("age") on a vertex
            // without "age"), look for an "unproductive" option branch. If none
            // exists, the traverser passes through as identity.
            if selector_unproductive {
                let mut unproductive_branch = None;
                let mut unproductive_alias = None;
                for (match_val, branch, branch_alias) in options.iter() {
                    if let Some(Value::Str(s)) = match_val {
                        if s.as_ref() == "Pick.unproductive" || s.as_ref() == "unproductive" {
                            unproductive_branch = Some(branch);
                            unproductive_alias = branch_alias.as_ref();
                            break;
                        }
                    }
                }
                if let Some(branch) = unproductive_branch {
                    self.foreach_seed.borrow_mut().replace(vec![row]);
                    let branch_rows = self.exec_operator(cx, branch)?;
                    self.foreach_seed.borrow_mut().take();
                    for mut brow in branch_rows {
                        if let (Some(out_a), Some(src_a)) = (output_alias, unproductive_alias) {
                            if let Some(val) = brow.get(src_a.0.as_str()).cloned() {
                                brow.insert(out_a.0.to_string(), val);
                            }
                        }
                        out.push(brow);
                    }
                } else {
                    // No unproductive branch → identity (pass through)
                    out.push(row);
                }
                continue;
            }

            // Find ALL matching option branches (Gremlin allows multiple
            // options with the same key — all matching branches execute).
            let mut matched_branches: Vec<(&Operator, Option<&crate::ast::Identifier>)> =
                Vec::new();
            let mut default_branch: Option<&Operator> = None;
            let mut default_alias: Option<&crate::ast::Identifier> = None;
            for (match_val, branch, branch_alias) in options {
                match match_val {
                    None => {
                        // Keep only the first default (Pick.none) branch;
                        // subsequent option(Pick.none, ...) entries are ignored
                        // per TinkerPop semantics.
                        if default_branch.is_none() {
                            default_branch = Some(branch);
                            default_alias = branch_alias.as_ref();
                        }
                    }
                    Some(v) => {
                        // Skip "unproductive" sentinel (handled above)
                        if let Value::Str(s) = v {
                            if s.as_ref() == "Pick.unproductive" || s.as_ref() == "unproductive" {
                                continue;
                            }
                            // Pick.any — always matches (wildcard)
                            if s.as_ref() == "Pick.any" || s.as_ref() == "any" {
                                matched_branches.push((branch, branch_alias.as_ref()));
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
                            // For predicate-based matching (P.*), only the
                            // first match fires (if-else-if semantics). For
                            // exact-value matching, all matching options fire.
                            let is_predicate_match =
                                matches!(v, Value::Str(s) if s.starts_with("P."));
                            matched_branches.push((branch, branch_alias.as_ref()));
                            if is_predicate_match {
                                break;
                            }
                        }
                    }
                }
            }

            if matched_branches.is_empty() {
                if let Some(branch) = default_branch {
                    matched_branches.push((branch, default_alias));
                }
            }

            if !matched_branches.is_empty() {
                for (branch, branch_alias) in &matched_branches {
                    self.foreach_seed.borrow_mut().replace(vec![row.clone()]);
                    let branch_rows = self.exec_operator(cx, branch)?;
                    self.foreach_seed.borrow_mut().take();
                    for mut brow in branch_rows {
                        if let (Some(out_a), Some(src_a)) = (output_alias, branch_alias) {
                            if let Some(val) = brow.get(src_a.0.as_str()).cloned() {
                                brow.insert(out_a.0.to_string(), val);
                            }
                        }
                        out.push(brow);
                    }
                }
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
        let Operator::Cap {
            keys,
            child,
            reductions,
        } = op
        else {
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

            // Check the side-effect store first — it has the authoritative
            // accumulated value for aggregate() / local(aggregate()) patterns.
            let se_val = self.gremlin_side_effects.borrow().get(&agg_col).cloned();

            let agg_vals: Vec<&Value> = child_rows.iter().filter_map(|r| r.get(&agg_col)).collect();
            if agg_vals.is_empty() {
                if let Some(stored) = se_val {
                    row.insert(key.clone(), stored);
                } else {
                    row.insert(key.clone(), Value::List(Vec::new()));
                }
                continue;
            }
            // If there is only one distinct value (all rows have the same),
            // take it directly.  Otherwise merge partial lists/maps.
            let first = agg_vals[0];
            let all_same = agg_vals.iter().all(|v| *v == first);
            let cap_value = if let Some(stored) = &se_val {
                // For List aggregations, prefer the side-effect store which
                // has the authoritative accumulated value (handles local(aggregate)).
                if matches!(stored, Value::List(_)) {
                    stored.clone()
                } else if all_same {
                    first.clone()
                } else if matches!(first, Value::Map(_)) {
                    // Merge maps from rows
                    let mut merged: IndexMap<Box<str>, Value> = IndexMap::new();
                    for v in &agg_vals {
                        if let Value::Map(m) = v {
                            for (mk, mv) in m {
                                match (merged.get(mk), mv) {
                                    (Some(Value::Int(existing)), Value::Int(incoming)) => {
                                        merged.insert(mk.clone(), Value::Int(existing + incoming));
                                    }
                                    (Some(Value::Float(existing)), Value::Float(incoming)) => {
                                        merged
                                            .insert(mk.clone(), Value::Float(existing + incoming));
                                    }
                                    _ => {
                                        merged.insert(mk.clone(), mv.clone());
                                    }
                                }
                            }
                        }
                    }
                    Value::Map(merged)
                } else {
                    stored.clone()
                }
            } else if all_same {
                first.clone()
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
                        Value::List(merged)
                    }
                    Value::Map(_) => {
                        let mut merged: IndexMap<Box<str>, Value> = IndexMap::new();
                        for v in &agg_vals {
                            if let Value::Map(m) = v {
                                for (mk, mv) in m {
                                    // For integer values (groupCount), add counts
                                    // instead of overwriting so accumulation across
                                    // traverser branches works correctly.
                                    match (merged.get(mk), mv) {
                                        (Some(Value::Int(existing)), Value::Int(incoming)) => {
                                            merged.insert(
                                                mk.clone(),
                                                Value::Int(existing + incoming),
                                            );
                                        }
                                        (Some(Value::Float(existing)), Value::Float(incoming)) => {
                                            merged.insert(
                                                mk.clone(),
                                                Value::Float(existing + incoming),
                                            );
                                        }
                                        _ => {
                                            merged.insert(mk.clone(), mv.clone());
                                        }
                                    }
                                }
                            }
                        }
                        Value::Map(merged)
                    }
                    _ => first.clone(),
                }
            };

            // Check for a reduction operator (from withSideEffect with Operator.*)
            let reduction = reductions.iter().find(|(k, _, _)| k == key);
            if let Some((_k, op, init)) = reduction {
                if op == "addAll" || op == "assign" {
                    // addAll / assign: the cap value is already the full list
                    // (initial values + aggregated values). Return as-is.
                    row.insert(key.clone(), cap_value);
                } else if let Value::List(items) = &cap_value {
                    // Handle boolean reductions (and/or)
                    if op == "and" || op == "or" {
                        let init_bool = *init != 0.0;
                        let mut acc = init_bool;
                        for item in items {
                            let v = match item {
                                Value::Bool(b) => *b,
                                Value::Int(n) => *n != 0,
                                _ => continue,
                            };
                            match op.as_str() {
                                "and" => acc = acc && v,
                                "or" => acc = acc || v,
                                _ => {}
                            }
                        }
                        row.insert(key.clone(), Value::Bool(acc));
                    } else {
                        // Numeric reductions
                        let mut acc = *init;
                        let init_is_int = (*init - init.round()).abs() < 1e-12;
                        let mut all_int = init_is_int;
                        for item in items {
                            let v = match item {
                                Value::Int(n) => *n as f64,
                                Value::Float(f) => {
                                    all_int = false;
                                    *f
                                }
                                _ => continue,
                            };
                            match op.as_str() {
                                "sum" | "add" => acc += v,
                                "minus" | "sub" => acc -= v,
                                "mult" | "multiply" => acc *= v,
                                "div" | "divide" => {
                                    if v != 0.0 {
                                        all_int = false;
                                        acc /= v;
                                    }
                                }
                                "min" => {
                                    if v < acc {
                                        acc = v;
                                    }
                                }
                                "max" => {
                                    if v > acc {
                                        acc = v;
                                    }
                                }
                                _ => acc += v,
                            }
                        }
                        if all_int && !matches!(op.as_str(), "div" | "divide") {
                            row.insert(key.clone(), Value::Int(acc as i64));
                        } else {
                            row.insert(key.clone(), Value::Float(acc));
                        }
                    }
                } else {
                    row.insert(key.clone(), cap_value);
                }
            } else {
                row.insert(key.clone(), cap_value);
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
            by_traversal,
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

        // Pre-seed from withSideEffect initial value on the very first call.
        // The initial value is stored in __side_effect_{key} column by the
        // compiler. Only pre-seed if the side-effect store has no entry yet.
        let mut collected: Vec<Value> = Vec::new();
        {
            let se = self.gremlin_side_effects.borrow();
            if se.contains_key(&agg_col) {
                // Already seeded from a prior call — don't re-seed.
            } else {
                let se_col = format!("__side_effect_{key}");
                if let Some(row) = input.first() {
                    if let Some(Value::List(items)) = row.get(&se_col) {
                        collected.extend(items.iter().cloned());
                    }
                }
            }
        }
        for row in &input {
            if let Some((trav_op, trav_alias)) = by_traversal {
                // by(traversal) modulation: run the sub-traversal per traverser
                self.foreach_seed.borrow_mut().replace(vec![row.clone()]);
                let trav_rows = self.exec_operator(cx, trav_op)?;
                self.foreach_seed.borrow_mut().take();
                if let Some(trav_row) = trav_rows.first() {
                    if let Some(val) = trav_row.get(trav_alias.as_str()) {
                        if *productive || !matches!(val, Value::Null) {
                            collected.push(val.clone());
                        }
                    } else if *productive {
                        collected.push(Value::Null);
                    }
                } else if *productive {
                    collected.push(Value::Null);
                }
            } else if by_keys.len() >= 2 {
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
        //
        // Also accumulate into the side-effect store so that cap() can
        // retrieve the full list even when downstream steps filter out rows.
        {
            let mut se = self.gremlin_side_effects.borrow_mut();
            if let Some(Value::List(existing)) = se.get_mut(&agg_col) {
                existing.extend(collected.iter().cloned());
            } else {
                se.insert(agg_col.clone(), Value::List(collected.clone()));
            }
        }
        if input.is_empty() {
            // Even with no input, the side-effect must be accessible via cap().
            let mut sentinel = Row::new();
            let stored = self
                .gremlin_side_effects
                .borrow()
                .get(&agg_col)
                .cloned()
                .unwrap_or(Value::List(collected));
            sentinel.insert(agg_col, stored);
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

    /// Resolve a user-facing label (e.g. "b") to the internal alias name
    /// that appears in path_aliases, by looking up the row for a matching
    /// Gremlin `as()` label column.
    fn resolve_label_alias(
        &self,
        label: &str,
        path_aliases: &[Identifier],
        row: &Row,
    ) -> Option<String> {
        // The label may have been recorded as `__gremlin_label_<label>` pointing to
        // an alias like "v_2". Check if any path alias matches the stored value.
        let label_key = format!("__gremlin_label_{label}");
        if let Some(Value::Str(alias_name)) = row.get(&label_key) {
            if path_aliases
                .iter()
                .any(|a| a.as_str() == alias_name.as_ref())
            {
                return Some(alias_name.to_string());
            }
        }
        // Direct match: the label IS an alias in path_aliases
        if path_aliases.iter().any(|a| a.as_str() == label) && row.contains_key(label) {
            return Some(label.to_string());
        }
        // Try to find the alias that was mapped to this label in the row
        for alias in path_aliases {
            if row.contains_key(alias.as_str()) {
                let as_key = format!("__gremlin_as_{}", alias.as_str());
                if let Some(Value::Str(l)) = row.get(&as_key) {
                    if l.as_ref() == label {
                        return Some(alias.as_str().to_string());
                    }
                }
            }
        }
        None
    }

    /// Apply a single by-key transformation to a path element value.
    fn apply_path_by_key(&self, cx: &mut ExecContext<'_>, key: &str, val: &Value) -> Value {
        if key.is_empty() {
            return val.clone();
        }
        // Handle ~toupper:prop and ~tolower:prop
        if let Some(prop) = key.strip_prefix("~toupper:") {
            let inner = self.apply_path_by_key(cx, prop, val);
            return match inner {
                Value::Str(s) => Value::Str(s.to_uppercase().into_boxed_str()),
                other => other,
            };
        }
        if let Some(prop) = key.strip_prefix("~tolower:") {
            let inner = self.apply_path_by_key(cx, prop, val);
            return match inner {
                Value::Str(s) => Value::Str(s.to_lowercase().into_boxed_str()),
                other => other,
            };
        }
        // Handle ~unfold_values_fold:prop — collect property from list of nodes
        if let Some(prop) = key.strip_prefix("~unfold_values_fold:") {
            if let Value::List(items) = val {
                let names: Vec<Value> = items
                    .iter()
                    .filter_map(|item| {
                        let v = self.apply_path_by_key(cx, prop, item);
                        if matches!(v, Value::Null) {
                            None
                        } else {
                            Some(v)
                        }
                    })
                    .collect();
                return Value::List(names);
            }
            return self.apply_path_by_key(cx, prop, val);
        }
        match key {
            "~label" | "T.label" | "label" => match val {
                Value::Node(nid) => {
                    if let Ok(Some(node)) = self.store.get_node(cx.graph, *nid) {
                        if let Some(label) = node.labels.into_iter().next() {
                            return Value::Str(label.into_boxed_str());
                        }
                    }
                    Value::Null
                }
                Value::Edge(eid) => {
                    if let Ok(Some(edge)) = self.store.get_edge(cx.graph, *eid) {
                        return Value::Str(edge.rel_type.into_boxed_str());
                    }
                    Value::Null
                }
                _ => val.clone(),
            },
            "~id" | "T.id" | "id" => match val {
                Value::Node(nid) => Value::Int(nid.0 as i64),
                Value::Edge(eid) => Value::Int(eid.0 as i64),
                _ => val.clone(),
            },
            _ => {
                // Property access
                match val {
                    Value::Node(nid) => {
                        if let Ok(Some(node)) = self.store.get_node(cx.graph, *nid) {
                            if let Some(v) = node.properties.get(key) {
                                return v.clone();
                            }
                        }
                        Value::Null
                    }
                    Value::Edge(eid) => {
                        if let Ok(Some(edge)) = self.store.get_edge(cx.graph, *eid) {
                            if let Some(v) = edge.properties.get(key) {
                                return v.clone();
                            }
                        }
                        Value::Null
                    }
                    _ => val.clone(),
                }
            }
        }
    }

    /// Execute `GremlinMatch` — declarative pattern matching with named bindings.
    ///
    /// Resolve a property on a Value (Node or Edge). Used by select()
    /// by-modulation to apply property extraction to Pop-resolved values.
    fn resolve_prop_on_value(
        &self,
        cx: &mut ExecContext<'_>,
        prop_name: &str,
        val: &Value,
    ) -> Value {
        self.apply_path_by_key(cx, prop_name, val)
    }

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
        let Operator::GremlinSelect {
            keys,
            productive,
            pop,
            label_stacks,
            child,
        } = op
        else {
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

                // Pop modifier handling: when Pop is set and label_stacks has
                // multiple aliases for this key, resolve values from all aliases.
                if let Some(pop_mode) = pop {
                    use crate::planner::GremlinPop;
                    let stack = label_stacks
                        .iter()
                        .find(|(k, _)| k == output_key)
                        .map(|(_, v)| v.as_slice())
                        .unwrap_or(&[]);
                    if !stack.is_empty() {
                        // Collect all values from the label stack
                        let mut vals: Vec<Value> = Vec::new();
                        for alias in stack {
                            if let Some(v) = row.get(alias.0.as_str()) {
                                vals.push(v.clone());
                            }
                        }
                        if vals.is_empty() {
                            all_found = false;
                            break;
                        }
                        let resolved = match pop_mode {
                            GremlinPop::First => vals.into_iter().next().unwrap(),
                            GremlinPop::Last => vals.into_iter().last().unwrap(),
                            GremlinPop::All => Value::List(vals),
                            GremlinPop::Mixed => {
                                if vals.len() == 1 {
                                    vals.into_iter().next().unwrap()
                                } else {
                                    Value::List(vals)
                                }
                            }
                        };
                        // Apply by-modulation: if src_var has a __prop_ prefix,
                        // extract the property from the resolved value.
                        let resolved = if let Some(rest) = var_name.strip_prefix("__prop_") {
                            if let Some(sep_pos) = rest.find("__") {
                                let prop_name = &rest[..sep_pos];
                                self.resolve_prop_on_value(cx, prop_name, &resolved)
                            } else {
                                resolved
                            }
                        } else {
                            resolved
                        };
                        new_row.insert(output_key.clone(), resolved);
                        continue;
                    }
                }

                // Handle property-accessor pattern: __prop_PROPNAME__VARNAME
                if let Some(rest) = var_name.strip_prefix("__prop_") {
                    if let Some(sep_pos) = rest.find("__") {
                        let prop_name = &rest[..sep_pos];
                        let real_var = &rest[sep_pos + 2..];
                        let val = row.get(real_var);
                        match val {
                            Some(node_val) => {
                                // Handle special property names (~id, T.id, ~label, T.label)
                                let resolved = if prop_name == "~id"
                                    || prop_name == "T.id"
                                    || prop_name == "t[id]"
                                {
                                    match node_val {
                                        Value::Node(nid) => Some(Value::Int(nid.0 as i64)),
                                        Value::Edge(eid) => Some(Value::Int(eid.0 as i64)),
                                        _ => None,
                                    }
                                } else if prop_name == "~label"
                                    || prop_name == "T.label"
                                    || prop_name == "t[label]"
                                {
                                    match node_val {
                                        Value::Node(nid) => {
                                            if let Ok(Some(node)) =
                                                self.store.get_node(cx.graph, *nid)
                                            {
                                                node.labels
                                                    .into_iter()
                                                    .next()
                                                    .map(|l| Value::Str(l.into_boxed_str()))
                                            } else {
                                                None
                                            }
                                        }
                                        Value::Edge(eid) => {
                                            if let Ok(Some(edge)) =
                                                self.store.get_edge(cx.graph, *eid)
                                            {
                                                Some(Value::Str(edge.rel_type.into_boxed_str()))
                                            } else {
                                                None
                                            }
                                        }
                                        _ => None,
                                    }
                                } else {
                                    None
                                };

                                if let Some(v) = resolved {
                                    new_row.insert(output_key.clone(), v);
                                    continue;
                                }

                                // Resolve property from the node/edge
                                let prop_expr = Expr::Property(
                                    Box::new(Expr::Variable(Identifier::new(real_var))),
                                    smol_str::SmolStr::new(prop_name),
                                );
                                match self.eval_with_storage(cx.graph, &prop_expr, row, &ctx) {
                                    Ok(v) => {
                                        if !*productive && matches!(v, Value::Null) {
                                            all_found = false;
                                            break;
                                        }
                                        new_row.insert(output_key.clone(), v);
                                    }
                                    Err(_) => {
                                        // Property doesn't exist
                                        if !*productive {
                                            // Non-productive: drop the traverser
                                            if let Value::Map(m) = node_val {
                                                let v = m.get(prop_name).cloned();
                                                match v {
                                                    Some(val) => {
                                                        new_row.insert(output_key.clone(), val);
                                                    }
                                                    None => {
                                                        all_found = false;
                                                        break;
                                                    }
                                                }
                                            } else {
                                                all_found = false;
                                                break;
                                            }
                                        } else {
                                            // Productive: use Null
                                            if let Value::Map(m) = node_val {
                                                let v = m
                                                    .get(prop_name)
                                                    .cloned()
                                                    .unwrap_or(Value::Null);
                                                new_row.insert(output_key.clone(), v);
                                            } else {
                                                new_row.insert(output_key.clone(), Value::Null);
                                            }
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
                } else if let Some(v) = row.get(&format!("__gremlin_agg_{}", output_key)) {
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
                    // Carry forward Gremlin internal columns (path
                    // accumulator, sack, side-effects) so downstream steps
                    // can access them after multi-key select.
                    for (k, v) in row.iter() {
                        if k.starts_with("__gremlin_") || k == "__sack" {
                            map_row.entry(k.clone()).or_insert_with(|| v.clone());
                        }
                    }
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
                    for (src_var, output_key) in keys {
                        if let Some(v) = new_row.get(output_key.as_str()) {
                            out_row.insert(output_key.clone(), v.clone());
                            // For Pop select, also update the source variable
                            // so downstream steps using the compiler alias get
                            // the Pop-resolved value.
                            if pop.is_some() {
                                out_row.insert(src_var.0.to_string(), v.clone());
                            }
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
            key_traversal,
            value_traversal,
            productive,
            explicit_fold,
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
        // Also collect raw rows per group for value_traversal (which runs on group members)
        let mut group_rows: IndexMap<Box<str>, Vec<Row>> = IndexMap::new();

        // Check if value_expr differs from key_expr (i.e., a value-by modulation is present).
        let has_value_by = value_expr != key_expr || value_traversal.is_some();

        for row in &input {
            cx.budget.check_or_err()?;
            let ctx = self.build_filter_ctx(cx.graph, row)?;

            // Compute group key: use traversal if available, else eval expression.
            let key_val = if let Some((key_op, key_alias)) = key_traversal {
                self.foreach_seed.borrow_mut().replace(vec![row.clone()]);
                let key_rows = self.exec_operator(cx, key_op)?;
                self.foreach_seed.borrow_mut().take();
                key_rows
                    .first()
                    .and_then(|r| r.get(key_alias.0.as_str()).cloned())
                    .or_else(|| key_rows.first().and_then(|r| r.values().next().cloned()))
                    .unwrap_or(Value::Null)
            } else {
                self.eval_with_storage(cx.graph, key_expr, row, &ctx)
                    .unwrap_or(Value::Null)
            };
            // Skip rows where the group key evaluates to Null (unproductive by),
            // unless ProductiveByStrategy is active.
            if matches!(key_val, Value::Null) && !*productive {
                continue;
            }

            let key_str = value_to_map_key(&key_val);

            if value_traversal.is_some() {
                // For value_traversal mode, collect rows; we'll run traversal per group later
                group_rows.entry(key_str).or_default().push(row.clone());
            } else {
                // For expression-based value, evaluate per row
                let val = self
                    .eval_with_storage(cx.graph, value_expr, row, &ctx)
                    .unwrap_or(Value::Null);
                let entry = groups.entry(key_str).or_default();
                if has_value_by && matches!(val, Value::Null) {
                    // Key still gets created with empty vec
                } else {
                    entry.push(val);
                }
            }
        }

        // For value_traversal, run the traversal per group with all members as seed
        if let Some((val_op, val_alias)) = value_traversal {
            for (key_str, rows) in &group_rows {
                self.foreach_seed.borrow_mut().replace(rows.clone());
                let val_rows = self.exec_operator(cx, val_op)?;
                self.foreach_seed.borrow_mut().take();
                let mut values = Vec::new();
                for r in &val_rows {
                    if let Some(v) = r.get(val_alias.0.as_str()).cloned() {
                        values.push(v);
                    } else if let Some(v) = r.values().next().cloned() {
                        values.push(v);
                    }
                }
                groups.insert(key_str.clone(), values);
            }
        }

        // Apply aggregation function to each group.
        let has_value_trav = value_traversal.is_some();
        let mut result_map: IndexMap<Box<str>, Value> = IndexMap::new();
        for (key_str, values) in groups {
            // When value traversal produces no results for a group member,
            // skip the entry for reducing aggregations and non-explicit Collect.
            if has_value_trav
                && values.is_empty()
                && (matches!(
                    agg_fn,
                    AggFunction::Sum | AggFunction::Min | AggFunction::Max | AggFunction::Avg
                ) || (matches!(agg_fn, AggFunction::Collect) && !*explicit_fold))
            {
                continue;
            }
            let agg_result = if has_value_trav {
                // When value_traversal is set, the traversal may produce a single
                // aggregated result (e.g., constant(1), count(), sum()) or multiple
                // per-member results to be collected. Apply agg_fn to the values,
                // then unwrap single scalar results for the single-result case.
                match agg_fn {
                    AggFunction::Count => Value::Int(values.len() as i64),
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
                    AggFunction::Collect => {
                        if *explicit_fold {
                            // Explicit fold(): always produce a List
                            Value::List(values)
                        } else if values.len() == 1
                            && !matches!(values.first(), Some(Value::List(_)))
                        {
                            // Single scalar result (e.g. constant(1)): unwrap
                            values.into_iter().next().unwrap_or(Value::Null)
                        } else {
                            Value::List(values)
                        }
                    }
                    _ => Value::List(values),
                }
            } else {
                match agg_fn {
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
                }
            };
            result_map.insert(key_str, agg_result);
        }

        // Named side-effect: store the Map on every input row and pass
        // traversers through instead of collapsing into a single Map row.
        if let Some(key) = side_effect_key {
            let agg_col = format!("__gremlin_agg_{key}");
            let mut output = input;
            if output.is_empty() {
                // Even with no input, the side-effect must be accessible via cap().
                let mut sentinel = Row::new();
                sentinel.insert(agg_col.clone(), Value::Map(result_map.clone()));
                // Store in side-effect store for cap() retrieval
                self.gremlin_side_effects
                    .borrow_mut()
                    .insert(agg_col, Value::Map(result_map));
                return Ok(vec![sentinel]);
            }
            // Merge new counts with any existing aggregation carried from
            // a previous repeat iteration so counts accumulate.
            let map_val = {
                let mut merged = result_map;
                if let Some(first) = output.first() {
                    if let Some(Value::Map(existing)) = first.get(&agg_col) {
                        for (ek, ev) in existing {
                            let entry = merged.entry(ek.clone()).or_insert(Value::Int(0));
                            match (entry.clone(), ev) {
                                (Value::Int(a), Value::Int(b)) => *entry = Value::Int(a + b),
                                (Value::Float(a), Value::Float(b)) => *entry = Value::Float(a + b),
                                (Value::List(mut a), Value::List(b)) => {
                                    a.extend(b.iter().cloned());
                                    *entry = Value::List(a);
                                }
                                _ => {}
                            }
                        }
                    }
                }
                Value::Map(merged)
            };
            // Store in side-effect store for cap() retrieval
            self.gremlin_side_effects
                .borrow_mut()
                .insert(agg_col.clone(), map_val.clone());
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
            if is_simple {
                out.push(row);
            }
        }
        Ok(out)
    }

    /// Execute `GremlinCyclicPath` — filter traversers whose path contains
    /// at least one repeated object (by node/edge identity).
    pub(crate) fn exec_gremlin_cyclic_path(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::GremlinCyclicPath {
            child,
            path_aliases,
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

        // Find start/end indices based on label aliases
        let find_label_idx =
            |label: &str, from_end: bool| -> Option<usize> {
                // Check if any path alias was mapped to this label
                for (i, (alias, _)) in full_path.iter().enumerate().rev().take(if from_end {
                    full_path.len()
                } else {
                    0
                }) {
                    if !alias.is_empty() {
                        if let Some(Value::Str(l)) = row.get(&format!("__gremlin_as_{alias}")) {
                            if l.as_ref() == label {
                                return Some(i);
                            }
                        }
                    }
                }
                // Direct alias match
                for (i, (alias, _)) in full_path.iter().enumerate() {
                    if *alias == label {
                        return Some(i);
                    }
                    // Check if this alias stores the label mapping
                    if !alias.is_empty() && row.contains_key(label) {
                        let label_key = format!("__gremlin_label_{label}");
                        if let Some(Value::Str(mapped)) = row.get(&label_key) {
                            if mapped.as_ref() == *alias {
                                return Some(i);
                            }
                        }
                    }
                }
                None
            };

        let start = from_label
            .and_then(|l| find_label_idx(l, false))
            .unwrap_or(0);
        let end = to_label
            .and_then(|l| find_label_idx(l, true))
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

        // Stream mode: each individual traverser gets index 0 (per TinkerPop semantics).
        if *with_map {
            // with(map) mode: each individual traverser → {0: element}
            for row in input.iter() {
                if let Some(val) = row.get(alias.as_str()) {
                    let mut map = IndexMap::new();
                    map.insert("0".to_string().into_boxed_str(), val.clone());
                    let mut new_row = Row::new();
                    new_row.insert(out_alias.0.clone(), Value::Map(map));
                    out.push(new_row);
                }
            }
        } else {
            // Collect all [element, 0] pairs into a single list for unfold().
            let mut pairs = Vec::with_capacity(input.len());
            for row in input.iter() {
                cx.budget.check_or_err()?;
                let val = row.get(alias.as_str()).cloned().unwrap_or(Value::Null);
                pairs.push(Value::List(vec![val, Value::Int(0)]));
            }
            let mut row = Row::new();
            row.insert(out_alias.0.clone(), Value::List(pairs));
            out.push(row);
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
                // Omit keys where by-modulation produced Null (property
                // doesn't exist), unless ProductiveByStrategy is active.
                if matches!(val, Value::Null) && !*productive {
                    continue;
                }
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
        Value::Float(f) => {
            let s = format!("{f}");
            // Ensure floats always have a decimal point for consistent key format
            if s.contains('.') || s.contains('E') || s.contains('e') {
                s.into_boxed_str()
            } else {
                format!("{s}.0").into_boxed_str()
            }
        }
        Value::Bool(b) => format!("{b}").into_boxed_str(),
        Value::Null => "null".to_string().into_boxed_str(),
        Value::Node(nid) => format!("v[{}]", nid.0).into_boxed_str(),
        Value::Edge(eid) => format!("e[{}]", eid.0).into_boxed_str(),
        Value::List(items) => {
            // Gremlin list key format: l[elem1,elem2,...] (no spaces)
            let parts: Vec<String> = items
                .iter()
                .map(|item| match item {
                    Value::Str(s) => s.to_string(),
                    Value::Int(n) => n.to_string(),
                    Value::Float(f) => f.to_string(),
                    Value::Bool(b) => b.to_string(),
                    Value::Null => "null".to_string(),
                    Value::Node(nid) => format!("v[{}]", nid.0),
                    Value::Edge(eid) => format!("e[{}]", eid.0),
                    other => format!("{other:?}"),
                })
                .collect();
            format!("l[{}]", parts.join(",")).into_boxed_str()
        }
        Value::Map(m) => {
            let parts: Vec<String> = m
                .iter()
                .map(|(k, v)| {
                    let vs = match v {
                        Value::Str(s) => s.to_string(),
                        Value::Int(n) => n.to_string(),
                        Value::Float(f) => f.to_string(),
                        other => format!("{other:?}"),
                    };
                    format!("{k}={vs}")
                })
                .collect();
            format!("{{{}}}", parts.join(", ")).into_boxed_str()
        }
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

    // Handle compound predicates: .or(P.xxx) and .and(P.xxx)
    // Split at top-level .or() / .and() boundaries.
    if let Some((left, right, is_or)) = split_compound_predicate(pred) {
        let left_str = if left.starts_with("P.") {
            left.to_string()
        } else {
            format!("P.{left}")
        };
        let right_str = if right.starts_with("P.") {
            right.to_string()
        } else {
            format!("P.{right}")
        };
        let left_result = eval_gremlin_predicate(&left_str, value);
        let right_result = eval_gremlin_predicate(&right_str, value);
        return if is_or {
            left_result || right_result
        } else {
            left_result && right_result
        };
    }

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

/// Split a compound predicate like "between(26, 30).or(gt(34))" into
/// (left_pred, right_pred, is_or). Returns None if not compound.
fn split_compound_predicate(pred: &str) -> Option<(&str, &str, bool)> {
    // Look for .or( or .and( at the top level (outside balanced parens).
    let mut depth = 0u32;
    let bytes = pred.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    while i < len {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => depth = depth.saturating_sub(1),
            b'.' if depth == 0 && i + 4 <= len => {
                let suffix = &pred[i..];
                if suffix.starts_with(".or(") {
                    let left = &pred[..i];
                    let inner = &pred[i + 4..];
                    // Find matching closing paren for .or(
                    if let Some(close) = inner.strip_suffix(')') {
                        return Some((left, close, true));
                    }
                }
                if suffix.starts_with(".and(") {
                    let left = &pred[..i];
                    let inner = &pred[i + 5..];
                    if let Some(close) = inner.strip_suffix(')') {
                        return Some((left, close, false));
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Check whether an operator tree contains a `Distinct` node (shallow search).
/// Used by `exec_repeat` to decide whether to enable cross-iteration dedup.
fn operator_contains_distinct(op: &Operator) -> bool {
    match op {
        Operator::Distinct { .. } => true,
        // Walk through wrappers that commonly sit between Repeat body root
        // and the actual Distinct.
        Operator::Project { child, .. }
        | Operator::Filter { child, .. }
        | Operator::Sort { child, .. }
        | Operator::Limit { child, .. }
        | Operator::Skip { child, .. }
        | Operator::Expand { child, .. }
        | Operator::GremlinSideEffect { child, .. }
        | Operator::Coalesce { child, .. }
        | Operator::GremlinSimplePath { child, .. }
        | Operator::GremlinCyclicPath { child, .. } => operator_contains_distinct(child),
        Operator::Apply { outer, inner } => {
            operator_contains_distinct(outer) || operator_contains_distinct(inner)
        }
        Operator::LeftOuterApply {
            preserved,
            optional_pattern,
        } => operator_contains_distinct(preserved) || operator_contains_distinct(optional_pattern),
        Operator::Union { left, right, .. } => {
            operator_contains_distinct(left) || operator_contains_distinct(right)
        }
        _ => false,
    }
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
            times_before: false,
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
            times_before: false,
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
            reductions: vec![],
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
            reductions: vec![],
            child: Box::new(Operator::GremlinAggregate {
                key: "x".to_string(),
                child: Box::new(Operator::NodeScan {
                    label: Some(Label::new("N")),
                    alias: Identifier::new("n"),
                }),
                value_alias: Identifier::new("n"),
                by_keys: vec![],
                by_traversal: None,
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
            reductions: vec![],
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
                by_traversal: None,
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
            by_traversal: None,
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
            reductions: vec![],
            child: Box::new(Operator::GremlinAggregate {
                key: "x".to_string(),
                child: Box::new(Operator::NodeScan {
                    label: Some(Label::new("N")),
                    alias: Identifier::new("n"),
                }),
                value_alias: Identifier::new("n"),
                by_keys: vec![],
                by_traversal: None,
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
            reductions: vec![],
            child: Box::new(Operator::GremlinAggregate {
                key: "all".to_string(),
                child: Box::new(Operator::NodeScan {
                    label: Some(Label::new("X")),
                    alias: Identifier::new("n"),
                }),
                value_alias: Identifier::new("n"),
                by_keys: vec![],
                by_traversal: None,
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
            by_keys: vec![],
            productive: false,
            from_label: None,
            to_label: None,
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
            by_keys: vec![],
            productive: false,
            from_label: None,
            to_label: None,
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
            by_keys: vec![],
            productive: false,
            from_label: None,
            to_label: None,
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
            by_keys: vec![],
            productive: false,
            from_label: None,
            to_label: None,
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
            by_keys: vec![],
            productive: false,
            from_label: None,
            to_label: None,
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
