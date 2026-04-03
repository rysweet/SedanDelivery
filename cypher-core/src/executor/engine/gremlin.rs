//! Gremlin traversal operator implementations.

use crate::ast::{Direction, Expr, Identifier};
use crate::error::{ExecutionError, GqlError};
use crate::planner::{GremlinSelectTraversal, Operator};
use crate::storage::NodeId;
use crate::types::Value;
use indexmap::IndexMap;

use super::helpers::{gremlin_cmp_values, inject_parameters};
use crate::executor::engine::{ExecContext, QueryExecutor, Row};

struct JavaCompatibleRandom {
    seed: u64,
}

impl JavaCompatibleRandom {
    const MULTIPLIER: u64 = 0x5DEECE66D;
    const ADDEND: u64 = 0xB;
    const MASK: u64 = (1u64 << 48) - 1;

    fn new(seed: i64) -> Self {
        Self {
            seed: ((seed as u64) ^ Self::MULTIPLIER) & Self::MASK,
        }
    }

    fn next(&mut self, bits: u32) -> u32 {
        self.seed = (self
            .seed
            .wrapping_mul(Self::MULTIPLIER)
            .wrapping_add(Self::ADDEND))
            & Self::MASK;
        (self.seed >> (48 - bits)) as u32
    }

    fn next_double(&mut self) -> f64 {
        let high = self.next(26) as u64;
        let low = self.next(27) as u64;
        ((high << 27) | low) as f64 / ((1u64 << 53) as f64)
    }
}

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
            loop_label,
            body_path_aliases,
            pop_origins,
            needs_path,
            child,
        } = op
        else {
            debug_assert!(false, "expected Repeat operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected Repeat operator".into(),
            }));
        };

        // Loop counter key: __gremlin_loop_count (unlabeled) or
        // __gremlin_loop_count_<label> (labeled). Always inject the generic
        // counter too so that unlabeled loops() reads the innermost repeat.
        let labeled_counter_key = loop_label
            .as_ref()
            .map(|l| format!("__gremlin_loop_count_{l}"));
        let generic_counter_key = "__gremlin_loop_count".to_string();

        let mut current = self.exec_operator(cx, child)?;
        let mut collected: Vec<Row> = Vec::new();
        let mut body_exhausted = false;

        // Preserve original seed values for Pop.first support: copy the
        // original alias value into the shadow column before any remap
        // overwrites it.
        for (shadow, original) in pop_origins {
            for row in &mut current {
                if let Some(val) = row.get(original.as_str()).cloned() {
                    row.insert(shadow.clone(), val);
                }
            }
        }

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

        // Vertex-counting fast path — skip when path tracking is needed
        // because the fast path collapses traversers by destination NodeId
        // and cannot maintain per-traverser path histories.
        if !*needs_path
            && until.is_none()
            && !*emit
            && emit_predicate.is_none()
            && pop_origins.is_empty()
        {
            if let Some(result) = self.try_repeat_counting(
                cx,
                body,
                &current,
                *times,
                max_iterations,
                until,
                *emit,
                *emit_before,
                emit_predicate,
                remap,
                *times_before,
                loop_label,
                body_path_aliases,
                pop_origins,
            )? {
                return Ok(expand_bulk_rows(result));
            }
        }

        // Batch size for streaming seed processing. When `current` exceeds
        // this threshold, seeds are fed to the body in chunks to cap peak
        // memory at BATCH_SIZE × branching_factor instead of
        // total_seeds × branching_factor. This prevents OOM on exponential
        // growth patterns (e.g., repeat(both()).until(loops().is(40)) where
        // each iteration multiplies traversers by the graph's branching factor).
        //
        // When the body contains barrier steps (Sort, Aggregate, Distinct),
        // batching is disabled because barriers must see all rows at once to
        // produce correct results (e.g., order().tail(2) must sort ALL
        // traversers, not each batch independently).
        const BATCH_SIZE: usize = 10_000;
        let body_has_barriers = operator_contains_barrier(body);
        let effective_batch = if body_has_barriers {
            usize::MAX
        } else {
            BATCH_SIZE
        };

        for iteration in 0..max_iterations {
            cx.budget.check_or_err()?;

            // Inject loop counter into current rows so emit predicates and
            // until conditions that reference loops() can see the value.
            for row in &mut current {
                row.insert(generic_counter_key.clone(), Value::Int(iteration as i64));
                if let Some(lk) = &labeled_counter_key {
                    row.insert(lk.clone(), Value::Int(iteration as i64));
                }
            }

            // Pre-emit: collect traversers before body execution.
            // Only when emit_before is true (emit() appeared before repeat()).
            if *emit && *emit_before {
                self.collect_emitted(cx, &current, emit_predicate, &mut collected)?;
            }

            // Execute the repeat body with batched streaming: process seeds
            // in chunks of BATCH_SIZE to cap peak memory. Each batch is
            // independently fed to the body operator and results are merged.
            let mut next: Vec<Row> = Vec::new();
            let mut iteration_agg_maps: std::collections::HashMap<
                String,
                indexmap::IndexMap<Box<str>, Value>,
            > = std::collections::HashMap::new();

            for batch_start in (0..current.len()).step_by(effective_batch) {
                cx.budget.check_or_err()?;
                let batch_end = (batch_start + BATCH_SIZE).min(current.len());
                let batch = &current[batch_start..batch_end];

                // Prepare seed rows for this batch.
                let mut seeds: Vec<Row> = Vec::with_capacity(batch.len());
                for row in batch {
                    let mut seed_row = row.clone();
                    seed_row.insert(generic_counter_key.clone(), Value::Int(iteration as i64));
                    if let Some(lk) = &labeled_counter_key {
                        seed_row.insert(lk.clone(), Value::Int(iteration as i64));
                    }
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
                    // Remove intermediate body path aliases from seed rows
                    // to avoid edge isomorphism conflicts in the next body
                    // execution (e.g., stale e_2 from a previous outE).
                    for alias in body_path_aliases {
                        seed_row.remove(alias.as_str());
                    }
                    seeds.push(seed_row);
                }

                self.foreach_seed.borrow_mut().replace(seeds);
                let body_rows = self.exec_operator(cx, body)?;
                self.foreach_seed.borrow_mut().take();

                // Separate traverser rows from side-effect sentinel rows.
                for body_row in body_rows {
                    if let Some(key) = seed_key {
                        if !body_row.contains_key(key)
                            && remap
                                .as_ref()
                                .is_none_or(|(from, _)| !body_row.contains_key(from.as_str()))
                        {
                            for (k, v) in &body_row {
                                if k.starts_with("__gremlin_agg_") {
                                    if let Value::Map(m) = v {
                                        let acc = iteration_agg_maps.entry(k.clone()).or_default();
                                        for (mk, mv) in m {
                                            merge_gremlin_map_entry(acc, mk, mv);
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
                    cx.budget.record_row();
                    next.push(body_row);
                }
            }

            // Merge side-effect agg maps from body results.
            if let Some(first) = next.first() {
                for (k, v) in first {
                    if k.starts_with("__gremlin_agg_") {
                        if let Value::Map(m) = v {
                            let acc = iteration_agg_maps.entry(k.clone()).or_default();
                            for (mk, mv) in m {
                                merge_gremlin_map_entry(acc, mk, mv);
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
                if !*emit {
                    return Ok(collected);
                }
                break;
            }

            // Accumulate intermediate body path aliases (e.g., edge values from
            // outE) into __gremlin_path_acc so path() sees them.
            if !body_path_aliases.is_empty() {
                for row in &mut next {
                    let vals: Vec<Value> = body_path_aliases
                        .iter()
                        .filter_map(|alias| row.get(alias.as_str()).cloned())
                        .collect();
                    if !vals.is_empty() {
                        let mut acc = match row.get("__gremlin_path_acc") {
                            Some(Value::List(l)) => l.clone(),
                            _ => vec![],
                        };
                        acc.extend(vals);
                        row.insert("__gremlin_path_acc".to_string(), Value::List(acc));
                    }
                }
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
            cx.budget.check_or_err()?;

            // Update loop count: after body execution in iteration N, the
            // traverser has completed N+1 loops.
            for row in &mut current {
                row.insert(
                    generic_counter_key.clone(),
                    Value::Int((iteration + 1) as i64),
                );
                if let Some(lk) = &labeled_counter_key {
                    row.insert(lk.clone(), Value::Int((iteration + 1) as i64));
                }
            }

            // Post-emit: collect body results after execution.
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
        // - Non-emit mode with times (no until): add final current
        // - Non-emit mode with until: don't add (never matched exit condition)
        // - Pre-emit with times (body not exhausted): add final current
        // - Post-emit or body exhausted: don't add (already collected)
        let add_final = if *emit {
            *emit_before && !body_exhausted
        } else {
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

    /// Vertex-counting fast path for repeat(Expand).times(N).
    #[allow(clippy::too_many_arguments)]
    fn try_repeat_counting(
        &self,
        cx: &mut ExecContext<'_>,
        body: &Operator,
        // Pre-computed seeds from executing the child operator. Passed in to
        // avoid re-executing child (which would fail when foreach_seed has
        // already been consumed by the initial exec at the caller site).
        precomputed_seeds: &[Row],
        _times: Option<usize>,
        max_iterations: usize,
        until: &Option<Box<Operator>>,
        emit: bool,
        emit_before: bool,
        emit_predicate: &Option<Box<Operator>>,
        remap: &Option<(String, String)>,
        times_before: bool,
        loop_label: &Option<Box<str>>,
        _body_path_aliases: &[String],
        pop_origins: &[(String, String)],
    ) -> Result<Option<Vec<Row>>, GqlError> {
        if !pop_origins.is_empty() {
            return Ok(None);
        }
        let expand_info = match extract_simple_expand(body) {
            Some(info) => info,
            None => return Ok(None),
        };
        if body_contains_simple_path(body) {
            return Ok(None);
        }

        let (direction, rel_types, src_alias, dst_alias) = expand_info;
        let type_strs: Vec<String> = rel_types.iter().map(|rt| rt.0.to_string()).collect();
        let type_refs: Vec<&str> = type_strs.iter().map(|s| s.as_str()).collect();

        if precomputed_seeds.is_empty() {
            return Ok(Some(Vec::new()));
        }

        let seed_key = remap.as_ref().map(|(_, to)| to.as_str());
        let src_alias_str = src_alias.0.as_str();

        // Simple mode: no emit, no until
        if !emit && until.is_none() && emit_predicate.is_none() {
            let mut vertex_counts: IndexMap<NodeId, (Row, i128)> = IndexMap::new();
            for row in precomputed_seeds {
                let alias_key = seed_key.unwrap_or(src_alias_str);
                if let Some(Value::Node(nid)) = row.get(alias_key) {
                    let entry = vertex_counts
                        .entry(*nid)
                        .or_insert_with(|| (row.clone(), 0));
                    entry.1 += 1;
                }
            }

            for _iteration in 0..max_iterations {
                cx.budget.check_or_err()?;
                let mut next: IndexMap<NodeId, (Row, i128)> = IndexMap::new();
                for (nid, (_row, count)) in &vertex_counts {
                    let entries = super::helpers::get_adj_entries(
                        self.store, cx.graph, *nid, &direction, &type_refs,
                    )?;
                    for adj in &entries {
                        let entry = next.entry(adj.neighbor_id).or_insert_with(|| {
                            let mut r = Row::new();
                            r.insert(dst_alias.0.clone(), Value::Node(adj.neighbor_id));
                            (r, 0)
                        });
                        entry.1 += count;
                    }
                }
                vertex_counts = next;
            }

            let remap_alias = remap.as_ref().map(|(_, to)| to.as_str());
            let mut result = Vec::with_capacity(vertex_counts.len());
            for (nid, (mut row, count)) in vertex_counts {
                let out_key = remap_alias.unwrap_or(dst_alias.0.as_str());
                row.insert(out_key.to_string(), Value::Node(nid));
                if count > 1 {
                    row.insert("__gremlin_bulk".to_string(), Value::Int(count as i64));
                }
                if let Some(sk) = seed_key {
                    row.insert(sk.to_string(), Value::Node(nid));
                }
                result.push(row);
            }
            return Ok(Some(result));
        }

        // Emit + until mode
        if emit || until.is_some() {
            let labeled_counter_key = loop_label
                .as_ref()
                .map(|l| format!("__gremlin_loop_count_{l}"));
            let generic_counter_key = "__gremlin_loop_count".to_string();

            let mut vertex_counts: IndexMap<NodeId, (Row, i128)> = IndexMap::new();
            for row in precomputed_seeds {
                let alias_key = seed_key.unwrap_or(src_alias_str);
                if let Some(Value::Node(nid)) = row.get(alias_key) {
                    let entry = vertex_counts
                        .entry(*nid)
                        .or_insert_with(|| (row.clone(), 0));
                    entry.1 += 1;
                }
            }

            for (shadow, original) in pop_origins {
                for (_nid, (row, _)) in vertex_counts.iter_mut() {
                    if let Some(val) = row.get(original.as_str()).cloned() {
                        row.insert(shadow.clone(), val);
                    }
                }
            }

            let mut collected: Vec<Row> = Vec::new();

            for iteration in 0..max_iterations {
                cx.budget.check_or_err()?;

                for (_nid, (row, _)) in vertex_counts.iter_mut() {
                    row.insert(generic_counter_key.clone(), Value::Int(iteration as i64));
                    if let Some(lk) = &labeled_counter_key {
                        row.insert(lk.clone(), Value::Int(iteration as i64));
                    }
                }

                if emit && emit_before {
                    for (_nid, (row, count)) in &vertex_counts {
                        let mut r = row.clone();
                        if *count > 1 {
                            r.insert("__gremlin_bulk".to_string(), Value::Int(*count as i64));
                        }
                        if let Some(pred_op) = emit_predicate {
                            self.foreach_seed.borrow_mut().replace(vec![r.clone()]);
                            let result = self.exec_operator(cx, pred_op)?;
                            self.foreach_seed.borrow_mut().take();
                            if !result.is_empty() {
                                collected.push(r);
                            }
                        } else {
                            collected.push(r);
                        }
                    }
                }

                if let Some(until_op) = until {
                    if times_before || iteration > 0 {
                        let mut remaining: IndexMap<NodeId, (Row, i128)> = IndexMap::new();
                        for (nid, (row, count)) in &vertex_counts {
                            self.foreach_seed.borrow_mut().replace(vec![row.clone()]);
                            let result = self.exec_operator(cx, until_op)?;
                            self.foreach_seed.borrow_mut().take();
                            if !result.is_empty() {
                                if !emit || !emit_before {
                                    let mut r = row.clone();
                                    if *count > 1 {
                                        r.insert(
                                            "__gremlin_bulk".to_string(),
                                            Value::Int(*count as i64),
                                        );
                                    }
                                    collected.push(r);
                                }
                            } else {
                                remaining.insert(*nid, (row.clone(), *count));
                            }
                        }
                        vertex_counts = remaining;
                        if vertex_counts.is_empty() {
                            break;
                        }
                    }
                }

                let mut next: IndexMap<NodeId, (Row, i128)> = IndexMap::new();
                for (nid, (_row, count)) in &vertex_counts {
                    let entries = super::helpers::get_adj_entries(
                        self.store, cx.graph, *nid, &direction, &type_refs,
                    )?;
                    for adj in &entries {
                        let entry = next.entry(adj.neighbor_id).or_insert_with(|| {
                            let mut r = Row::new();
                            r.insert(dst_alias.0.clone(), Value::Node(adj.neighbor_id));
                            if let Some(sk) = seed_key {
                                r.insert(sk.to_string(), Value::Node(adj.neighbor_id));
                            }
                            (r, 0)
                        });
                        entry.1 += count;
                    }
                }
                vertex_counts = next;

                if vertex_counts.is_empty() {
                    break;
                }

                if emit && !emit_before {
                    for (_nid, (row, count)) in &vertex_counts {
                        let mut r = row.clone();
                        if *count > 1 {
                            r.insert("__gremlin_bulk".to_string(), Value::Int(*count as i64));
                        }
                        if let Some(pred_op) = emit_predicate {
                            self.foreach_seed.borrow_mut().replace(vec![r.clone()]);
                            let result = self.exec_operator(cx, pred_op)?;
                            self.foreach_seed.borrow_mut().take();
                            if !result.is_empty() {
                                collected.push(r);
                            }
                        } else {
                            collected.push(r);
                        }
                    }
                }
            }

            // Mirror the slow-path add_final logic:
            //   emit + pre-emit mode  → add remaining (not yet emitted)
            //   emit + post-emit mode → DON'T add (already emitted in last iteration)
            //   no-emit + no-until   → add final results
            //   no-emit + until only → don't add (until-matched rows already collected)
            //   no-emit + times      → add final (times terminates, add last batch)
            let add_final = if emit {
                emit_before // post-emit already collected final rows; pre-emit hasn't
            } else {
                until.is_none() || _times.is_some()
            };
            if !vertex_counts.is_empty() && add_final {
                for (_nid, (mut row, count)) in vertex_counts {
                    if count > 1 {
                        row.insert("__gremlin_bulk".to_string(), Value::Int(count as i64));
                    }
                    collected.push(row);
                }
            }

            return Ok(Some(collected));
        }

        Ok(None)
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
                None => {
                    if default_idx.is_none() {
                        default_idx = Some(idx);
                    }
                }
                Some(Value::Str(s))
                    if s.as_ref() == "Pick.unproductive" || s.as_ref() == "unproductive" =>
                {
                    if unproductive_idx.is_none() {
                        unproductive_idx = Some(idx);
                    }
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
            if agg_col == "__gremlin_agg_x" {
                append_gremlin_debug_log(&format!(
                    "[cap:x] child_rows={} agg_vals={} stored={}",
                    child_rows.len(),
                    agg_vals.len(),
                    match se_val.as_ref() {
                        Some(Value::Map(m)) => format!("map:{}", m.len()),
                        Some(Value::List(items)) => format!("list:{}", items.len()),
                        Some(other) => other.type_name().to_string(),
                        None => "none".to_string(),
                    }
                ));
            }
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
                if matches!(stored, Value::List(_) | Value::Map(_)) {
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
        // For multi-key cap, also create a combined Map value so that
        // downstream unfold() can iterate over the {key: value} entries.
        if keys.len() > 1 {
            let mut map: IndexMap<Box<str>, Value> = IndexMap::new();
            for key in keys {
                if let Some(v) = row.get(key.as_str()) {
                    map.insert(key.clone().into(), v.clone());
                }
            }
            let map_alias = keys.join("_");
            row.insert(map_alias, Value::Map(map));
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
            set_semantics,
            assign_semantics,
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
        if !*assign_semantics {
            // Pre-seed from withSideEffect initial value on the very first call.
            // Skipped for assign_semantics since assign REPLACES (not appends).
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
                    let val = self.eval_gremlin_by_key(cx, row, value_alias, by_prop)?;
                    map.insert(by_prop.clone().into_boxed_str(), val);
                }
                collected.push(Value::Map(map));
            } else if let Some(by_prop) = by_keys.first() {
                // Single by("prop") modulation: resolve property from the node/edge value
                let projected = self.eval_gremlin_by_key(cx, row, value_alias, by_prop)?;
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
            if *assign_semantics {
                // Operator.assign: REPLACE the accumulated list with the current
                // traverser's values. The last aggregate call wins.
                se.insert(agg_col.clone(), Value::List(collected.clone()));
            } else if let Some(Value::List(existing)) = se.get_mut(&agg_col) {
                if *set_semantics {
                    for v in &collected {
                        if !existing.contains(v) {
                            existing.push(v.clone());
                        }
                    }
                } else {
                    existing.extend(collected.iter().cloned());
                }
            } else {
                let stored = if *set_semantics {
                    let mut deduped = Vec::new();
                    for v in &collected {
                        if !deduped.contains(v) {
                            deduped.push(v.clone());
                        }
                    }
                    deduped
                } else {
                    collected.clone()
                };
                se.insert(agg_col.clone(), Value::List(stored));
            }
        }
        if input.is_empty() {
            // No traversers passed the filter (all edges already visited, etc.).
            // The side-effect store was already updated above, so cap() can
            // retrieve the accumulated value from gremlin_side_effects.
            // Return an empty stream so that downstream steps (like otherV())
            // inside a repeat body do not see a sentinel row that is missing
            // intermediate step variables (e.g. the edge alias from bothE).
            return Ok(vec![]);
        }
        for row in &mut input {
            if *assign_semantics {
                // Operator.assign: REPLACE — row carries only the current values.
                row.insert(agg_col.clone(), Value::List(collected.clone()));
            } else if let Some(Value::List(existing)) = row.get(&agg_col).cloned() {
                let mut merged = existing;
                if *set_semantics {
                    for v in &collected {
                        if !merged.contains(v) {
                            merged.push(v.clone());
                        }
                    }
                } else {
                    merged.extend(collected.iter().cloned());
                }
                row.insert(agg_col.clone(), Value::List(merged));
            } else {
                let stored = if *set_semantics {
                    let mut deduped = Vec::new();
                    for v in &collected {
                        if !deduped.contains(v) {
                            deduped.push(v.clone());
                        }
                    }
                    deduped
                } else {
                    collected.clone()
                };
                row.insert(agg_col.clone(), Value::List(stored));
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

    /// Resolve a label (now pre-resolved at compile time to an internal
    /// alias like "v_2") against path_aliases.  Falls back to the old
    /// row-based resolution for backward compatibility.
    fn resolve_label_alias(
        &self,
        label: &str,
        path_aliases: &[Identifier],
        _row: &Row,
    ) -> Option<String> {
        // Direct match: the label is an internal alias in path_aliases.
        // This is the primary path after compile-time resolution.
        if path_aliases.iter().any(|a| a.as_str() == label) {
            return Some(label.to_string());
        }
        None
    }

    /// Apply a single by-key transformation to a path element value.
    fn apply_path_by_key(&self, cx: &mut ExecContext<'_>, key: &str, val: &Value) -> Value {
        if key.is_empty() {
            return val.clone();
        }
        if let Some(expr) = key.strip_prefix("~math:") {
            return eval_select_math_expr(expr, val);
        }
        if let Some(rest) = key.strip_prefix("~select_map:") {
            return self.select_map_fields(rest, val);
        }
        if let Some(rest) = key.strip_prefix("~in_values_min:") {
            if let Some((rel_type, prop)) = rest.split_once(':') {
                return self.resolve_in_values_min(cx, rel_type, prop, val);
            }
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
            "~key" | "T.key" | "key" => match val {
                Value::Map(m) => m.get("~key").cloned().unwrap_or_else(|| val.clone()),
                _ => val.clone(),
            },
            "~value" | "T.value" | "value" => match val {
                Value::Map(m) => m.get("~value").cloned().unwrap_or_else(|| val.clone()),
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

    pub(crate) fn resolve_prop_on_value_for_filter(
        &self,
        graph: &str,
        prop_name: &str,
        val: &Value,
    ) -> Result<Value, GqlError> {
        if prop_name.is_empty() {
            return Ok(val.clone());
        }
        if let Some(rest) = prop_name.strip_prefix("~select_map:") {
            return Ok(self.select_map_fields(rest, val));
        }
        if let Some(rest) = prop_name.strip_prefix("~in_values_min:") {
            if let Some((rel_type, prop)) = rest.split_once(':') {
                return self.resolve_in_values_min_for_filter(graph, rel_type, prop, val);
            }
        }
        match prop_name {
            "~label" | "T.label" | "label" => match val {
                Value::Node(node_id) => Ok(match self.cached_get_node(graph, *node_id)? {
                    Some(node_data) => node_data
                        .labels
                        .into_iter()
                        .next()
                        .map(|label| Value::Str(label.into_boxed_str()))
                        .unwrap_or(Value::Null),
                    None => Value::Null,
                }),
                Value::Edge(edge_id) => Ok(match self.cached_get_edge(graph, *edge_id)? {
                    Some(edge_data) => Value::Str(edge_data.rel_type.into_boxed_str()),
                    None => Value::Null,
                }),
                _ => Ok(val.clone()),
            },
            "~id" | "T.id" | "id" => Ok(match val {
                Value::Node(nid) => Value::Int(nid.0 as i64),
                Value::Edge(eid) => Value::Int(eid.0 as i64),
                _ => val.clone(),
            }),
            "~key" | "T.key" | "key" => Ok(match val {
                Value::Map(m) => m.get("~key").cloned().unwrap_or_else(|| val.clone()),
                _ => val.clone(),
            }),
            "~value" | "T.value" | "value" => Ok(match val {
                Value::Map(m) => m.get("~value").cloned().unwrap_or_else(|| val.clone()),
                _ => val.clone(),
            }),
            _ => match val {
                Value::Node(node_id) => Ok(match self.cached_get_node(graph, *node_id)? {
                    Some(node_data) => node_data
                        .properties
                        .get(prop_name)
                        .cloned()
                        .unwrap_or(Value::Null),
                    None => Value::Null,
                }),
                Value::Edge(edge_id) => Ok(match self.cached_get_edge(graph, *edge_id)? {
                    Some(edge_data) => edge_data
                        .properties
                        .get(prop_name)
                        .cloned()
                        .unwrap_or(Value::Null),
                    None => Value::Null,
                }),
                Value::Map(m) => Ok(m.get(prop_name).cloned().unwrap_or(Value::Null)),
                _ => Ok(val.clone()),
            },
        }
    }

    fn resolve_in_values_min(
        &self,
        cx: &mut ExecContext<'_>,
        rel_type: &str,
        prop: &str,
        val: &Value,
    ) -> Value {
        let Value::Node(node_id) = val else {
            return Value::Null;
        };

        let rel_types = [rel_type];
        let incoming = match self.store.in_edges_by_type(cx.graph, *node_id, &rel_types) {
            Ok(entries) => entries,
            Err(_) => return Value::Null,
        };
        if incoming.is_empty() {
            return Value::Null;
        }

        let neighbor_ids: Vec<NodeId> = incoming.iter().map(|entry| entry.neighbor_id).collect();
        let nodes = match self.store.get_nodes_batch(cx.graph, &neighbor_ids) {
            Ok(nodes) => nodes,
            Err(_) => return Value::Null,
        };

        let mut best: Option<Value> = None;
        for node in nodes.into_iter().flatten() {
            let Some(candidate) = node.properties.get(prop).cloned() else {
                continue;
            };
            match &best {
                Some(current) if gremlin_cmp_values(&candidate, current).is_ge() => {}
                _ => best = Some(candidate),
            }
        }

        best.unwrap_or(Value::Null)
    }

    fn resolve_in_values_min_for_filter(
        &self,
        graph: &str,
        rel_type: &str,
        prop: &str,
        val: &Value,
    ) -> Result<Value, GqlError> {
        let Value::Node(node_id) = val else {
            return Ok(Value::Null);
        };

        let rel_types = [rel_type];
        let incoming = match self.store.in_edges_by_type(graph, *node_id, &rel_types) {
            Ok(entries) => entries,
            Err(_) => return Ok(Value::Null),
        };
        if incoming.is_empty() {
            return Ok(Value::Null);
        }

        let neighbor_ids: Vec<NodeId> = incoming.iter().map(|entry| entry.neighbor_id).collect();
        let nodes = match self.store.get_nodes_batch(graph, &neighbor_ids) {
            Ok(nodes) => nodes,
            Err(_) => return Ok(Value::Null),
        };

        let mut best: Option<Value> = None;
        for node in nodes.into_iter().flatten() {
            let Some(candidate) = node.properties.get(prop).cloned() else {
                continue;
            };
            match &best {
                Some(current) if gremlin_cmp_values(&candidate, current).is_ge() => {}
                _ => best = Some(candidate),
            }
        }

        Ok(best.unwrap_or(Value::Null))
    }

    fn select_map_fields(&self, fields: &str, val: &Value) -> Value {
        let Value::Map(map) = val else {
            return Value::Null;
        };
        let mut selected = IndexMap::new();
        for field in fields.split(',') {
            if let Some(value) = map.get(field) {
                selected.insert(field.to_string().into_boxed_str(), value.clone());
            }
        }
        Value::Map(selected)
    }

    fn resolve_map_select_value(&self, row: &Row, encoded: &str) -> Option<Value> {
        let rest = encoded.strip_prefix("__map_select__")?;
        let (map_alias, key_alias) = rest.split_once("__keyalias__")?;
        let map_val = row.get(map_alias)?;
        let key_val = row.get(key_alias)?;
        let Value::Map(map) = map_val else {
            return None;
        };
        let key = value_to_map_key(key_val);
        map.get(key.as_ref()).cloned()
    }

    fn split_prop_accessor<'b>(&self, rest: &'b str) -> Option<(&'b str, &'b str)> {
        if let Some(map_pos) = rest.find("__map_select__") {
            if map_pos >= 2 {
                return Some((&rest[..map_pos - 2], &rest[map_pos..]));
            }
        }
        let sep_pos = rest.find("__")?;
        Some((&rest[..sep_pos], &rest[sep_pos + 2..]))
    }

    fn apply_select_by_traversal(
        &self,
        cx: &mut ExecContext<'_>,
        row: &Row,
        src_var: &Identifier,
        selected: Value,
        traversal: &GremlinSelectTraversal,
    ) -> Result<Value, GqlError> {
        let GremlinSelectTraversal::Traversal { op, out_alias } = traversal;
        let mut seed = row.clone();
        seed.insert(src_var.0.to_string(), selected);
        self.foreach_seed.borrow_mut().replace(vec![seed]);
        let sub_rows = self.exec_operator(cx, op)?;
        self.foreach_seed.borrow_mut().take();
        Ok(sub_rows
            .first()
            .and_then(|sub_row| {
                sub_row
                    .get(out_alias.as_str())
                    .or_else(|| sub_row.get(src_var.0.as_str()))
                    .cloned()
            })
            .unwrap_or(Value::Null))
    }

    fn eval_gremlin_by_key(
        &self,
        cx: &mut ExecContext<'_>,
        row: &Row,
        value_alias: &Identifier,
        by_key: &str,
    ) -> Result<Value, GqlError> {
        if let Some(selected) = row.get(value_alias.as_str()) {
            if by_key.starts_with('~') {
                return Ok(self.resolve_prop_on_value(cx, by_key, selected));
            }
        }
        let ctx = self.build_filter_ctx(cx.graph, row)?;
        let prop_expr = Expr::Property(
            Box::new(Expr::Variable(value_alias.clone())),
            smol_str::SmolStr::new(by_key),
        );
        Ok(self
            .eval_with_storage(cx.graph, &prop_expr, row, &ctx)
            .unwrap_or(Value::Null))
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
            pattern_labels,
            label_map,
            result_alias,
            child,
            ..
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
        // Use or_insert to keep the FIRST label for each alias. When
        // Repeat remap causes two labels to share an alias, the earlier
        // binding is correct for initial row normalization.
        let mut reverse_map: std::collections::HashMap<&str, &str> =
            std::collections::HashMap::new();
        for (label, alias) in label_map {
            reverse_map
                .entry(alias.0.as_str())
                .or_insert(label.as_str());
        }

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

            for (pattern_idx, pattern) in patterns.iter().enumerate() {
                let mut next_bindings = Vec::new();

                // Build per-pattern reverse map from produced labels only.
                // This avoids extracting the seed label's value from the
                // pattern output when the seed was overwritten internally
                // (e.g. by a Repeat step).
                // Multiple labels can share the same alias (e.g. repeat().as("b")
                // maps "b" to the same alias as "a"), so we store Vec<label>.
                let p_labels = pattern_labels.get(pattern_idx);
                let p_reverse: Option<std::collections::HashMap<&str, Vec<&str>>> =
                    p_labels.map(|pl| {
                        let mut m: std::collections::HashMap<&str, Vec<&str>> =
                            std::collections::HashMap::new();
                        for (label, alias) in pl {
                            m.entry(alias.0.as_str()).or_default().push(label.as_str());
                        }
                        m
                    });

                for binding in &current_bindings {
                    cx.budget.check_or_err()?;
                    // Seed: the pattern expects internal alias names, so we
                    // need to provide the binding with both label and internal
                    // alias keys. Build a denormalized seed row.
                    // Avoid overwriting an alias already set for a different
                    // label (e.g. Repeat remap makes "a" and "b" share v_0;
                    // we keep the first label's value).
                    let mut seed = binding.clone();
                    let mut alias_owner: std::collections::HashMap<&str, &str> =
                        std::collections::HashMap::new();
                    for (label, alias) in label_map {
                        if let Some(v) = binding.get(label.as_str()) {
                            if let Some(prev_label) = alias_owner.get(alias.0.as_str()) {
                                if *prev_label != label.as_str() {
                                    continue;
                                }
                            }
                            alias_owner.insert(alias.0.as_str(), label.as_str());
                            seed.insert(alias.0.to_string(), v.clone());
                        }
                    }

                    self.foreach_seed.borrow_mut().replace(vec![seed]);
                    let pattern_rows = self.exec_operator(cx, pattern)?;
                    self.foreach_seed.borrow_mut().take();

                    // Normalize pattern results using per-pattern produced
                    // labels, then merge on shared label keys for consistency.
                    for p_row in pattern_rows {
                        let norm = if let Some(ref pr) = p_reverse {
                            let mut n = Row::new();
                            for (k, v) in &p_row {
                                if let Some(labels) = pr.get(k.as_str()) {
                                    // Emit all labels sharing this alias (e.g.
                                    // repeat().as("b") causes "a" and "b" to
                                    // share the same column alias).
                                    for label in labels {
                                        n.insert((*label).to_string(), v.clone());
                                    }
                                }
                            }
                            n
                        } else {
                            normalize(&p_row)
                        };
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
                // Insert label columns from binding.
                for (k, v) in &binding {
                    result_row.insert(k.clone(), v.clone());
                    map_entries
                        .entry(Box::from(k.as_str()))
                        .or_insert_with(|| v.clone());
                }
                // Set alias columns deterministically: iterate label_map
                // in order (pattern-1 entries first, then pattern-2, etc.)
                // so the FIRST label claiming an alias wins.  This avoids
                // non-deterministic HashMap iteration reordering the labels
                // when two labels share an alias (Repeat remap).
                let mut alias_set = std::collections::HashSet::new();
                for (label, alias) in label_map {
                    if alias_set.contains(alias.0.as_str()) {
                        continue;
                    }
                    if let Some(v) = binding.get(label.as_str()) {
                        alias_set.insert(alias.0.as_str());
                        result_row.insert(alias.0.to_string(), v.clone());
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
            by_traversals,
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

            for (idx, (src_var, output_key)) in keys.iter().enumerate() {
                let var_name = src_var.0.as_str();
                let by_traversal = by_traversals.get(idx).and_then(|entry| entry.as_ref());

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
                        // Repeat() uses synthetic pop_orig_* aliases to preserve
                        // the original binding for Pop.first when a label is
                        // rebound inside the loop body. Those shadow aliases
                        // must not leak into Pop.all/last/mixed materialization.
                        let mut vals: Vec<Value> = Vec::new();
                        for alias in stack.iter().filter(|alias| {
                            matches!(pop_mode, GremlinPop::First)
                                || !alias.0.as_str().starts_with("pop_orig_")
                        }) {
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
                            if let Some((prop_name, _real_var)) = self.split_prop_accessor(rest) {
                                if let Some(n_str) = prop_name.strip_prefix("~limit_local:") {
                                    let n = n_str.parse::<usize>().unwrap_or(0);
                                    match resolved {
                                        Value::List(items) => {
                                            Value::List(items.into_iter().take(n).collect())
                                        }
                                        other => other,
                                    }
                                } else if let Some(n_str) = prop_name.strip_prefix("~tail_local:") {
                                    let n = n_str.parse::<usize>().unwrap_or(0);
                                    match resolved {
                                        Value::List(items) => {
                                            let skip = items.len().saturating_sub(n);
                                            Value::List(items.into_iter().skip(skip).collect())
                                        }
                                        other => other,
                                    }
                                } else {
                                    self.resolve_prop_on_value(cx, prop_name, &resolved)
                                }
                            } else {
                                resolved
                            }
                        } else {
                            resolved
                        };
                        let resolved = if let Some(traversal) = by_traversal {
                            self.apply_select_by_traversal(cx, row, src_var, resolved, traversal)?
                        } else {
                            resolved
                        };
                        let empty_local = matches!(&resolved, Value::List(items) if items.is_empty())
                            || matches!(&resolved, Value::Map(map) if map.is_empty());
                        if !*productive && empty_local {
                            all_found = false;
                            break;
                        }
                        new_row.insert(output_key.clone(), resolved);
                        continue;
                    }
                }

                // Handle property-accessor pattern: __prop_PROPNAME__VARNAME
                if let Some(rest) = var_name.strip_prefix("__prop_") {
                    if let Some((prop_name, real_var)) = self.split_prop_accessor(rest) {
                        let val = row.get(real_var);
                        let selected = if let Some(v) = self.resolve_map_select_value(row, real_var)
                        {
                            Some(v)
                        } else {
                            val.cloned()
                        };
                        match selected.as_ref() {
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
                                    let v = if let Some(traversal) = by_traversal {
                                        self.apply_select_by_traversal(
                                            cx, row, src_var, v, traversal,
                                        )?
                                    } else {
                                        v
                                    };
                                    new_row.insert(output_key.clone(), v);
                                    continue;
                                }

                                // Handle tilde-prefixed transformation keys
                                // (e.g., ~unfold_values_fold:name, ~toupper:prop)
                                // via resolve_prop_on_value instead of raw
                                // property access.
                                if prop_name.starts_with('~') {
                                    let r = self.resolve_prop_on_value(cx, prop_name, node_val);
                                    let r = if let Some(traversal) = by_traversal {
                                        self.apply_select_by_traversal(
                                            cx, row, src_var, r, traversal,
                                        )?
                                    } else {
                                        r
                                    };
                                    if !*productive && matches!(r, Value::Null) {
                                        all_found = false;
                                        break;
                                    }
                                    new_row.insert(output_key.clone(), r);
                                    continue;
                                }

                                // Resolve property from the node/edge
                                let prop_expr = Expr::Property(
                                    Box::new(Expr::Variable(Identifier::new(real_var))),
                                    smol_str::SmolStr::new(prop_name),
                                );
                                match self.eval_with_storage(cx.graph, &prop_expr, row, &ctx) {
                                    Ok(v) => {
                                        let v = if let Some(traversal) = by_traversal {
                                            self.apply_select_by_traversal(
                                                cx, row, src_var, v, traversal,
                                            )?
                                        } else {
                                            v
                                        };
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
                                                        let val = if let Some(traversal) =
                                                            by_traversal
                                                        {
                                                            self.apply_select_by_traversal(
                                                                cx, row, src_var, val, traversal,
                                                            )?
                                                        } else {
                                                            val
                                                        };
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
                let val = if let Some(v) = self.resolve_map_select_value(row, var_name) {
                    Some(v)
                } else if let Some(v) = row.get(var_name) {
                    Some(v.clone())
                } else if let Some(v) = row.get(&format!("__gremlin_agg_{}", output_key)) {
                    // Fallback: aggregate sideeffect columns are stored
                    // with a __gremlin_agg_ prefix.
                    Some(v.clone())
                } else if let Some(v) = self
                    .gremlin_side_effects
                    .borrow()
                    .get(&format!("__gremlin_agg_{}", output_key))
                    .cloned()
                {
                    // Fallback: check the global side-effect store.
                    // Inside local(), the seed row doesn't carry the
                    // aggregate column, but the store accumulates it.
                    Some(v)
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
                        // Last-resort variable resolution via expression eval.
                        // .ok() is intentional: a missing variable here means all
                        // prior lookup paths failed, which is a valid "not found"
                        // in Gremlin select() semantics — not an error to propagate.
                        let expr = Expr::Variable(src_var.clone());
                        crate::executor::filter::eval_expr(&expr, &ctx).ok()
                    } else {
                        found
                    }
                };

                match val {
                    Some(v) => {
                        // Select-from-Map: when the resolved value is a Map
                        // containing the output key, extract the Map entry.
                        // This implements Gremlin's behavior where select(key)
                        // on a Map traverser acts as a Map key selector.
                        let v = if keys.len() == 1 {
                            if let Value::Map(ref m) = v {
                                m.get(output_key.as_str()).cloned().unwrap_or(v)
                            } else {
                                v
                            }
                        } else {
                            v
                        };
                        let v = if let Some(traversal) = by_traversal {
                            self.apply_select_by_traversal(cx, row, src_var, v, traversal)?
                        } else {
                            v
                        };
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
                            // Always update the compiler source alias so
                            // downstream steps operate on the selected value.
                            out_row.insert(src_var.0.to_string(), v.clone());
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
            key_is_identity,
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

        // Extract withSideEffect initial map from input rows so we can
        // merge its entries into the final result_map after traversal completes.
        // When group("a") follows withSideEffect("a", initialMap), the compiler
        // injects the initial map into rows as __side_effect_a via a Project operator.
        let se_initial_map: Option<IndexMap<Box<str>, Value>> = if let Some(key) = side_effect_key {
            let se_col = format!("__side_effect_{key}");
            let result = input.first().and_then(|row| {
                if let Some(Value::Map(m)) = row.get(&se_col) {
                    Some(m.clone())
                } else {
                    None
                }
            });
            result
        } else {
            None
        };

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
            // unless ProductiveByStrategy is active or the key is the identity
            // (traverser itself). When key_is_identity is true, the traverser IS
            // the key — a null traverser is a valid group key and must be counted.
            if matches!(key_val, Value::Null) && !*productive && !*key_is_identity {
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

        // Merge withSideEffect initial values into groups.
        // For keys already in groups (traversal matched): prepend initial values.
        // For keys only in initial map (no matching vertex): use initial values as-is.
        if let Some(init_map) = se_initial_map {
            for (k, v) in init_map {
                let init_vals: Vec<Value> = match v {
                    Value::List(items) => items,
                    other => vec![other],
                };
                let entry = groups.entry(k).or_default();
                // Prepend so initial values come before traversal values.
                let mut merged = init_vals;
                merged.append(entry);
                *entry = merged;
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
                        } else if values.len() == 1 {
                            // Single result from value traversal: unwrap unconditionally.
                            // This handles both scalar results (e.g., constant(1)) and
                            // list results (e.g., select(Column.values) returning a List).
                            // The value traversal already produced the final value.
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
                // Even with no input, the side-effect must remain accessible via
                // cap(). Preserve any previously accumulated named side-effect map
                // instead of overwriting it with an empty map on an exhausted
                // repeat() iteration.
                let map_val = if let Some(Value::Map(existing)) =
                    self.gremlin_side_effects.borrow().get(&agg_col).cloned()
                {
                    let mut merged = existing;
                    for (mk, mv) in result_map {
                        merged.insert(mk, mv);
                    }
                    Value::Map(merged)
                } else {
                    Value::Map(result_map)
                };
                let mut sentinel = Row::new();
                sentinel.insert(agg_col.clone(), map_val.clone());
                // Store in side-effect store for cap() retrieval
                self.gremlin_side_effects
                    .borrow_mut()
                    .insert(agg_col, map_val);
                if key == "x" {
                    append_gremlin_debug_log(&format!(
                        "[group:x] empty-input sentinel map-size={}",
                        match sentinel.get("__gremlin_agg_x") {
                            Some(Value::Map(m)) => m.len(),
                            _ => 0,
                        }
                    ));
                }
                return Ok(vec![sentinel]);
            }
            // Merge new counts with any existing aggregation carried from
            // a previous repeat iteration so counts accumulate.
            let map_val = {
                let mut merged = result_map;
                let has_row_existing = output.first().and_then(|f| f.get(&agg_col)).is_some();
                if has_row_existing {
                    // Row carries forward the accumulated map (repeat case).
                    if let Some(Value::Map(existing)) = output.first().and_then(|f| f.get(&agg_col))
                    {
                        for (ek, ev) in existing {
                            merge_gremlin_map_entry(&mut merged, ek, ev);
                        }
                    }
                } else if let Some(Value::Map(existing)) =
                    self.gremlin_side_effects.borrow().get(&agg_col).cloned()
                {
                    for (ek, ev) in existing {
                        merge_gremlin_map_entry(&mut merged, &ek, &ev);
                    }
                }
                Value::Map(merged)
            };
            // Store in side-effect store for cap() retrieval
            self.gremlin_side_effects
                .borrow_mut()
                .insert(agg_col.clone(), map_val.clone());
            if key == "x" {
                let sample_keys = match &map_val {
                    Value::Map(m) => m
                        .keys()
                        .take(6)
                        .map(|k| k.to_string())
                        .collect::<Vec<_>>()
                        .join(" | "),
                    _ => String::new(),
                };
                let sample_value = match &map_val {
                    Value::Map(m) => m
                        .values()
                        .next()
                        .map(|value| summarize_gremlin_value_shape(value, 3))
                        .unwrap_or_default(),
                    _ => String::new(),
                };
                append_gremlin_debug_log(&format!(
                    "[group:x] output_rows={} map-size={} sample={} value={}",
                    output.len(),
                    match &map_val {
                        Value::Map(m) => m.len(),
                        _ => 0,
                    },
                    sample_keys,
                    sample_value
                ));
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

    /// Execute `GremlinSample` — barrier sampling of the traverser stream using
    /// Gremlin's weighted roulette semantics.
    pub(crate) fn exec_gremlin_sample(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::GremlinSample {
            count,
            weight,
            seed,
            child,
        } = op
        else {
            debug_assert!(false, "expected GremlinSample operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected GremlinSample operator".into(),
            }));
        };

        let input = self.exec_operator(cx, child)?;
        let mut weighted_rows = Vec::with_capacity(input.len());
        for row in input {
            cx.budget.check_or_err()?;
            let row_weight = if let Some(weight_expr) = weight {
                let filter_ctx = self.build_filter_ctx(cx.graph, &row)?;
                match self.eval_with_storage(cx.graph, weight_expr, &row, &filter_ctx)? {
                    Value::Int(n) => n as f64,
                    Value::Float(f) => f,
                    Value::Null => continue,
                    other => {
                        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: "sample().by() numeric weight".into(),
                            actual: other.type_name().to_string(),
                        }))
                    }
                }
            } else {
                1.0
            };
            if row_weight.is_sign_negative() || !row_weight.is_finite() {
                continue;
            }
            weighted_rows.push((row, row_weight));
        }

        if weighted_rows.len() <= *count {
            return Ok(weighted_rows.into_iter().map(|(row, _)| row).collect());
        }

        let mut total_weight: f64 = weighted_rows.iter().map(|(_, weight)| *weight).sum();
        let seed = seed.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as i64
        });
        let mut rng = JavaCompatibleRandom::new(seed);
        let mut sampled = Vec::with_capacity(*count);

        while sampled.len() < *count && !weighted_rows.is_empty() {
            let mut running_total_weight = total_weight;
            let mut selected = None;
            for (idx, (_, row_weight)) in weighted_rows.iter().enumerate() {
                cx.budget.check_or_err()?;
                let draw = rng.next_double();
                if draw <= (*row_weight / running_total_weight.max(f64::MIN_POSITIVE)) {
                    selected = Some(idx);
                    break;
                }
                running_total_weight -= *row_weight;
            }
            if let Some(idx) = selected {
                let (row, row_weight) = weighted_rows.remove(idx);
                total_weight -= row_weight;
                sampled.push(row);
            } else {
                break;
            }
        }

        Ok(sampled)
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
                    // Produce a single row containing the list of [element, index] pairs.
                    // TinkerPop index() on a folded list returns a list of pairs so that
                    // subsequent unfold() yields each pair as a List value rather than
                    // unfolding the pairs themselves.
                    let pairs: Vec<Value> = items
                        .iter()
                        .enumerate()
                        .map(|(i, item)| Value::List(vec![item.clone(), Value::Int(i as i64)]))
                        .collect();
                    let mut row = input[0].clone();
                    row.insert(out_alias.0.clone(), Value::List(pairs));
                    out.push(row);
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

    /// Execute `GremlinBarrier` — bulk synchronization with optional sack normalization.
    pub(crate) fn exec_gremlin_barrier(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::GremlinBarrier {
            child,
            current_alias,
            out_alias,
            norm_sack,
            sack_merge_op,
            with_bulk,
        } = op
        else {
            debug_assert!(false, "expected GremlinBarrier operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected GremlinBarrier operator".into(),
            }));
        };

        let input = self.exec_operator(cx, child)?;

        // If no merge and not normSack, pass-through
        if !*norm_sack && sack_merge_op.is_none() {
            let mut out = Vec::with_capacity(input.len());
            for mut row in input {
                if let Some(val) = row.get(current_alias.0.as_str()).cloned() {
                    row.insert(out_alias.0.clone(), val);
                }
                out.push(row);
            }
            return Ok(out);
        }

        // normSack: divide each __sack by sum of all sacks
        if *norm_sack {
            let sum: f64 = input
                .iter()
                .filter_map(|row| {
                    row.get("__sack").and_then(|v| match v {
                        Value::Float(f) => Some(*f),
                        Value::Int(i) => Some(*i as f64),
                        _ => None,
                    })
                })
                .sum();

            let count = input.len();
            let mut out = Vec::with_capacity(count);
            for mut row in input {
                if let Some(sack_val) = row.get("__sack").cloned() {
                    let sack_f = match &sack_val {
                        Value::Float(f) => *f,
                        Value::Int(i) => *i as f64,
                        _ => 0.0,
                    };
                    let normalized = if sum == 0.0 || sum.is_infinite() {
                        1.0 / count as f64
                    } else {
                        sack_f / sum
                    };
                    row.insert("__sack".to_string(), Value::Float(normalized));
                }
                if let Some(val) = row.get(current_alias.0.as_str()).cloned() {
                    row.insert(out_alias.0.clone(), val);
                }
                out.push(row);
            }
            return Ok(out);
        }

        // Merge mode: group by current_alias value, merge sacks
        if let Some(merge_op) = sack_merge_op {
            let mut groups: IndexMap<String, (Row, f64, i64)> = IndexMap::new();
            for row in &input {
                let key = row
                    .get(current_alias.0.as_str())
                    .map(|v| format!("{v:?}"))
                    .unwrap_or_default();
                let sack_f = row
                    .get("__sack")
                    .and_then(|v| match v {
                        Value::Float(f) => Some(*f),
                        Value::Int(i) => Some(*i as f64),
                        _ => None,
                    })
                    .unwrap_or(0.0);
                let entry = groups.entry(key).or_insert_with(|| (row.clone(), 0.0, 0));
                entry.2 += 1;
                match merge_op.as_str() {
                    "sum" => entry.1 += sack_f,
                    "mult" => {
                        if entry.2 == 1 {
                            entry.1 = sack_f;
                        } else {
                            entry.1 *= sack_f;
                        }
                    }
                    "min" => {
                        if entry.2 == 1 || sack_f < entry.1 {
                            entry.1 = sack_f;
                        }
                    }
                    "max" => {
                        if entry.2 == 1 || sack_f > entry.1 {
                            entry.1 = sack_f;
                        }
                    }
                    "minus" => {
                        if entry.2 == 1 {
                            entry.1 = sack_f;
                        } else {
                            entry.1 -= sack_f;
                        }
                    }
                    "div" => {
                        if entry.2 == 1 {
                            entry.1 = sack_f;
                        } else if sack_f != 0.0 {
                            entry.1 /= sack_f;
                        }
                    }
                    _ => {
                        entry.1 = sack_f;
                    }
                }
            }

            let mut out = Vec::new();
            for (_key, (mut row, merged_sack, bulk)) in groups {
                row.insert("__sack".to_string(), Value::Float(merged_sack));
                if let Some(val) = row.get(current_alias.0.as_str()).cloned() {
                    row.insert(out_alias.0.clone(), val);
                }
                if *with_bulk {
                    for _ in 0..bulk {
                        out.push(row.clone());
                    }
                } else {
                    out.push(row);
                }
            }
            return Ok(out);
        }

        // Fallback: pass-through
        let mut out = Vec::with_capacity(input.len());
        for mut row in input {
            if let Some(val) = row.get(current_alias.0.as_str()).cloned() {
                row.insert(out_alias.0.clone(), val);
            }
            out.push(row);
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
                .map(|item| value_to_map_key(item).to_string())
                .collect();
            format!("l[{}]", parts.join(",")).into_boxed_str()
        }
        Value::Map(m) => {
            let parts: Vec<String> = m
                .iter()
                .map(|(k, v)| format!("{k}={}", value_to_map_key(v)))
                .collect();
            format!("{{{}}}", parts.join(", ")).into_boxed_str()
        }
        _ => format!("{v:?}").into_boxed_str(),
    }
}

fn merge_gremlin_map_entry(map: &mut IndexMap<Box<str>, Value>, key: &str, incoming: &Value) {
    match map.entry(Box::from(key)) {
        indexmap::map::Entry::Vacant(entry) => {
            entry.insert(incoming.clone());
        }
        indexmap::map::Entry::Occupied(mut entry) => match (entry.get().clone(), incoming) {
            (Value::Int(a), Value::Int(b)) => {
                entry.insert(Value::Int(a + b));
            }
            (Value::Float(a), Value::Float(b)) => {
                entry.insert(Value::Float(a + b));
            }
            (Value::List(mut a), Value::List(b)) => {
                a.extend(b.iter().cloned());
                entry.insert(Value::List(a));
            }
            (_, other) => {
                entry.insert(other.clone());
            }
        },
    }
}

fn summarize_gremlin_value_shape(value: &Value, depth: usize) -> String {
    if depth == 0 {
        return "...".to_string();
    }
    match value {
        Value::List(items) => {
            let sample = items
                .iter()
                .take(3)
                .map(|item| summarize_gremlin_value_shape(item, depth - 1))
                .collect::<Vec<_>>()
                .join(", ");
            format!("list(len={}, sample=[{}])", items.len(), sample)
        }
        Value::Map(entries) => {
            let sample = entries
                .iter()
                .take(2)
                .map(|(k, v)| format!("{k}:{}", summarize_gremlin_value_shape(v, depth - 1)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("map(len={}, sample={{{}}})", entries.len(), sample)
        }
        Value::Node(nid) => format!("node({})", nid.0),
        Value::Edge(eid) => format!("edge({})", eid.0),
        Value::Str(s) => format!("str({s})"),
        Value::Int(i) => format!("int({i})"),
        Value::Float(f) => format!("float({f})"),
        Value::Bool(b) => format!("bool({b})"),
        Value::Null => "null".to_string(),
        other => format!("{other:?}"),
    }
}

fn append_gremlin_debug_log(line: &str) {
    use std::io::Write;

    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/sd-gremlin-debug.log")
    {
        let _ = writeln!(file, "{line}");
    }
}

fn eval_select_math_expr(expr: &str, current: &Value) -> Value {
    fn as_f64(v: &Value) -> Option<f64> {
        match v {
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }

    let Some(current_num) = as_f64(current) else {
        return current.clone();
    };

    let compact: String = expr.chars().filter(|c| !c.is_whitespace()).collect();
    match compact.as_str() {
        "_+_" => Value::Float(current_num + current_num),
        "_-_" => Value::Float(0.0),
        "_*_" => Value::Float(current_num * current_num),
        "_/_" => Value::Float(if current_num != 0.0 { 1.0 } else { f64::NAN }),
        "_%_" => Value::Float(if current_num != 0.0 {
            current_num % current_num
        } else {
            f64::NAN
        }),
        _ => current.clone(),
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

fn parse_gremlin_option_args(args: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    for ch in args.chars() {
        match ch {
            '\'' if !in_double => {
                in_single = !in_single;
                current.push(ch);
            }
            '"' if !in_single => {
                in_double = !in_double;
                current.push(ch);
            }
            ',' if !in_single && !in_double => {
                parts.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    if !current.trim().is_empty() {
        parts.push(current.trim().to_string());
    }
    parts
}

fn trim_gremlin_option_string(s: &str) -> &str {
    s.trim().trim_matches(|c| c == '"' || c == '\'')
}

fn parse_gremlin_option_value(raw: &str) -> Value {
    let trimmed = trim_gremlin_option_string(raw);
    if raw.trim().starts_with('"') || raw.trim().starts_with('\'') {
        return Value::Str(trimmed.into());
    }
    if let Ok(i) = trimmed.parse::<i64>() {
        return Value::Int(i);
    }
    if let Ok(f) = trimmed.parse::<f64>() {
        return Value::Float(f);
    }
    match trimmed {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        "null" => Value::Null,
        _ => Value::Str(trimmed.into()),
    }
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

/// Returns `true` if the operator tree contains a barrier step (Sort, Aggregate,
/// Distinct) that must see all input rows before producing output. When a repeat
/// body contains barrier steps, batched seed processing must be disabled because
/// each batch would run the barrier independently, producing incorrect results.
fn operator_contains_barrier(op: &Operator) -> bool {
    match op {
        Operator::Sort { .. }
        | Operator::Aggregate { .. }
        | Operator::Distinct { .. }
        | Operator::GremlinBarrier { .. } => true,
        Operator::Project { child, .. }
        | Operator::Filter { child, .. }
        | Operator::Skip { child, .. }
        | Operator::Expand { child, .. }
        | Operator::GremlinSideEffect { child, .. }
        | Operator::Coalesce { child, .. }
        | Operator::GremlinSimplePath { child, .. }
        | Operator::GremlinCyclicPath { child, .. }
        | Operator::GremlinGroup { child, .. }
        | Operator::GremlinAggregate { child, .. } => operator_contains_barrier(child),
        // gremlin_tail limits need all rows → treat as barrier.
        Operator::Limit { count, child } => {
            if matches!(count, Expr::FunctionCall { name, .. } if name.as_str() == "gremlin_tail") {
                true
            } else {
                operator_contains_barrier(child)
            }
        }
        Operator::Apply { outer, inner } => {
            operator_contains_barrier(outer) || operator_contains_barrier(inner)
        }
        Operator::LeftOuterApply {
            preserved,
            optional_pattern,
        } => operator_contains_barrier(preserved) || operator_contains_barrier(optional_pattern),
        Operator::Union { left, right, .. } => {
            operator_contains_barrier(left) || operator_contains_barrier(right)
        }
        Operator::Repeat { body, .. } => operator_contains_barrier(body),
        Operator::Choose {
            if_true, if_false, ..
        } => {
            operator_contains_barrier(if_true)
                || if_false
                    .as_ref()
                    .is_some_and(|f| operator_contains_barrier(f))
        }
        _ => false,
    }
}

/// Extract direction, rel_types, src_alias, dst_alias from a simple Expand
/// whose child is SingleRow.
fn extract_simple_expand(
    op: &Operator,
) -> Option<(Direction, Vec<crate::ast::RelType>, Identifier, Identifier)> {
    match op {
        Operator::Expand {
            direction,
            rel_types,
            src_alias,
            dst_alias,
            child,
            ..
        } if matches!(child.as_ref(), Operator::SingleRow) => Some((
            *direction,
            rel_types.clone(),
            src_alias.clone(),
            dst_alias.clone(),
        )),
        Operator::Project { child, .. } => extract_simple_expand(child),
        _ => None,
    }
}

/// Returns true if the operator tree contains a GremlinSimplePath filter.
fn body_contains_simple_path(op: &Operator) -> bool {
    match op {
        Operator::GremlinSimplePath { .. } => true,
        Operator::Expand { child, .. }
        | Operator::Project { child, .. }
        | Operator::Filter { child, .. }
        | Operator::GremlinCyclicPath { child, .. } => body_contains_simple_path(child),
        _ => false,
    }
}

/// Expand rows that carry `__gremlin_bulk` into individual rows.
///
/// The vertex-counting fast path collapses identical destination nodes into
/// a single row with a bulk count.  Downstream steps (tail, path, select,
/// group-by-count, etc.) expect individual traverser rows, so we expand
/// here at the fast-path boundary.
fn expand_bulk_rows(rows: Vec<Row>) -> Vec<Row> {
    let mut out = Vec::with_capacity(rows.len());
    for mut row in rows {
        if let Some(Value::Int(bulk)) = row.remove("__gremlin_bulk") {
            let n = (bulk as usize).max(1);
            for _ in 0..n {
                out.push(row.clone());
            }
        } else {
            out.push(row);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use indexmap::IndexMap;

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
        let mut pa = IndexMap::new();
        pa.insert("name".to_string(), Value::Str("A".into()));
        let a = store.create_node("g", vec!["N".to_string()], pa).unwrap();
        let mut pb = IndexMap::new();
        pb.insert("name".to_string(), Value::Str("B".into()));
        let b = store.create_node("g", vec!["N".to_string()], pb).unwrap();
        let mut pc = IndexMap::new();
        pc.insert("name".to_string(), Value::Str("C".into()));
        let _c = store.create_node("g", vec!["N".to_string()], pc).unwrap();
        store
            .create_edge("g", a, b, "NEXT".to_string(), IndexMap::new())
            .unwrap();
        store
            .create_edge("g", b, _c, "NEXT".to_string(), IndexMap::new())
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
            loop_label: None,
            body_path_aliases: vec![],
            pop_origins: vec![],
            needs_path: false,
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
            loop_label: None,
            body_path_aliases: vec![],
            pop_origins: vec![],
            needs_path: false,
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
        let mut py = IndexMap::new();
        py.insert("tag".to_string(), Value::Str("yes".into()));
        store.create_node("g", vec!["Yes".to_string()], py).unwrap();
        let mut pn = IndexMap::new();
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
            .create_node("g", vec!["A".to_string()], IndexMap::new())
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

    #[test]
    fn choose_map_matches_traversal_style_option_tokens() {
        let store = make_store();
        store
            .create_node("g", vec!["Software".to_string()], IndexMap::new())
            .unwrap();
        let mut props = IndexMap::new();
        props.insert("name".to_string(), Value::Str("vadas".into()));
        store
            .create_node("g", vec!["Person".to_string()], props)
            .unwrap();

        let executor = QueryExecutor::new(&store).with_gremlin_mode();
        let plan = Operator::ChooseMap {
            selector: Box::new(Operator::SingleRow),
            selector_alias: Identifier::new("n"),
            options: vec![
                (
                    Some(Value::Str(r#"__.hasLabel("Software")"#.into())),
                    Box::new(Operator::Project {
                        items: vec![(
                            Expr::Str("software".into()),
                            Some(Identifier::new("result")),
                        )],
                        child: Box::new(Operator::SingleRow),
                    }),
                    Some(Identifier::new("result")),
                ),
                (
                    Some(Value::Str(r#"__.has("name","vadas")"#.into())),
                    Box::new(Operator::Project {
                        items: vec![(Expr::Str("vadas".into()), Some(Identifier::new("result")))],
                        child: Box::new(Operator::SingleRow),
                    }),
                    Some(Identifier::new("result")),
                ),
            ],
            child: Box::new(Operator::NodeScan {
                label: None,
                alias: Identifier::new("n"),
            }),
            output_alias: Some(Identifier::new("result")),
        };

        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        let mut values: Vec<_> = result
            .rows
            .iter()
            .filter_map(|row| row.get("result").cloned())
            .collect();
        values.sort_by_key(|v| format!("{v:?}"));
        assert_eq!(values.len(), 2);
        assert!(values.contains(&Value::Str("software".into())));
        assert!(values.contains(&Value::Str("vadas".into())));
    }

    // ── Coalesce tests ───────────────────────────────────────────────────────

    #[test]
    fn coalesce_returns_first_nonempty() {
        let store = make_store();
        store
            .create_node("g", vec!["A".to_string()], IndexMap::new())
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
            .create_node("g", vec!["A".to_string()], IndexMap::new())
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
        let mut props = IndexMap::new();
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
                set_semantics: false,
                assign_semantics: false,
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
                set_semantics: false,
                assign_semantics: false,
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
            set_semantics: false,
            assign_semantics: false,
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
                set_semantics: false,
                assign_semantics: false,
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
        let mut props = IndexMap::new();
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
                set_semantics: false,
                assign_semantics: false,
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
            .create_node("g", vec!["A".to_string()], IndexMap::new())
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
            .create_node("g", vec!["A".to_string()], IndexMap::new())
            .unwrap();
        store
            .create_node("g", vec!["A".to_string()], IndexMap::new())
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
            pattern_labels: vec![],
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
            pattern_labels: vec![],
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
            pattern_labels: vec![],
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
            pattern_labels: vec![],
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
            pattern_labels: vec![],
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
