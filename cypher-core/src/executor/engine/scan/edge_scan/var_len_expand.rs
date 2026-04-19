//! `VarLenExpand` operator: variable-length BFS traversal.

use std::collections::{HashSet, VecDeque};

use crate::error::{ExecutionError, GqlError};
use crate::planner::Operator;
use crate::storage::{EdgeId, NodeId};
use crate::types::Value;

use crate::executor::engine::helpers::{extract_node_id, get_adj_entries, undefined_var};
use crate::executor::engine::{BfsArena, ExecContext, QueryExecutor, Row};

impl<'a> QueryExecutor<'a> {
    pub(crate) fn exec_var_len_expand(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::VarLenExpand {
            direction,
            rel_types,
            min,
            max,
            src_alias,
            path_alias,
            path_mode,
            dst_alias,
            child,
            rel_props,
        } = op
        else {
            debug_assert!(false, "expected VarLenExpand operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected VarLenExpand operator".into(),
            }));
        };
        let input = self.exec_operator(cx, child)?;
        let type_strs: Vec<&str> = rel_types.iter().map(|r| r.as_str()).collect();
        let mut out = Vec::with_capacity(input.len());

        for row in input {
            cx.budget.check_or_err()?;
            let src_val = row
                .get(src_alias.0.as_str())
                .ok_or_else(|| undefined_var(&src_alias.0))?;
            let src_id = extract_node_id(src_val, &src_alias.0)?;

            // Arena-based BFS: instead of cloning Vec<NodeId>, Vec<EdgeId>, and
            // HashSet<EdgeId> at every expansion step (O(depth) per step), we use
            // a parent-pointer arena where each step is O(1).  Paths are
            // reconstructed only when yielding output rows.
            let (mut arena, root_idx) = BfsArena::new(src_id);

            // BFS queue entries: (current_node_id, arena_index, depth)
            let mut queue: VecDeque<(NodeId, usize, u32)> = VecDeque::new();

            // Collect edges already used in earlier steps of this named path.
            // This enforces cross-operator relationship isomorphism:
            // - Direct Value::Edge in the row (from regular Expand steps, incl. pre-bound r)
            //   should not be re-traversed.
            // - Value::Path and Value::List([Edge]) from __path_rel_* (VarLen internal storage)
            //   also shouldn't be re-traversed.
            // We intentionally exclude user-defined list variables (like `rs` from WITH)
            // that contain edges intended to be FOLLOWED by this VarLen, not avoided.
            // Heuristic: collect Value::Edge (direct edge vars) and __path_rel_* (VarLen storage).
            let prior_path_edges: HashSet<EdgeId> = row
                .iter()
                .flat_map(|(k, v)| match v {
                    Value::Edge(e) => vec![*e],
                    Value::List(items) if k.starts_with("__path_rel_") => items
                        .iter()
                        .filter_map(|i| {
                            if let Value::Edge(e) = i {
                                Some(*e)
                            } else {
                                None
                            }
                        })
                        .collect(),
                    Value::Path { edges, .. } if k.starts_with("__path_rel_") => edges.clone(),
                    _ => vec![],
                })
                .collect();
            queue.push_back((src_id, root_idx, 0));

            // Scratch row for edge property constraint evaluation -- built once
            // from the base `row` and reused across all candidate edges instead
            // of cloning the entire row per candidate.
            let mut edge_scratch: Option<Row> = None;

            while let Some((current, arena_idx, depth)) = queue.pop_front() {
                cx.budget.check_or_err()?;

                // Yield this path if depth is within [min, max]
                if depth >= *min && depth <= *max {
                    let (path_nodes, path_edges) = arena.reconstruct_path(arena_idx);
                    let end_node = *path_nodes.last().ok_or_else(|| {
                        GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: "non-empty path".to_string(),
                            actual: "empty path_nodes".to_string(),
                        })
                    })?;
                    let mut new_row = row.clone();
                    new_row.insert(dst_alias.0.clone(), Value::Node(end_node));
                    if let Some(pa) = path_alias {
                        if *path_mode {
                            // Named path mode: store full path for AssemblePath
                            new_row.insert(
                                pa.0.clone(),
                                Value::Path {
                                    nodes: path_nodes,
                                    edges: path_edges,
                                },
                            );
                        } else {
                            // Relationship variable mode: store list of edges
                            new_row.insert(
                                pa.0.clone(),
                                Value::List(path_edges.iter().map(|e| Value::Edge(*e)).collect()),
                            );
                        }
                    }
                    cx.budget.record_row();
                    out.push(new_row);
                }

                // Expand further if depth < max
                if depth < *max {
                    let adj_entries =
                        get_adj_entries(self.store, cx.graph, current, direction, &type_strs)?;
                    for adj in adj_entries {
                        cx.budget.check_or_err()?;
                        // Per openCypher spec: edge isomorphism is enforced (same
                        // relationship cannot appear twice in one path), but node
                        // repetition is allowed. This prevents infinite loops.
                        // Also check edges from prior steps in this named path.
                        if arena.path_contains_edge(arena_idx, adj.edge_id)
                            || prior_path_edges.contains(&adj.edge_id)
                        {
                            continue;
                        }
                        // Check inline property constraints on the traversed edge.
                        // Instead of cloning the entire row per candidate edge, we
                        // build a scratch row once and update only the edge entry.
                        if let Some(pred) = rel_props {
                            let scratch = edge_scratch.get_or_insert_with(|| {
                                let mut s = row.clone();
                                s.insert("__varlen_edge__".to_string(), Value::Edge(adj.edge_id));
                                s
                            });
                            // Update just the edge entry for this candidate
                            scratch.insert("__varlen_edge__".to_string(), Value::Edge(adj.edge_id));
                            let ctx = self.build_filter_ctx(cx.graph, scratch)?;
                            match self.eval_with_storage(cx.graph, pred, scratch, &ctx) {
                                Ok(Value::Bool(true)) => {}
                                _ => continue, // edge doesn't satisfy constraints
                            }
                        }
                        let child_idx = arena.push(adj.neighbor_id, adj.edge_id, arena_idx);
                        queue.push_back((adj.neighbor_id, child_idx, depth + 1));
                    }
                }
            }
        }
        Ok(out)
    }
}
