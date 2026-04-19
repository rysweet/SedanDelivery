//! Per-iteration body execution for the `repeat` step.
//!
//! Streams seeds into the body operator in fixed-size batches to cap peak
//! memory on exponential traversal patterns and merges per-iteration
//! side-effect aggregate maps emitted by the body.

use std::collections::HashMap;

use indexmap::IndexMap;

use crate::error::GqlError;
use crate::executor::engine::{ExecContext, QueryExecutor, Row};
use crate::planner::Operator;
use crate::types::Value;

use super::super::helpers::merge_gremlin_map_entry;

/// Type returned by the batched-body executor: the next iteration's traverser
/// rows, plus the merged side-effect aggregate maps emitted by the body.
pub(super) type IterationOutput = (Vec<Row>, HashMap<String, IndexMap<Box<str>, Value>>);

impl<'a> QueryExecutor<'a> {
    /// Execute the repeat body with batched streaming: process seeds in
    /// chunks of `effective_batch` to cap peak memory. Each batch is
    /// independently fed to the body operator and results are merged.
    ///
    /// Returns the next iteration's traverser rows plus the merged
    /// side-effect aggregate maps emitted by the body.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn repeat_exec_body_batched(
        &self,
        cx: &mut ExecContext<'_>,
        body: &Operator,
        current: &[Row],
        iteration: usize,
        seed_key: Option<&str>,
        remap: &Option<(String, String)>,
        body_path_aliases: &[String],
        generic_counter_key: &str,
        labeled_counter_key: Option<&str>,
        effective_batch: usize,
    ) -> Result<IterationOutput, GqlError> {
        const BATCH_SIZE: usize = 10_000;

        let mut next: Vec<Row> = Vec::new();
        let mut iteration_agg_maps: HashMap<String, IndexMap<Box<str>, Value>> = HashMap::new();

        for batch_start in (0..current.len()).step_by(effective_batch) {
            cx.budget.check_or_err()?;
            let batch_end = (batch_start + BATCH_SIZE).min(current.len());
            let batch = &current[batch_start..batch_end];

            // Prepare seed rows for this batch.
            let mut seeds: Vec<Row> = Vec::with_capacity(batch.len());
            for row in batch {
                let mut seed_row = row.clone();
                seed_row.insert(
                    generic_counter_key.to_string(),
                    Value::Int(iteration as i64),
                );
                if let Some(lk) = labeled_counter_key {
                    seed_row.insert(lk.to_string(), Value::Int(iteration as i64));
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

        Ok((next, iteration_agg_maps))
    }
}
