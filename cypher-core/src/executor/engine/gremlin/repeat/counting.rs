//! Vertex-counting fast path for `repeat(Expand).times(N)`.
//!
//! Collapses traversers by destination `NodeId` and accumulates a bulk
//! count instead of materializing duplicate rows. Used when path tracking
//! is not required and there are no per-traverser side effects.

use indexmap::IndexMap;

use crate::error::GqlError;
use crate::planner::Operator;
use crate::storage::NodeId;
use crate::types::Value;

use crate::executor::engine::{ExecContext, QueryExecutor, Row};

use super::super::helpers::{body_contains_simple_path, extract_simple_expand};

impl<'a> QueryExecutor<'a> {
    /// Vertex-counting fast path for `repeat(Expand).times(N)`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_repeat_counting(
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
