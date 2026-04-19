//! Gremlin executor: `repeat` step implementations.
//!
//! Submodules:
//! - [`iteration`]: batched per-iteration body execution + agg-map merging.
//! - [`emit`]: emit-predicate filtering helper.
//! - [`counting`]: vertex-counting fast path for `repeat(Expand).times(N)`.

mod counting;
mod emit;
mod iteration;

use crate::error::{ExecutionError, GqlError};
use crate::planner::Operator;
use crate::types::Value;

use crate::executor::engine::{ExecContext, QueryExecutor, Row};

use super::helpers::{expand_bulk_rows, operator_contains_barrier, operator_contains_distinct};

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

            // Execute the repeat body with batched streaming.
            let (mut next, iteration_agg_maps) = self.repeat_exec_body_batched(
                cx,
                body,
                &current,
                iteration,
                seed_key,
                remap,
                body_path_aliases,
                &generic_counter_key,
                labeled_counter_key.as_deref(),
                effective_batch,
            )?;

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
}
