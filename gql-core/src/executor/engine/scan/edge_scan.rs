//! Edge traversal operators: Expand, VarLenExpand, ShortestPath.

use std::collections::{HashSet, VecDeque};

use crate::ast::Direction;
use crate::error::{ExecutionError, GqlError};
use crate::planner::Operator;
use crate::storage::{EdgeId, NodeId};
use crate::types::Value;

use crate::executor::engine::helpers::{extract_node_id, get_adj_entries, undefined_var};
use crate::executor::engine::{BfsArena, ExecContext, QueryExecutor, Row};

impl<'a> QueryExecutor<'a> {
    pub(crate) fn exec_expand(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Expand {
            direction,
            rel_types,
            src_alias,
            rel_alias,
            dst_alias,
            child,
        } = op
        else {
            debug_assert!(false, "expected Expand operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected Expand operator".into(),
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

            let adj_entries = match direction {
                Direction::Outgoing => {
                    self.store.out_edges_by_type(cx.graph, src_id, &type_strs)?
                }
                Direction::Incoming => self.store.in_edges_by_type(cx.graph, src_id, &type_strs)?,
                Direction::Undirected => {
                    let mut out_entries =
                        self.store.out_edges_by_type(cx.graph, src_id, &type_strs)?;
                    let in_entries = self.store.in_edges_by_type(cx.graph, src_id, &type_strs)?;
                    // For both()/bothE(), concatenate outgoing and incoming
                    // edges. Self-loop edges intentionally appear twice (once
                    // from each direction), matching Gremlin semantics.
                    out_entries.extend(in_entries);
                    out_entries
                }
            };

            // Hoist outer_edge_vars construction to the per-row level.
            // This HashSet only depends on the row (not on individual adjacency
            // entries), so building it once per row avoids redundant allocations.
            let current_alias = rel_alias.as_ref().map(|ra| ra.0.as_str());
            let outer_edge_vars: HashSet<&str> = match row.get("__outer_edge_vars__") {
                Some(Value::List(vs)) => vs
                    .iter()
                    .filter_map(|v| {
                        if let Value::Str(s) = v {
                            Some(s.as_ref())
                        } else {
                            None
                        }
                    })
                    .collect(),
                _ => HashSet::new(),
            };

            for adj in adj_entries {
                cx.budget.check_or_err()?;
                // If dst_alias is already bound in the row (cyclic patterns
                // like (a)-->()-->(a)), only keep the result if the bound
                // value matches the expansion result.
                // Skip this check for anonymous variables: they may be
                // inherited from an outer scope (e.g. pattern comprehension)
                // and should not constrain the inner expansion.
                let dst_val = Value::Node(adj.neighbor_id);
                if !dst_alias.0.starts_with("__anon") {
                    if let Some(existing) = row.get(dst_alias.0.as_str()) {
                        if *existing != dst_val {
                            continue;
                        }
                    }
                }
                // Same for rel_alias: if already bound, values must match.
                if let Some(ra) = rel_alias {
                    let rel_val = Value::Edge(adj.edge_id);
                    if let Some(existing) = row.get(ra.0.as_str()) {
                        if *existing != rel_val {
                            continue;
                        }
                    }
                }
                // Edge isomorphism: per openCypher spec, each relationship in a
                // pattern can only be bound once in any single path traversal.
                // Skip this edge if it is already used by a DIFFERENT rel in the row
                // (exclude the current rel_alias, which may legitimately re-bind it).
                // Note: named path variables (Value::Path) are NOT checked here because
                // edges from paths in previous MATCH clauses can be reused independently.
                // Also exclude outer-scope edge variables (from previous WITH/MATCH scopes).
                //
                // In Gremlin mode, edge isomorphism is disabled — Gremlin traversals
                // allow revisiting the same edge from different directions.
                let eid = adj.edge_id;
                let edge_already_used = if self.gremlin_mode {
                    false
                } else {
                    row.iter().any(|(key, v)| {
                        // Don't count the current alias as "already used"
                        if Some(key.as_str()) == current_alias {
                            return false;
                        }
                        // Don't count outer-scope edges (from previous MATCH clauses)
                        if outer_edge_vars.contains(key.as_str()) {
                            return false;
                        }
                        match v {
                            Value::Edge(e) => *e == eid,
                            Value::List(items) => items
                                .iter()
                                .any(|item| matches!(item, Value::Edge(e) if *e == eid)),
                            // Named path variables are NOT subject to edge isomorphism
                            // across MATCH clauses — only relationship variables are.
                            Value::Path { .. } => false,
                            _ => false,
                        }
                    })
                };
                if edge_already_used {
                    continue;
                }
                let mut new_row = row.clone();
                if let Some(ra) = rel_alias {
                    new_row.insert(ra.0.clone(), Value::Edge(adj.edge_id));
                }
                new_row.insert(dst_alias.0.clone(), dst_val);
                cx.budget.record_row();
                out.push(new_row);
            }
        }
        Ok(out)
    }

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

    pub(crate) fn exec_shortest_path(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::ShortestPath {
            src_alias,
            dst_alias,
            rel_types,
            direction,
            min: _min,
            max,
            path_alias,
            all_paths,
        } = op
        else {
            debug_assert!(false, "expected ShortestPath operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected ShortestPath operator".into(),
            }));
        };
        // ShortestPath requires both src and dst to be bound in outer context.
        // For now, work from outer_rows or require them to be bound.
        let type_strs: Vec<&str> = rel_types.iter().map(|r| r.as_str()).collect();
        let mut out = Vec::new();

        // If there are outer rows, use them. Otherwise create an empty row.
        let context_rows = if cx.outer_rows.is_empty() {
            vec![Row::new()]
        } else {
            cx.outer_rows.to_vec()
        };

        for row in &context_rows {
            cx.budget.check_or_err()?;
            let src_val = row
                .get(src_alias.0.as_str())
                .ok_or_else(|| undefined_var(&src_alias.0))?;
            let src_id = extract_node_id(src_val, &src_alias.0)?;

            let dst_val = row
                .get(dst_alias.0.as_str())
                .ok_or_else(|| undefined_var(&dst_alias.0))?;
            let dst_id = extract_node_id(dst_val, &dst_alias.0)?;

            // Arena-based BFS from src to dst.  Instead of cloning
            // Vec<NodeId> and Vec<EdgeId> per queue entry (O(depth) per step),
            // we use a parent-pointer arena -- O(1) per step.
            let (mut arena, root_idx) = BfsArena::new(src_id);

            // Queue: (current_node, arena_index, depth)
            let mut queue: VecDeque<(NodeId, usize, u32)> = VecDeque::new();
            queue.push_back((src_id, root_idx, 0));
            let mut visited = HashSet::new();
            visited.insert(src_id);

            // Store arena indices of found paths; reconstruct only at the end.
            let mut found_indices: Vec<usize> = Vec::new();
            let mut shortest_len: Option<u32> = None;

            while let Some((current, arena_idx, depth)) = queue.pop_front() {
                cx.budget.check_or_err()?;

                // If we already found shortest paths and current depth exceeds
                // that length, stop (BFS guarantees depth ordering).
                if let Some(sl) = shortest_len {
                    if depth > sl {
                        break;
                    }
                }

                // Check if we reached the destination
                if current == dst_id && depth > 0 {
                    found_indices.push(arena_idx);
                    shortest_len = Some(depth);
                    if !*all_paths {
                        // shortestPath: just need the first one
                        break;
                    }
                    // For allShortestPaths, continue at same depth level
                    continue;
                }

                // Don't expand beyond max depth
                if depth >= *max {
                    continue;
                }

                let adj_entries =
                    get_adj_entries(self.store, cx.graph, current, direction, &type_strs)?;
                for adj in adj_entries {
                    cx.budget.check_or_err()?;
                    // For shortest path BFS, we track visited globally
                    // but allow reaching dst through different paths at same depth.
                    let is_dst = adj.neighbor_id == dst_id;
                    if !is_dst && visited.contains(&adj.neighbor_id) {
                        continue;
                    }
                    if !is_dst {
                        visited.insert(adj.neighbor_id);
                    }
                    let child_idx = arena.push(adj.neighbor_id, adj.edge_id, arena_idx);
                    queue.push_back((adj.neighbor_id, child_idx, depth + 1));
                }
            }

            // Produce output rows for each found path, reconstructing only now.
            for idx in found_indices {
                let (pn, pe) = arena.reconstruct_path(idx);
                let mut new_row = row.clone();
                new_row.insert(
                    path_alias.0.clone(),
                    Value::Path {
                        nodes: pn,
                        edges: pe,
                    },
                );
                cx.budget.record_row();
                out.push(new_row);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Direction, Identifier, Label, RelType};
    use crate::executor::budget::ExecutionBudget;
    use crate::executor::engine::MutationStats;
    use crate::planner::Operator;
    use crate::storage::{GraphStore, InMemoryGraphStore};
    use indexmap::IndexMap;

    fn make_store() -> InMemoryGraphStore {
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();
        store
    }

    fn executor_result(
        store: &InMemoryGraphStore,
        plan: &Operator,
        budget: &mut ExecutionBudget,
    ) -> Vec<Row> {
        let executor = QueryExecutor::new(store);
        executor.execute("g", plan, budget).unwrap().rows
    }

    #[test]
    fn expand_outgoing() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".to_string()], IndexMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["N".to_string()], IndexMap::new())
            .unwrap();
        store
            .create_edge("g", a, b, "R".to_string(), IndexMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Expand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("R")],
            src_alias: Identifier::new("a"),
            rel_alias: Some(Identifier::new("r")),
            dst_alias: Identifier::new("b"),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("a"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
    }

    #[test]
    fn expand_incoming() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".to_string()], IndexMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["N".to_string()], IndexMap::new())
            .unwrap();
        store
            .create_edge("g", a, b, "R".to_string(), IndexMap::new())
            .unwrap();
        let plan = Operator::Expand {
            direction: Direction::Incoming,
            rel_types: vec![RelType::new("R")],
            src_alias: Identifier::new("b"),
            rel_alias: Some(Identifier::new("r")),
            dst_alias: Identifier::new("a"),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("b"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor_result(&store, &plan, &mut budget);
        // Only b has an incoming edge, so we get 1 result
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn expand_no_matching_type() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".to_string()], IndexMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["N".to_string()], IndexMap::new())
            .unwrap();
        store
            .create_edge("g", a, b, "R".to_string(), IndexMap::new())
            .unwrap();
        let plan = Operator::Expand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("NONEXISTENT")],
            src_alias: Identifier::new("a"),
            rel_alias: Some(Identifier::new("r")),
            dst_alias: Identifier::new("b"),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("a"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor_result(&store, &plan, &mut budget);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn expand_isomorphism_prevents_edge_reuse() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".to_string()], IndexMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["N".to_string()], IndexMap::new())
            .unwrap();
        let c = store
            .create_node("g", vec!["N".to_string()], IndexMap::new())
            .unwrap();
        let e1 = store
            .create_edge("g", a, b, "R".to_string(), IndexMap::new())
            .unwrap();
        let _e2 = store
            .create_edge("g", b, c, "R".to_string(), IndexMap::new())
            .unwrap();

        let plan = Operator::Expand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("R")],
            src_alias: Identifier::new("b"),
            rel_alias: Some(Identifier::new("r2")),
            dst_alias: Identifier::new("c"),
            child: Box::new(Operator::Expand {
                direction: Direction::Outgoing,
                rel_types: vec![RelType::new("R")],
                src_alias: Identifier::new("a"),
                rel_alias: Some(Identifier::new("r1")),
                dst_alias: Identifier::new("b"),
                child: Box::new(Operator::NodeScan {
                    label: Some(Label::new("N")),
                    alias: Identifier::new("a"),
                }),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor_result(&store, &plan, &mut budget);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].get("r1"), Some(&Value::Edge(e1)));
    }

    #[test]
    fn expand_outer_edge_vars_allows_reuse_across_scopes() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".to_string()], IndexMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["N".to_string()], IndexMap::new())
            .unwrap();
        let e1 = store
            .create_edge("g", a, b, "R".to_string(), IndexMap::new())
            .unwrap();

        let mut input_row = Row::new();
        input_row.insert("a".to_string(), Value::Node(a));
        input_row.insert("r1".to_string(), Value::Edge(e1));
        input_row.insert("b2".to_string(), Value::Node(a));
        input_row.insert(
            "__outer_edge_vars__".to_string(),
            Value::List(vec![Value::Str("r1".to_string().into_boxed_str())]),
        );

        let executor = QueryExecutor::new(&store);
        let plan = Operator::Expand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("R")],
            src_alias: Identifier::new("b2"),
            rel_alias: Some(Identifier::new("r2")),
            dst_alias: Identifier::new("c"),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("b2"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let mut stats = MutationStats::default();
        let rows = executor
            .exec_operator(
                &mut ExecContext {
                    graph: "g",
                    budget: &mut budget,
                    outer_rows: &[input_row],
                    stats: &mut stats,
                },
                &plan,
            )
            .unwrap();
        let matching: Vec<_> = rows
            .iter()
            .filter(|r| r.get("r2") == Some(&Value::Edge(e1)))
            .collect();
        assert!(
            !matching.is_empty(),
            "outer edge var r1 should not block r2 from using the same edge"
        );
    }

    #[test]
    fn varlen_expand_chain() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".to_string()], IndexMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["N".to_string()], IndexMap::new())
            .unwrap();
        let c = store
            .create_node("g", vec!["N".to_string()], IndexMap::new())
            .unwrap();
        store
            .create_edge("g", a, b, "R".to_string(), IndexMap::new())
            .unwrap();
        store
            .create_edge("g", b, c, "R".to_string(), IndexMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::VarLenExpand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("R")],
            min: 1,
            max: 2,
            src_alias: Identifier::new("a"),
            path_alias: Some(Identifier::new("p")),
            path_mode: true,
            dst_alias: Identifier::new("b"),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("a"),
            }),
            rel_props: None,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 3);
    }

    #[test]
    fn varlen_expand_deep_chain() {
        let store = make_store();
        let mut nodes = Vec::new();
        for _ in 0..6 {
            nodes.push(
                store
                    .create_node("g", vec!["N".to_string()], IndexMap::new())
                    .unwrap(),
            );
        }
        for i in 0..5 {
            store
                .create_edge(
                    "g",
                    nodes[i],
                    nodes[i + 1],
                    "R".to_string(),
                    IndexMap::new(),
                )
                .unwrap();
        }
        let executor = QueryExecutor::new(&store);
        let plan = Operator::VarLenExpand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("R")],
            min: 1,
            max: 5,
            src_alias: Identifier::new("a"),
            path_alias: Some(Identifier::new("p")),
            path_mode: true,
            dst_alias: Identifier::new("b"),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("a"),
            }),
            rel_props: None,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        // 5+4+3+2+1 = 15
        assert_eq!(result.rows.len(), 15);

        let depth5_rows: Vec<_> = result
            .rows
            .iter()
            .filter(|r| {
                if let Some(Value::Path { nodes: pn, .. }) = r.get("p") {
                    pn.len() == 6
                } else {
                    false
                }
            })
            .collect();
        assert_eq!(depth5_rows.len(), 1);
        if let Some(Value::Path {
            nodes: pn,
            edges: pe,
        }) = depth5_rows[0].get("p")
        {
            assert_eq!(pn.len(), 6);
            assert_eq!(pe.len(), 5);
            for i in 0..6 {
                assert_eq!(pn[i], nodes[i]);
            }
        }
    }

    #[test]
    fn varlen_expand_rel_variable_mode() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".to_string()], IndexMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["N".to_string()], IndexMap::new())
            .unwrap();
        let c = store
            .create_node("g", vec!["N".to_string()], IndexMap::new())
            .unwrap();
        let e1 = store
            .create_edge("g", a, b, "R".to_string(), IndexMap::new())
            .unwrap();
        let e2 = store
            .create_edge("g", b, c, "R".to_string(), IndexMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::VarLenExpand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("R")],
            min: 2,
            max: 2,
            src_alias: Identifier::new("a"),
            path_alias: Some(Identifier::new("rs")),
            path_mode: false,
            dst_alias: Identifier::new("b"),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("a"),
            }),
            rel_props: None,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
        if let Some(Value::List(edges)) = result.rows[0].get("rs") {
            assert_eq!(edges.len(), 2);
            assert_eq!(edges[0], Value::Edge(e1));
            assert_eq!(edges[1], Value::Edge(e2));
        } else {
            panic!("expected list of edges for relationship variable mode");
        }
    }

    #[test]
    fn varlen_expand_min_zero() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".to_string()], IndexMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["N".to_string()], IndexMap::new())
            .unwrap();
        store
            .create_edge("g", a, b, "R".to_string(), IndexMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::VarLenExpand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("R")],
            min: 0,
            max: 1,
            src_alias: Identifier::new("a"),
            path_alias: Some(Identifier::new("p")),
            path_mode: true,
            dst_alias: Identifier::new("b"),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("a"),
            }),
            rel_props: None,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 3);
    }

    #[test]
    fn varlen_expand_cycle_edge_isomorphism() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".to_string()], IndexMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["N".to_string()], IndexMap::new())
            .unwrap();
        store
            .create_edge("g", a, b, "R".to_string(), IndexMap::new())
            .unwrap();
        store
            .create_edge("g", b, a, "R".to_string(), IndexMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::VarLenExpand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("R")],
            min: 1,
            max: 3,
            src_alias: Identifier::new("a"),
            path_alias: Some(Identifier::new("p")),
            path_mode: true,
            dst_alias: Identifier::new("b"),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("a"),
            }),
            rel_props: None,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 4);
    }

    #[test]
    fn shortest_path_finds_path() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".to_string()], IndexMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["N".to_string()], IndexMap::new())
            .unwrap();
        store
            .create_edge("g", a, b, "R".to_string(), IndexMap::new())
            .unwrap();
        let mut start_row = Row::new();
        start_row.insert("src".to_string(), Value::Node(a));
        start_row.insert("dst".to_string(), Value::Node(b));
        let executor = QueryExecutor::new(&store);
        let plan = Operator::ShortestPath {
            src_alias: Identifier::new("src"),
            dst_alias: Identifier::new("dst"),
            rel_types: vec![],
            direction: Direction::Outgoing,
            min: 1,
            max: 10,
            path_alias: Identifier::new("p"),
            all_paths: false,
        };
        let mut budget = ExecutionBudget::unlimited();
        let mut stats = MutationStats::default();
        let rows = executor
            .exec_operator(
                &mut ExecContext {
                    graph: "g",
                    budget: &mut budget,
                    outer_rows: &[start_row],
                    stats: &mut stats,
                },
                &plan,
            )
            .unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn shortest_path_deep_chain() {
        let store = make_store();
        let mut nodes = Vec::new();
        for _ in 0..4 {
            nodes.push(
                store
                    .create_node("g", vec!["N".to_string()], IndexMap::new())
                    .unwrap(),
            );
        }
        for i in 0..3 {
            store
                .create_edge(
                    "g",
                    nodes[i],
                    nodes[i + 1],
                    "R".to_string(),
                    IndexMap::new(),
                )
                .unwrap();
        }
        let mut start_row = Row::new();
        start_row.insert("src".to_string(), Value::Node(nodes[0]));
        start_row.insert("dst".to_string(), Value::Node(nodes[3]));
        let executor = QueryExecutor::new(&store);
        let plan = Operator::ShortestPath {
            src_alias: Identifier::new("src"),
            dst_alias: Identifier::new("dst"),
            rel_types: vec![],
            direction: Direction::Outgoing,
            min: 1,
            max: 10,
            path_alias: Identifier::new("p"),
            all_paths: false,
        };
        let mut budget = ExecutionBudget::unlimited();
        let mut stats = MutationStats::default();
        let rows = executor
            .exec_operator(
                &mut ExecContext {
                    graph: "g",
                    budget: &mut budget,
                    outer_rows: &[start_row],
                    stats: &mut stats,
                },
                &plan,
            )
            .unwrap();
        assert_eq!(rows.len(), 1);
        if let Some(Value::Path {
            nodes: pn,
            edges: pe,
        }) = rows[0].get("p")
        {
            assert_eq!(pn.len(), 4);
            assert_eq!(pe.len(), 3);
            for i in 0..4 {
                assert_eq!(pn[i], nodes[i]);
            }
        } else {
            panic!("expected path in result row");
        }
    }

    #[test]
    fn shortest_path_no_path_returns_empty() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".to_string()], IndexMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["N".to_string()], IndexMap::new())
            .unwrap();
        // No edge between a and b
        let mut start_row = Row::new();
        start_row.insert("src".to_string(), Value::Node(a));
        start_row.insert("dst".to_string(), Value::Node(b));
        let executor = QueryExecutor::new(&store);
        let plan = Operator::ShortestPath {
            src_alias: Identifier::new("src"),
            dst_alias: Identifier::new("dst"),
            rel_types: vec![],
            direction: Direction::Outgoing,
            min: 1,
            max: 10,
            path_alias: Identifier::new("p"),
            all_paths: false,
        };
        let mut budget = ExecutionBudget::unlimited();
        let mut stats = MutationStats::default();
        let rows = executor
            .exec_operator(
                &mut ExecContext {
                    graph: "g",
                    budget: &mut budget,
                    outer_rows: &[start_row],
                    stats: &mut stats,
                },
                &plan,
            )
            .unwrap();
        assert_eq!(rows.len(), 0, "no path should yield no rows");
    }

    #[test]
    fn expand_self_loop() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".to_string()], IndexMap::new())
            .unwrap();
        store
            .create_edge("g", a, a, "R".to_string(), IndexMap::new())
            .unwrap();
        let plan = Operator::Expand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("R")],
            src_alias: Identifier::new("a"),
            rel_alias: Some(Identifier::new("r")),
            dst_alias: Identifier::new("b"),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("a"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor_result(&store, &plan, &mut budget);
        // Self-loop: a -> a, so expand from a finds 1 edge back to a
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].get("b"), Some(&Value::Node(a)));
    }
}
