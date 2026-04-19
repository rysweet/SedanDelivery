//! Gremlin executor: `repeat` step implementations.

use indexmap::IndexMap;

use crate::error::{ExecutionError, GqlError};
use crate::planner::Operator;
use crate::storage::NodeId;
use crate::types::Value;

use crate::executor::engine::{ExecContext, QueryExecutor, Row};

#[allow(unused_imports)]
use super::helpers::*;

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
                    let entries = crate::executor::engine::helpers::get_adj_entries(
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
                    let entries = crate::executor::engine::helpers::get_adj_entries(
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
}
