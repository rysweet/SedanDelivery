//! Graph traversal operators: node scan, expand, variable-length expand,
//! index scan, node-by-id seek, and shortest path.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::ast::Direction;
use crate::error::{ExecutionError, GqlError};
use crate::executor::budget::ExecutionBudget;
use crate::executor::filter::eval_expr;
use crate::executor::filter::Row as FilterRow;
use crate::planner::Operator;
use crate::storage::{EdgeId, NodeId, RangeQuery};
use crate::types::Value;

use super::helpers::{extract_node_id, get_adj_entries, inject_parameters, undefined_var};
use super::{BfsArena, MutationStats, QueryExecutor, Row};

impl<'a> QueryExecutor<'a> {
    pub(crate) fn exec_node_scan(
        &self,
        graph: &str,
        op: &Operator,
        budget: &mut ExecutionBudget,
        outer_rows: &[Row],
        _stats: &mut MutationStats,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::NodeScan { label, alias } = op else {
            unreachable!()
        };
        let node_ids = if let Some(lbl) = label {
            self.store.find_nodes_by_label(graph, lbl.as_str())?
        } else {
            self.store.all_node_ids(graph)?
        };

        // When used as the inner side of Apply/LeftOuterApply,
        // outer_rows carries context variables that inner operators
        // (e.g., Filter) may reference. Merge them into each produced row.
        // Exclude internal anonymous edge aliases (__anon_rel_*) — these are
        // per-pattern tracking variables and must not leak into inner patterns.
        let base_rows: &[Row] = if outer_rows.is_empty() {
            &[]
        } else {
            outer_rows
        };

        let mut rows = Vec::with_capacity(node_ids.len());
        for id in &node_ids {
            budget.check_or_err()?;
            if base_rows.is_empty() {
                let mut row = Row::new();
                row.insert(alias.0.clone(), Value::Node(*id));
                inject_parameters(&mut row, &self.parameters);
                budget.record_row();
                rows.push(row);
            } else {
                for outer_row in base_rows {
                    budget.check_or_err()?;
                    // If the alias is already pre-bound in outer_row to a specific node,
                    // only produce a row when the scanned node matches that bound node.
                    // This correctly handles correlated subqueries and repeated MATCH clauses.
                    // Only apply for non-anonymous variables (anonymous vars like __anon*
                    // should always scan all nodes independently).
                    if !alias.0.starts_with("__anon") {
                        if let Some(Value::Node(pre_bound_id)) = outer_row.get(alias.0.as_str()) {
                            if *pre_bound_id != *id {
                                continue; // Skip: doesn't match the pre-bound value
                            }
                        }
                    }
                    let mut row: Row = outer_row
                        .iter()
                        .filter(|(k, _)| !k.starts_with("__anon_rel_"))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    row.insert(alias.0.clone(), Value::Node(*id));
                    // Track outer-scope edge variable names so Expand's isomorphism
                    // check doesn't block edges from previous MATCH clauses.
                    let outer_edge_keys: Vec<Value> = outer_row
                        .iter()
                        .filter(|(k, v)| {
                            !k.starts_with("__anon_rel_")
                                && !k.starts_with("__outer_edge_vars__")
                                && matches!(v, Value::Edge(_))
                        })
                        .map(|(k, _)| Value::Str(k.clone().into_boxed_str()))
                        .collect();
                    if !outer_edge_keys.is_empty() {
                        let mut existing: Vec<Value> = match row.get("__outer_edge_vars__") {
                            Some(Value::List(vs)) => vs.clone(),
                            _ => vec![],
                        };
                        existing.extend(outer_edge_keys);
                        row.insert("__outer_edge_vars__".to_string(), Value::List(existing));
                    }
                    budget.record_row();
                    rows.push(row);
                }
            }
        }
        Ok(rows)
    }

    pub(crate) fn exec_expand(
        &self,
        graph: &str,
        op: &Operator,
        budget: &mut ExecutionBudget,
        outer_rows: &[Row],
        stats: &mut MutationStats,
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
            unreachable!()
        };
        let input = self.exec_operator(graph, child, budget, outer_rows, stats)?;
        let type_strs: Vec<&str> = rel_types.iter().map(|r| r.as_str()).collect();
        let mut out = Vec::with_capacity(input.len());

        for row in input {
            budget.check_or_err()?;
            let src_val = row
                .get(src_alias.0.as_str())
                .ok_or_else(|| undefined_var(&src_alias.0))?;
            let src_id = extract_node_id(src_val, &src_alias.0)?;

            let adj_entries = match direction {
                Direction::Outgoing => self.store.out_edges_by_type(graph, src_id, &type_strs)?,
                Direction::Incoming => self.store.in_edges_by_type(graph, src_id, &type_strs)?,
                Direction::Undirected => {
                    let mut out_entries =
                        self.store.out_edges_by_type(graph, src_id, &type_strs)?;
                    let in_entries = self.store.in_edges_by_type(graph, src_id, &type_strs)?;
                    // Deduplicate: for self-loops (src==dst), the same edge
                    // appears in both out_edges and in_edges. Keep only unique
                    // (edge_id, neighbor_id) pairs.
                    let mut seen_edges: HashSet<(u64, u64)> = HashSet::new();
                    for e in &out_entries {
                        seen_edges.insert((e.edge_id.0, e.neighbor_id.0));
                    }
                    for e in in_entries {
                        if seen_edges.insert((e.edge_id.0, e.neighbor_id.0)) {
                            out_entries.push(e);
                        }
                    }
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
                budget.check_or_err()?;
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
                let eid = adj.edge_id;
                let edge_already_used = row.iter().any(|(key, v)| {
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
                });
                if edge_already_used {
                    continue;
                }
                let mut new_row = row.clone();
                if let Some(ra) = rel_alias {
                    new_row.insert(ra.0.clone(), Value::Edge(adj.edge_id));
                }
                new_row.insert(dst_alias.0.clone(), dst_val);
                budget.record_row();
                out.push(new_row);
            }
        }
        Ok(out)
    }

    pub(crate) fn exec_var_len_expand(
        &self,
        graph: &str,
        op: &Operator,
        budget: &mut ExecutionBudget,
        outer_rows: &[Row],
        stats: &mut MutationStats,
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
            unreachable!()
        };
        let input = self.exec_operator(graph, child, budget, outer_rows, stats)?;
        let type_strs: Vec<&str> = rel_types.iter().map(|r| r.as_str()).collect();
        let mut out = Vec::with_capacity(input.len());

        for row in input {
            budget.check_or_err()?;
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
                budget.check_or_err()?;

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
                    budget.record_row();
                    out.push(new_row);
                }

                // Expand further if depth < max
                if depth < *max {
                    let adj_entries =
                        get_adj_entries(self.store, graph, current, direction, &type_strs)?;
                    for adj in adj_entries {
                        budget.check_or_err()?;
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
                            let ctx = self.build_filter_ctx(graph, scratch)?;
                            match self.eval_with_storage(graph, pred, scratch, &ctx) {
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

    pub(crate) fn exec_index_scan(
        &self,
        graph: &str,
        op: &Operator,
        budget: &mut ExecutionBudget,
        _outer_rows: &[Row],
        _stats: &mut MutationStats,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::IndexScan {
            label,
            property,
            alias,
        } = op
        else {
            unreachable!()
        };
        // Use the property index to find matching nodes.
        // For a bare IndexScan (no specific value predicate), we scan all nodes
        // with the label that have the property defined (using Gte on lowest value).
        // In practice, the planner pairs this with a Filter for the predicate.
        let node_ids = self.store.find_nodes_by_label_and_property(
            graph,
            label.as_str(),
            property,
            RangeQuery::Gte(&Value::Null),
        )?;

        // Fallback: if the range query returns empty (Null comparison issue),
        // fall back to label scan with property existence check.
        let effective_ids = if node_ids.is_empty() {
            let all_label = self.store.find_nodes_by_label(graph, label.as_str())?;
            let mut with_prop = Vec::new();
            for id in all_label {
                if let Some(node) = self.store.get_node(graph, id)? {
                    if node.properties.contains_key(property.as_str()) {
                        with_prop.push(id);
                    }
                }
            }
            with_prop
        } else {
            node_ids
        };

        let mut rows = Vec::with_capacity(effective_ids.len());
        for id in effective_ids {
            budget.check_or_err()?;
            let mut row = Row::new();
            row.insert(alias.0.clone(), Value::Node(id));
            budget.record_row();
            rows.push(row);
        }
        Ok(rows)
    }

    pub(crate) fn exec_node_by_id_seek(
        &self,
        graph: &str,
        op: &Operator,
        budget: &mut ExecutionBudget,
        _outer_rows: &[Row],
        _stats: &mut MutationStats,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::NodeByIdSeek { id_expr, alias } = op else {
            unreachable!()
        };
        let ctx: FilterRow<'_> = HashMap::new();
        let id_val = eval_expr(id_expr, &ctx)?;
        let raw_id = match id_val {
            Value::Int(n) if n >= 0 => n as u64,
            Value::Int(n) => {
                return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "non-negative integer for node ID".to_string(),
                    actual: n.to_string(),
                }));
            }
            other => {
                return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "integer for node ID".to_string(),
                    actual: other.type_name().to_string(),
                }));
            }
        };

        let node_id = NodeId(raw_id);
        let mut rows = Vec::with_capacity(1);
        if let Some(_node_data) = self.store.get_node(graph, node_id)? {
            let mut row = Row::new();
            row.insert(alias.0.clone(), Value::Node(node_id));
            budget.record_row();
            rows.push(row);
        }
        Ok(rows)
    }

    pub(crate) fn exec_shortest_path(
        &self,
        graph: &str,
        op: &Operator,
        budget: &mut ExecutionBudget,
        outer_rows: &[Row],
        _stats: &mut MutationStats,
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
            unreachable!()
        };
        // ShortestPath requires both src and dst to be bound in outer context.
        // For now, work from outer_rows or require them to be bound.
        let type_strs: Vec<&str> = rel_types.iter().map(|r| r.as_str()).collect();
        let mut out = Vec::new();

        // If there are outer rows, use them. Otherwise create an empty row.
        let context_rows = if outer_rows.is_empty() {
            vec![Row::new()]
        } else {
            outer_rows.to_vec()
        };

        for row in &context_rows {
            budget.check_or_err()?;
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
                budget.check_or_err()?;

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
                    get_adj_entries(self.store, graph, current, direction, &type_strs)?;
                for adj in adj_entries {
                    budget.check_or_err()?;
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
                budget.record_row();
                out.push(new_row);
            }
        }
        Ok(out)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Direction, Identifier, Label, RelType};
    use crate::executor::budget::ExecutionBudget;
    use crate::storage::{GraphStore, InMemoryGraphStore};

    fn make_store() -> InMemoryGraphStore {
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();
        store
    }

    #[test]
    fn node_scan_all_nodes() {
        let store = make_store();
        store
            .create_node("g", vec!["A".to_string()], HashMap::new())
            .unwrap();
        store
            .create_node("g", vec!["B".to_string()], HashMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::NodeScan {
            label: None,
            alias: Identifier::new("n"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 2);
    }

    #[test]
    fn node_scan_with_label_filter() {
        let store = make_store();
        store
            .create_node("g", vec!["Person".to_string()], HashMap::new())
            .unwrap();
        store
            .create_node("g", vec!["Movie".to_string()], HashMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::NodeScan {
            label: Some(Label::new("Person")),
            alias: Identifier::new("n"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
    }

    #[test]
    fn expand_outgoing() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        store
            .create_edge("g", a, b, "R".to_string(), HashMap::new())
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
    fn varlen_expand_chain() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        let c = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        store
            .create_edge("g", a, b, "R".to_string(), HashMap::new())
            .unwrap();
        store
            .create_edge("g", b, c, "R".to_string(), HashMap::new())
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
    fn index_scan_finds_nodes() {
        let store = make_store();
        let mut p1 = HashMap::new();
        p1.insert("age".to_string(), Value::Int(30));
        store
            .create_node("g", vec!["Person".to_string()], p1)
            .unwrap();
        store
            .create_node("g", vec!["Person".to_string()], HashMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::IndexScan {
            label: Label::new("Person"),
            property: "age".to_string(),
            alias: Identifier::new("n"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
    }

    #[test]
    fn node_by_id_seek_found() {
        let store = make_store();
        let node_id = store
            .create_node("g", vec!["Person".to_string()], HashMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::NodeByIdSeek {
            id_expr: crate::ast::Expr::Integer(node_id.0 as i64),
            alias: Identifier::new("n"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
    }

    #[test]
    fn shortest_path_finds_path() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        store
            .create_edge("g", a, b, "R".to_string(), HashMap::new())
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
            .exec_operator("g", &plan, &mut budget, &[start_row], &mut stats)
            .unwrap();
        assert_eq!(rows.len(), 1);
    }

    /// Verify that edge isomorphism works correctly in exec_expand after hoisting
    /// the outer_edge_vars HashSet to the per-row level. In a triangle graph
    /// (a)-[r1]->(b)-[r2]->(c), expanding from (b) with r1 already bound should
    /// not re-use r1 for r2 (isomorphism), but should allow r2.
    #[test]
    fn expand_isomorphism_prevents_edge_reuse() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        let c = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        let e1 = store
            .create_edge("g", a, b, "R".to_string(), HashMap::new())
            .unwrap();
        let _e2 = store
            .create_edge("g", b, c, "R".to_string(), HashMap::new())
            .unwrap();

        // Build a plan: NodeScan(a) -> Expand(a-[r1]->b) -> Expand(b-[r2]->c)
        // The second Expand should NOT reuse the same edge as r1.
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
        // Only one valid path: a-[e1]->b-[e2]->c
        // The path a-[e1]->b would NOT have b-[e1]->? because e1 goes a->b not b->anything
        // But we check that the result is correct (1 row: a->b->c)
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].get("r1"), Some(&Value::Edge(e1)));
    }

    /// Verify that outer_edge_vars correctly excludes edges from previous MATCH
    /// scopes, allowing them to be traversed again in the current pattern.
    #[test]
    fn expand_outer_edge_vars_allows_reuse_across_scopes() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        let e1 = store
            .create_edge("g", a, b, "R".to_string(), HashMap::new())
            .unwrap();

        // Simulate a second MATCH clause that re-traverses the same edge.
        // Input row: {a: Node(a), r1: Edge(e1), b: Node(b), __outer_edge_vars__: ["r1"]}
        // Expand from a -[r2]-> b should still find e1 because r1 is an outer edge var.
        let mut input_row = Row::new();
        input_row.insert("a".to_string(), Value::Node(a));
        input_row.insert("r1".to_string(), Value::Edge(e1));
        input_row.insert("b2".to_string(), Value::Node(a)); // src for new expand
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
        // Execute with the pre-built input row as outer context
        let rows = executor
            .exec_operator("g", &plan, &mut budget, &[input_row], &mut stats)
            .unwrap();
        // The expand should find a->b via e1, even though e1 is in the row as r1,
        // because r1 is listed in __outer_edge_vars__ (from a previous MATCH scope).
        let matching: Vec<_> = rows
            .iter()
            .filter(|r| r.get("r2") == Some(&Value::Edge(e1)))
            .collect();
        assert!(
            !matching.is_empty(),
            "outer edge var r1 should not block r2 from using the same edge"
        );
    }

    /// Helper: execute a plan and return the resulting rows.
    fn executor_result(
        store: &InMemoryGraphStore,
        plan: &Operator,
        budget: &mut ExecutionBudget,
    ) -> Vec<Row> {
        let executor = QueryExecutor::new(store);
        executor.execute("g", plan, budget).unwrap().rows
    }

    /// Verify that the budget check fires inside the inner loop
    /// (node * outer_row Cartesian product) of exec_node_scan.
    #[test]
    fn node_scan_budget_enforced_in_inner_loop() {
        use std::time::Duration;

        let store = make_store();
        // Create a few nodes.
        for _ in 0..3 {
            store
                .create_node("g", vec!["N".to_string()], HashMap::new())
                .unwrap();
        }

        // Build outer_rows with 3 rows so the Cartesian product is 3 nodes * 3 rows = 9.
        let outer_rows: Vec<Row> = (0..3)
            .map(|_| {
                let mut r = Row::new();
                r.insert("__dummy__".to_string(), Value::Int(0));
                r
            })
            .collect();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("x"),
        };

        // Budget that allows only 2 rows -- the inner loop should hit the limit
        // before completing all 9 combinations.
        let mut budget = ExecutionBudget::new(Duration::from_secs(60), 2);
        let mut stats = MutationStats::default();
        let result = executor.exec_node_scan("g", &plan, &mut budget, &outer_rows, &mut stats);
        assert!(
            result.is_err(),
            "expected budget error for Cartesian product exceeding row limit"
        );
    }

    // -- Arena-based BFS path tests ------------------------------------------

    /// Test VarLenExpand with a longer chain (depth 5) to verify arena
    /// reconstruction produces the correct path at each depth.
    #[test]
    fn varlen_expand_deep_chain() {
        let store = make_store();
        // Build a chain: n0 -> n1 -> n2 -> n3 -> n4 -> n5
        let mut nodes = Vec::new();
        for _ in 0..6 {
            nodes.push(
                store
                    .create_node("g", vec!["N".to_string()], HashMap::new())
                    .unwrap(),
            );
        }
        for i in 0..5 {
            store
                .create_edge("g", nodes[i], nodes[i + 1], "R".to_string(), HashMap::new())
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
        // For a 6-node chain, from each starting node we get paths:
        // n0: depth 1..5 -> 5 paths  (to n1, n2, n3, n4, n5)
        // n1: depth 1..4 -> 4 paths  (to n2, n3, n4, n5)
        // n2: depth 1..3 -> 3 paths  (to n3, n4, n5)
        // n3: depth 1..2 -> 2 paths  (to n4, n5)
        // n4: depth 1    -> 1 path   (to n5)
        // n5: no outgoing edges -> 0
        // Total: 5+4+3+2+1 = 15
        assert_eq!(result.rows.len(), 15);

        // Verify that path_nodes for the longest path (depth=5) from n0 is correct
        let depth5_rows: Vec<_> = result
            .rows
            .iter()
            .filter(|r| {
                if let Some(Value::Path { nodes: pn, .. }) = r.get("p") {
                    pn.len() == 6 // 6 nodes = depth 5
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

    /// Test VarLenExpand with relationship variable mode (path_mode=false).
    #[test]
    fn varlen_expand_rel_variable_mode() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        let c = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        let e1 = store
            .create_edge("g", a, b, "R".to_string(), HashMap::new())
            .unwrap();
        let e2 = store
            .create_edge("g", b, c, "R".to_string(), HashMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::VarLenExpand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("R")],
            min: 2,
            max: 2,
            src_alias: Identifier::new("a"),
            path_alias: Some(Identifier::new("rs")),
            path_mode: false, // relationship variable mode
            dst_alias: Identifier::new("b"),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("a"),
            }),
            rel_props: None,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        // Only 1 result: a-[e1]->b-[e2]->c with min=max=2
        assert_eq!(result.rows.len(), 1);
        if let Some(Value::List(edges)) = result.rows[0].get("rs") {
            assert_eq!(edges.len(), 2);
            assert_eq!(edges[0], Value::Edge(e1));
            assert_eq!(edges[1], Value::Edge(e2));
        } else {
            panic!("expected list of edges for relationship variable mode");
        }
    }

    /// Test ShortestPath with a longer chain to verify arena reconstruction.
    #[test]
    fn shortest_path_deep_chain() {
        let store = make_store();
        // Build: n0 -> n1 -> n2 -> n3
        let mut nodes = Vec::new();
        for _ in 0..4 {
            nodes.push(
                store
                    .create_node("g", vec!["N".to_string()], HashMap::new())
                    .unwrap(),
            );
        }
        for i in 0..3 {
            store
                .create_edge("g", nodes[i], nodes[i + 1], "R".to_string(), HashMap::new())
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
            .exec_operator("g", &plan, &mut budget, &[start_row], &mut stats)
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

    /// Test VarLenExpand with min=0 (includes zero-length paths).
    #[test]
    fn varlen_expand_min_zero() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        store
            .create_edge("g", a, b, "R".to_string(), HashMap::new())
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
        // n=a: depth 0 -> itself, depth 1 -> b (2 paths)
        // n=b: depth 0 -> itself (1 path, no outgoing R)
        // Total: 3
        assert_eq!(result.rows.len(), 3);
    }

    /// Test edge isomorphism in VarLenExpand: a cycle a->b->a should not
    /// reuse the same edge.
    #[test]
    fn varlen_expand_cycle_edge_isomorphism() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        // a->b and b->a (two distinct edges)
        store
            .create_edge("g", a, b, "R".to_string(), HashMap::new())
            .unwrap();
        store
            .create_edge("g", b, a, "R".to_string(), HashMap::new())
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
        // From a: a->b (d=1), a->b->a (d=2). d=3 would need reusing e(a->b). STOP.
        // From b: b->a (d=1), b->a->b (d=2). d=3 would need reusing e(b->a). STOP.
        // Total = 4
        assert_eq!(result.rows.len(), 4);
    }
}
