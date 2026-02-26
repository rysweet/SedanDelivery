//! Query executor — evaluates a physical [`Operator`] tree against a [`GraphStore`].
//!
//! # Row representation
//!
//! A [`Row`] is an owned map from column name to [`Value`].  During execution
//! node/edge variables hold `Value::Node(id)` / `Value::Edge(id)`.  When an
//! expression needs a property value (e.g. `n.name`) the executor resolves the
//! ID against the store and returns the concrete property value.
//!
//! # Budget enforcement
//!
//! Every operator checks `budget.check_or_err()` before doing any work.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::ast::{Direction, Expr};
use crate::error::{ExecutionError, GqlError};
use crate::executor::budget::ExecutionBudget;
use crate::executor::filter::{eval_expr, eval_function_with_vals};
use crate::executor::filter::Row as FilterRow;
use crate::planner::{AggFunction, Operator, RemoveItem, SetItem};
use crate::storage::{EdgeId, GraphStore, NodeId, RangeQuery};
use crate::types::Value;

// ── Public types ──────────────────────────────────────────────────────────────

/// A single result row: owned map from column name to value.
pub type Row = HashMap<String, Value>;

/// The final result of executing a query.
#[derive(Debug)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Row>,
    pub stats: MutationStats,
}

/// Counters for write operations performed during query execution.
#[derive(Debug, Default, Clone)]
pub struct MutationStats {
    pub nodes_created: u64,
    pub nodes_deleted: u64,
    pub edges_created: u64,
    pub edges_deleted: u64,
    pub properties_set: u64,
    pub labels_added: u64,
}

// ── QueryExecutor ─────────────────────────────────────────────────────────────

/// Executes a physical plan against a [`GraphStore`].
pub struct QueryExecutor<'a> {
    store: &'a dyn GraphStore,
}

impl<'a> QueryExecutor<'a> {
    /// Create a new executor backed by `store`.
    pub fn new(store: &'a dyn GraphStore) -> Self {
        Self { store }
    }

    /// Execute `plan` against the named `graph`, enforcing `budget`.
    pub fn execute(
        &self,
        graph: &str,
        plan: &Operator,
        budget: &mut ExecutionBudget,
    ) -> Result<QueryResult, GqlError> {
        let mut stats = MutationStats::default();
        let rows = self.exec_operator(graph, plan, budget, &[], &mut stats)?;

        // Derive column list.  Handle the case where the root is Distinct{ProduceResults}.
        let effective_root = match plan {
            Operator::Distinct { child } => child.as_ref(),
            other => other,
        };
        let has_produce_results = has_produce_results(effective_root);
        let columns = if let Operator::ProduceResults { columns, .. } = effective_root {
            columns.iter().map(|(name, _)| name.clone()).collect()
        } else {
            rows.first()
                .map(|r| {
                    let mut cols: Vec<String> = r.keys().cloned().collect();
                    cols.sort();
                    cols
                })
                .unwrap_or_default()
        };

        Ok(QueryResult {
            columns,
            rows,
            stats,
        })
    }

    // ── Internal dispatch ─────────────────────────────────────────────────────

    /// Recursively execute an operator, returning its output rows.
    ///
    /// `outer_rows` carries the current row context for correlated operators
    /// (currently unused — kept for future Apply/LeftOuterApply support).
    #[allow(clippy::only_used_in_recursion)]
    fn exec_operator(
        &self,
        graph: &str,
        op: &Operator,
        budget: &mut ExecutionBudget,
        outer_rows: &[Row],
        stats: &mut MutationStats,
    ) -> Result<Vec<Row>, GqlError> {
        budget.check_or_err()?;

        match op {
            // ── SingleRow ─────────────────────────────────────────────────────
            Operator::SingleRow => {
                budget.record_row();
                Ok(vec![Row::new()])
            }

            // ── NodeScan ──────────────────────────────────────────────────────
            Operator::NodeScan { label, alias } => {
                let node_ids = if let Some(lbl) = label {
                    self.store.find_nodes_by_label(graph, lbl.as_str())?
                } else {
                    self.store.all_node_ids(graph)?
                };

                let mut rows = Vec::with_capacity(node_ids.len());
                for id in node_ids {
                    budget.check_or_err()?;
                    let mut row = Row::new();
                    row.insert(alias.0.clone(), Value::Node(id));
                    budget.record_row();
                    rows.push(row);
                }
                Ok(rows)
            }

            // ── Filter ────────────────────────────────────────────────────────
            Operator::Filter { predicate, child } => {
                let input = self.exec_operator(graph, child, budget, outer_rows, stats)?;
                let mut out = Vec::new();
                for row in input {
                    budget.check_or_err()?;
                    let ctx = self.build_filter_ctx(graph, &row)?;
                    // Use eval_with_storage so that property access on Node/Edge IDs
                    // is resolved against the store rather than returning Null.
                    let result = self.eval_with_storage(graph, predicate, &row, &ctx)?;
                    if result == Value::Bool(true) {
                        out.push(row);
                    }
                }
                Ok(out)
            }

            // ── Expand ────────────────────────────────────────────────────────
            Operator::Expand {
                direction,
                rel_types,
                src_alias,
                rel_alias,
                dst_alias,
                child,
            } => {
                let input = self.exec_operator(graph, child, budget, outer_rows, stats)?;
                let type_strs: Vec<&str> = rel_types.iter().map(|r| r.as_str()).collect();
                let mut out = Vec::new();

                for row in input {
                    budget.check_or_err()?;
                    let src_val = row
                        .get(src_alias.0.as_str())
                        .ok_or_else(|| undefined_var(&src_alias.0))?;
                    let src_id = extract_node_id(src_val, &src_alias.0)?;

                    let adj_entries = match direction {
                        Direction::Outgoing => {
                            self.store.out_edges_by_type(graph, src_id, &type_strs)?
                        }
                        Direction::Incoming => {
                            self.store.in_edges_by_type(graph, src_id, &type_strs)?
                        }
                        Direction::Undirected => {
                            let mut out_entries =
                                self.store.out_edges_by_type(graph, src_id, &type_strs)?;
                            let in_entries =
                                self.store.in_edges_by_type(graph, src_id, &type_strs)?;
                            out_entries.extend(in_entries);
                            out_entries
                        }
                    };

                    for adj in adj_entries {
                        budget.check_or_err()?;
                        let mut new_row = row.clone();
                        if let Some(ra) = rel_alias {
                            new_row.insert(ra.0.clone(), Value::Edge(adj.edge_id));
                        }
                        new_row.insert(dst_alias.0.clone(), Value::Node(adj.neighbor_id));
                        budget.record_row();
                        out.push(new_row);
                    }
                }
                Ok(out)
            }

            // ── Project ───────────────────────────────────────────────────────
            Operator::Project { items, child } => {
                let input = self.exec_operator(graph, child, budget, outer_rows, stats)?;
                let mut out = Vec::with_capacity(input.len());

                for row in &input {
                    budget.check_or_err()?;
                    let ctx = self.build_filter_ctx(graph, row)?;
                    let mut new_row = Row::new();
                    for (expr, alias) in items {
                        let val = self.eval_with_storage(graph, expr, row, &ctx)?;
                        let col_name = match alias {
                            Some(id) => id.0.clone(),
                            None => expr_to_column_name(expr),
                        };
                        new_row.insert(col_name, val);
                    }
                    budget.record_row();
                    out.push(new_row);
                }
                Ok(out)
            }

            // ── Distinct ──────────────────────────────────────────────────────
            Operator::Distinct { child } => {
                let input = self.exec_operator(graph, child, budget, outer_rows, stats)?;
                let mut seen: HashSet<String> = HashSet::new();
                let mut out = Vec::new();
                for row in input {
                    budget.check_or_err()?;
                    let key = row_key(&row);
                    if seen.insert(key) {
                        out.push(row);
                    }
                }
                Ok(out)
            }

            // ── Sort ──────────────────────────────────────────────────────────
            Operator::Sort { items, child } => {
                let input = self.exec_operator(graph, child, budget, outer_rows, stats)?;
                budget.check_or_err()?;

                // Evaluate all sort keys up front to avoid repeated store lookups.
                let mut keyed: Vec<(Vec<Value>, Row)> = input
                    .into_iter()
                    .map(|row| {
                        let ctx = self.build_filter_ctx(graph, &row)?;
                        let keys: Result<Vec<Value>, GqlError> = items
                            .iter()
                            .map(|(expr, _)| self.eval_with_storage(graph, expr, &row, &ctx))
                            .collect();
                        Ok((keys?, row))
                    })
                    .collect::<Result<Vec<_>, GqlError>>()?;

                keyed.sort_by(|(ka, _), (kb, _)| {
                    for i in 0..items.len() {
                        let va = &ka[i];
                        let vb = &kb[i];
                        let asc = items[i].1;
                        let ord = cmp_values(va, vb);
                        if ord != std::cmp::Ordering::Equal {
                            return if asc { ord } else { ord.reverse() };
                        }
                    }
                    std::cmp::Ordering::Equal
                });

                Ok(keyed.into_iter().map(|(_, row)| row).collect())
            }

            // ── Limit ─────────────────────────────────────────────────────────
            Operator::Limit { count, child } => {
                let input = self.exec_operator(graph, child, budget, outer_rows, stats)?;
                budget.check_or_err()?;
                let n = eval_count_expr(count, &HashMap::new())?;
                Ok(input.into_iter().take(n).collect())
            }

            // ── Skip ──────────────────────────────────────────────────────────
            Operator::Skip { count, child } => {
                let input = self.exec_operator(graph, child, budget, outer_rows, stats)?;
                budget.check_or_err()?;
                let n = eval_count_expr(count, &HashMap::new())?;
                Ok(input.into_iter().skip(n).collect())
            }

            // ── Aggregate ─────────────────────────────────────────────────────
            Operator::Aggregate {
                keys,
                aggregations,
                child,
            } => {
                let input = self.exec_operator(graph, child, budget, outer_rows, stats)?;
                budget.check_or_err()?;

                // Group rows by the key expression values.
                // We use a Vec-of-(key_row, agg_state) because Value doesn't impl Hash.
                let mut groups: Vec<(Vec<Value>, Vec<AggState>)> = Vec::new();

                for row in &input {
                    let ctx = self.build_filter_ctx(graph, row)?;
                    let group_key: Vec<Value> = keys
                        .iter()
                        .map(|expr| self.eval_with_storage(graph, expr, row, &ctx))
                        .collect::<Result<Vec<_>, GqlError>>()?;

                    // Find existing group or create new.
                    let group_idx = groups.iter().position(|(k, _)| k == &group_key);
                    let idx = if let Some(i) = group_idx {
                        i
                    } else {
                        let new_states = aggregations.iter().map(|_| AggState::default()).collect();
                        groups.push((group_key, new_states));
                        groups.len() - 1
                    };

                    // Accumulate into each aggregation state.
                    for (agg_idx, (func, arg_expr, distinct, _alias)) in
                        aggregations.iter().enumerate()
                    {
                        // count(*) — the `*` argument is a sentinel meaning "count rows".
                        // Use a non-null placeholder so the accumulator counts every row.
                        let arg_val =
                            if matches!(arg_expr, Expr::Variable(ref id) if id.0 == "*") {
                                Value::Bool(true)
                            } else {
                                self.eval_with_storage(graph, arg_expr, row, &ctx)?
                            };
                        groups[idx].1[agg_idx].accumulate(func, arg_val, *distinct);
                    }
                }

                // Handle empty input with no-key aggregation (e.g. COUNT(*) on empty).
                if groups.is_empty() && keys.is_empty() {
                    let new_states: Vec<AggState> =
                        aggregations.iter().map(|_| AggState::default()).collect();
                    groups.push((vec![], new_states));
                }

                // Build output rows.
                let mut out = Vec::with_capacity(groups.len());
                for (key_vals, agg_states) in groups {
                    budget.check_or_err()?;
                    let mut row = Row::new();

                    // Add group key columns — derive names from key expressions.
                    for (expr, val) in keys.iter().zip(key_vals) {
                        row.insert(expr_to_column_name(expr), val);
                    }

                    // Add aggregation result columns.
                    for ((func, _, _, alias), state) in aggregations.iter().zip(&agg_states) {
                        row.insert(alias.0.clone(), state.finalize(func));
                    }
                    budget.record_row();
                    out.push(row);
                }
                Ok(out)
            }

            // ── Create ────────────────────────────────────────────────────────
            Operator::Create { patterns, child } => {
                // Execute child first (for MATCH ... CREATE patterns).
                let input = if let Some(ch) = child {
                    self.exec_operator(graph, ch, budget, outer_rows, stats)?
                } else {
                    // No child means standalone CREATE: one empty row to iterate over.
                    vec![Row::new()]
                };

                let mut out = Vec::new();
                for mut row in input {
                    budget.check_or_err()?;
                    for pattern in patterns {
                        // Create nodes first, mapping alias -> NodeId.
                        let mut alias_to_id: HashMap<String, NodeId> = HashMap::new();

                        for (alias, labels, prop_exprs) in &pattern.nodes {
                            let ctx = self.build_filter_ctx(graph, &row)?;
                            let props = eval_prop_map(graph, self, prop_exprs, &row, &ctx)?;
                            let label_strings: Vec<String> =
                                labels.iter().map(|l| l.as_str().to_string()).collect();
                            let label_count = label_strings.len() as u64;
                            let node_id = self.store.create_node(graph, label_strings, props)?;
                            stats.nodes_created += 1;
                            stats.labels_added += label_count;
                            if let Some(a) = alias {
                                alias_to_id.insert(a.0.clone(), node_id);
                                row.insert(a.0.clone(), Value::Node(node_id));
                            }
                        }

                        // Create edges using the alias map for src/dst lookup.
                        for (alias, src_alias, dst_alias, rel_type, _direction, prop_exprs) in
                            &pattern.edges
                        {
                            let ctx = self.build_filter_ctx(graph, &row)?;
                            let props = eval_prop_map(graph, self, prop_exprs, &row, &ctx)?;

                            let src_id = alias_to_id
                                .get(&src_alias.0)
                                .copied()
                                .or_else(|| {
                                    row.get(&src_alias.0).and_then(|v| {
                                        if let Value::Node(id) = v {
                                            Some(*id)
                                        } else {
                                            None
                                        }
                                    })
                                })
                                .ok_or_else(|| undefined_var(&src_alias.0))?;

                            let dst_id = alias_to_id
                                .get(&dst_alias.0)
                                .copied()
                                .or_else(|| {
                                    row.get(&dst_alias.0).and_then(|v| {
                                        if let Value::Node(id) = v {
                                            Some(*id)
                                        } else {
                                            None
                                        }
                                    })
                                })
                                .ok_or_else(|| undefined_var(&dst_alias.0))?;

                            let edge_id = self.store.create_edge(
                                graph,
                                src_id,
                                dst_id,
                                rel_type.as_str().to_string(),
                                props,
                            )?;
                            stats.edges_created += 1;
                            if let Some(a) = alias {
                                row.insert(a.0.clone(), Value::Edge(edge_id));
                            }
                        }
                    }
                    budget.record_row();
                    out.push(row);
                }
                Ok(out)
            }

            // ── AssemblePath ──────────────────────────────────────────────────
            Operator::AssemblePath {
                path_alias,
                node_aliases,
                edge_aliases,
                child,
            } => {
                use crate::storage::{NodeId, EdgeId};
                let input = self.exec_operator(graph, child, budget, outer_rows, stats)?;
                let mut out = Vec::with_capacity(input.len());
                for mut row in input {
                    budget.check_or_err()?;
                    let mut nodes: Vec<NodeId> = Vec::new();
                    let mut edges: Vec<EdgeId> = Vec::new();
                    for na in node_aliases {
                        if let Some(Value::Node(nid)) = row.get(&na.0) {
                            nodes.push(*nid);
                        }
                    }
                    for ea in edge_aliases {
                        if let Some(Value::Edge(eid)) = row.get(&ea.0) {
                            edges.push(*eid);
                        }
                    }
                    let path_val = Value::Path { nodes, edges };
                    row.insert(path_alias.0.clone(), path_val);
                    out.push(row);
                }
                Ok(out)
            }

            // ── ProduceResults ────────────────────────────────────────────────
            Operator::ProduceResults { columns, child } => {
                let input = self.exec_operator(graph, child, budget, outer_rows, stats)?;
                let mut out = Vec::with_capacity(input.len());

                // Check if any column is the '*' wildcard that needs
                // runtime expansion (fallback for planner edge-cases).
                let has_star = columns
                    .iter()
                    .any(|(name, _)| name == "*");

                for row in &input {
                    budget.check_or_err()?;
                    let ctx = self.build_filter_ctx(graph, row)?;
                    let mut new_row = Row::new();

                    if has_star {
                        // Expand '*' to all variables in the row.
                        for (col_name, expr) in columns {
                            if col_name == "*" {
                                // Insert all non-internal variables from the
                                // input row, resolving through storage.
                                for (var_name, val) in row {
                                    if !var_name.starts_with("__") {
                                        new_row.insert(var_name.clone(), val.clone());
                                    }
                                }
                            } else {
                                let val = self.eval_with_storage(graph, expr, row, &ctx)?;
                                new_row.insert(col_name.clone(), val);
                            }
                        }
                    } else {
                        for (col_name, expr) in columns {
                            let val = self.eval_with_storage(graph, expr, row, &ctx)?;
                            new_row.insert(col_name.clone(), val);
                        }
                    }
                    budget.record_row();
                    out.push(new_row);
                }
                Ok(out)
            }

            // ── VarLenExpand ──────────────────────────────────────────────────
            Operator::VarLenExpand {
                direction,
                rel_types,
                min,
                max,
                src_alias,
                path_alias,
                dst_alias,
                child,
            } => {
                let input = self.exec_operator(graph, child, budget, outer_rows, stats)?;
                let type_strs: Vec<&str> = rel_types.iter().map(|r| r.as_str()).collect();
                let mut out = Vec::new();

                for row in input {
                    budget.check_or_err()?;
                    let src_val = row
                        .get(src_alias.0.as_str())
                        .ok_or_else(|| undefined_var(&src_alias.0))?;
                    let src_id = extract_node_id(src_val, &src_alias.0)?;

                    // BFS tracking full paths: queue entries are (node_id, path_nodes, path_edges)
                    let mut queue: VecDeque<(NodeId, Vec<NodeId>, Vec<EdgeId>)> = VecDeque::new();
                    queue.push_back((src_id, vec![src_id], vec![]));
                    // Track visited to avoid cycles
                    let mut visited = HashSet::new();
                    visited.insert(src_id);

                    while let Some((current, path_nodes, path_edges)) = queue.pop_front() {
                        budget.check_or_err()?;
                        let depth = path_edges.len() as u32;

                        // Yield this path if depth is within [min, max]
                        if depth >= *min && depth <= *max {
                            let mut new_row = row.clone();
                            let end_node = *path_nodes.last().unwrap();
                            new_row.insert(dst_alias.0.clone(), Value::Node(end_node));
                            if let Some(pa) = path_alias {
                                new_row.insert(
                                    pa.0.clone(),
                                    Value::Path {
                                        nodes: path_nodes.clone(),
                                        edges: path_edges.clone(),
                                    },
                                );
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
                                // Allow revisiting nodes for different paths, but avoid
                                // infinite cycles by not revisiting at the expansion level.
                                // For VarLenExpand, we only skip nodes already in the
                                // current path to allow different paths through the graph.
                                if path_nodes.contains(&adj.neighbor_id) {
                                    continue;
                                }
                                let mut new_path_nodes = path_nodes.clone();
                                new_path_nodes.push(adj.neighbor_id);
                                let mut new_path_edges = path_edges.clone();
                                new_path_edges.push(adj.edge_id);
                                queue.push_back((adj.neighbor_id, new_path_nodes, new_path_edges));
                            }
                        }
                    }
                }
                Ok(out)
            }

            // ── Set ──────────────────────────────────────────────────────────────
            Operator::Set { items, child } => {
                let input = self.exec_operator(graph, child, budget, outer_rows, stats)?;
                let mut out = Vec::new();

                for row in input {
                    budget.check_or_err()?;
                    let current_row = row;
                    for item in items {
                        self.apply_set_item(graph, &current_row, item, stats)?;
                    }
                    budget.record_row();
                    out.push(current_row);
                }
                Ok(out)
            }

            // ── Remove ───────────────────────────────────────────────────────────
            Operator::Remove { items, child } => {
                let input = self.exec_operator(graph, child, budget, outer_rows, stats)?;
                let mut out = Vec::new();

                for row in input {
                    budget.check_or_err()?;
                    for item in items {
                        match item {
                            RemoveItem::Property { alias, key } => {
                                let val = row
                                    .get(alias.0.as_str())
                                    .ok_or_else(|| undefined_var(&alias.0))?;
                                match val {
                                    Value::Node(node_id) => {
                                        self.store.remove_node_property(graph, *node_id, key)?;
                                    }
                                    Value::Edge(edge_id) => {
                                        self.store.remove_edge_property(graph, *edge_id, key)?;
                                    }
                                    other => {
                                        return Err(GqlError::Execution(
                                            ExecutionError::TypeMismatch {
                                                expected: "Node or Relationship".to_string(),
                                                actual: other.type_name().to_string(),
                                            },
                                        ));
                                    }
                                }
                                stats.properties_set += 1;
                            }
                            RemoveItem::Labels { alias, labels } => {
                                let val = row
                                    .get(alias.0.as_str())
                                    .ok_or_else(|| undefined_var(&alias.0))?;
                                let node_id = extract_node_id(val, &alias.0)?;
                                let label_strings: Vec<String> =
                                    labels.iter().map(|l| l.as_str().to_string()).collect();
                                self.store
                                    .remove_node_labels(graph, node_id, &label_strings)?;
                                stats.labels_added += label_strings.len() as u64;
                            }
                        }
                    }
                    budget.record_row();
                    out.push(row);
                }
                Ok(out)
            }

            // ── Delete ───────────────────────────────────────────────────────────
            Operator::Delete {
                exprs,
                detach,
                child,
            } => {
                let input = self.exec_operator(graph, child, budget, outer_rows, stats)?;
                let mut out = Vec::new();

                for row in input {
                    budget.check_or_err()?;
                    let ctx = self.build_filter_ctx(graph, &row)?;
                    for expr in exprs {
                        let val = self.eval_with_storage(graph, expr, &row, &ctx)?;
                        match val {
                            Value::Node(node_id) => {
                                self.store.delete_node(graph, node_id, *detach)?;
                                stats.nodes_deleted += 1;
                            }
                            Value::Edge(edge_id) => {
                                self.store.delete_edge(graph, edge_id)?;
                                stats.edges_deleted += 1;
                            }
                            Value::Null => {
                                // Deleting NULL is a no-op in Cypher
                            }
                            other => {
                                return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                                    expected: "Node or Relationship for DELETE".to_string(),
                                    actual: other.type_name().to_string(),
                                }));
                            }
                        }
                    }
                    budget.record_row();
                    out.push(row);
                }
                Ok(out)
            }

            // ── Merge ────────────────────────────────────────────────────────────
            Operator::Merge {
                node_alias,
                labels,
                on_create,
                on_match,
                child,
            } => {
                let input = if let Some(ch) = child {
                    self.exec_operator(graph, ch, budget, outer_rows, stats)?
                } else {
                    vec![Row::new()]
                };

                let mut out = Vec::new();
                for mut row in input {
                    budget.check_or_err()?;

                    // Try to find existing nodes matching ALL labels
                    let mut candidates: Option<HashSet<u64>> = None;
                    for label in labels {
                        let ids = self.store.find_nodes_by_label(graph, label.as_str())?;
                        let id_set: HashSet<u64> = ids.iter().map(|n| n.0).collect();
                        candidates = Some(match candidates {
                            None => id_set,
                            Some(existing) => existing.intersection(&id_set).copied().collect(),
                        });
                    }

                    let matched_id = candidates.and_then(|cands| {
                        // No label filtering means no candidates unless labels is empty
                        if labels.is_empty() {
                            return None;
                        }
                        // Return first candidate (MERGE finds or creates exactly one)
                        cands.into_iter().next().map(NodeId)
                    });

                    if let Some(node_id) = matched_id {
                        // ON MATCH: apply on_match set items
                        if let Some(alias) = node_alias {
                            row.insert(alias.0.clone(), Value::Node(node_id));
                        }
                        for item in on_match {
                            self.apply_set_item(graph, &row, item, stats)?;
                        }
                    } else {
                        // Not found: CREATE the node, then apply ON CREATE set items
                        let label_strings: Vec<String> =
                            labels.iter().map(|l| l.as_str().to_string()).collect();
                        let label_count = label_strings.len() as u64;
                        let node_id =
                            self.store
                                .create_node(graph, label_strings, HashMap::new())?;
                        stats.nodes_created += 1;
                        stats.labels_added += label_count;
                        if let Some(alias) = node_alias {
                            row.insert(alias.0.clone(), Value::Node(node_id));
                        }
                        for item in on_create {
                            self.apply_set_item(graph, &row, item, stats)?;
                        }
                    }
                    budget.record_row();
                    out.push(row);
                }
                Ok(out)
            }

            // ── Unwind ───────────────────────────────────────────────────────────
            Operator::Unwind {
                list_expr,
                alias,
                child,
            } => {
                let input = self.exec_operator(graph, child, budget, outer_rows, stats)?;
                let mut out = Vec::new();

                for row in input {
                    budget.check_or_err()?;
                    let ctx = self.build_filter_ctx(graph, &row)?;
                    let list_val = self.eval_with_storage(graph, list_expr, &row, &ctx)?;
                    match list_val {
                        Value::List(items) => {
                            for item in items {
                                budget.check_or_err()?;
                                let mut new_row = row.clone();
                                new_row.insert(alias.0.clone(), item);
                                budget.record_row();
                                out.push(new_row);
                            }
                        }
                        Value::Null => {
                            // UNWIND null produces zero rows (Cypher semantics)
                        }
                        other => {
                            return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                                expected: "List for UNWIND".to_string(),
                                actual: other.type_name().to_string(),
                            }));
                        }
                    }
                }
                Ok(out)
            }

            // ── Union ────────────────────────────────────────────────────────────
            Operator::Union { left, right, all } => {
                let left_rows = self.exec_operator(graph, left, budget, outer_rows, stats)?;
                let right_rows = self.exec_operator(graph, right, budget, outer_rows, stats)?;

                let mut out = Vec::with_capacity(left_rows.len() + right_rows.len());
                if *all {
                    // UNION ALL: concatenate without deduplication
                    out.extend(left_rows);
                    out.extend(right_rows);
                } else {
                    // UNION: deduplicate
                    let mut seen: HashSet<String> = HashSet::new();
                    for row in left_rows.into_iter().chain(right_rows) {
                        budget.check_or_err()?;
                        let key = row_key(&row);
                        if seen.insert(key) {
                            out.push(row);
                        }
                    }
                }
                Ok(out)
            }

            // ── LeftOuterApply ───────────────────────────────────────────────────
            Operator::LeftOuterApply {
                preserved,
                optional_pattern,
            } => {
                let left_rows = self.exec_operator(graph, preserved, budget, outer_rows, stats)?;
                let mut out = Vec::new();

                for left_row in left_rows {
                    budget.check_or_err()?;
                    // Execute right side with the left row as context
                    let right_rows = self.exec_operator(
                        graph,
                        optional_pattern,
                        budget,
                        std::slice::from_ref(&left_row),
                        stats,
                    )?;

                    if right_rows.is_empty() {
                        // No matches on right side: extend left row with NULLs
                        // for any columns the right side would have produced.
                        let mut result_row = left_row;
                        // Discover right-side columns from the pattern structure
                        let null_cols = discover_operator_aliases(optional_pattern);
                        for col in null_cols {
                            result_row.entry(col).or_insert(Value::Null);
                        }
                        budget.record_row();
                        out.push(result_row);
                    } else {
                        // Combine left and right rows
                        for right_row in right_rows {
                            budget.check_or_err()?;
                            let mut combined = left_row.clone();
                            for (k, v) in right_row {
                                combined.insert(k, v);
                            }
                            budget.record_row();
                            out.push(combined);
                        }
                    }
                }
                Ok(out)
            }

            // ── Apply ────────────────────────────────────────────────────────────
            // Cross-product (inner join) semantics: if the inner side produces
            // no rows for a given outer row, that outer row is dropped entirely.
            // This matches Cypher `MATCH (a) MATCH (b)` behaviour.
            Operator::Apply { outer, inner } => {
                let outer_rows = self.exec_operator(graph, outer, budget, outer_rows, stats)?;
                let mut out = Vec::new();

                for outer_row in &outer_rows {
                    budget.check_or_err()?;
                    let inner_rows = self.exec_operator(
                        graph,
                        inner,
                        budget,
                        std::slice::from_ref(outer_row),
                        stats,
                    )?;

                    for inner_row in inner_rows {
                        budget.check_or_err()?;
                        let mut combined = outer_row.clone();
                        for (k, v) in inner_row {
                            combined.insert(k, v);
                        }
                        budget.record_row();
                        out.push(combined);
                    }
                }
                Ok(out)
            }

            // ── IndexScan ────────────────────────────────────────────────────────
            Operator::IndexScan {
                label,
                property,
                alias,
            } => {
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

            // ── NodeByIdSeek ─────────────────────────────────────────────────────
            Operator::NodeByIdSeek { id_expr, alias } => {
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
                let mut rows = Vec::new();
                if let Some(_node_data) = self.store.get_node(graph, node_id)? {
                    let mut row = Row::new();
                    row.insert(alias.0.clone(), Value::Node(node_id));
                    budget.record_row();
                    rows.push(row);
                }
                Ok(rows)
            }

            // ── ShortestPath ─────────────────────────────────────────────────────
            Operator::ShortestPath {
                src_alias,
                dst_alias,
                rel_types,
                direction,
                min: _min,
                max,
                path_alias,
                all_paths,
            } => {
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

                    // BFS from src to dst, tracking full paths
                    // Queue: (current_node, path_nodes, path_edges)
                    let mut queue: VecDeque<(NodeId, Vec<NodeId>, Vec<EdgeId>)> = VecDeque::new();
                    queue.push_back((src_id, vec![src_id], vec![]));
                    let mut visited = HashSet::new();
                    visited.insert(src_id);

                    let mut found_paths: Vec<(Vec<NodeId>, Vec<EdgeId>)> = Vec::new();
                    let mut shortest_len: Option<u32> = None;

                    while let Some((current, path_nodes, path_edges)) = queue.pop_front() {
                        budget.check_or_err()?;
                        let depth = path_edges.len() as u32;

                        // If we already found shortest paths and current depth exceeds
                        // that length, stop (BFS guarantees depth ordering).
                        if let Some(sl) = shortest_len {
                            if depth > sl {
                                break;
                            }
                        }

                        // Check if we reached the destination
                        if current == dst_id && depth > 0 {
                            found_paths.push((path_nodes.clone(), path_edges.clone()));
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
                            let mut new_nodes = path_nodes.clone();
                            new_nodes.push(adj.neighbor_id);
                            let mut new_edges = path_edges.clone();
                            new_edges.push(adj.edge_id);
                            queue.push_back((adj.neighbor_id, new_nodes, new_edges));
                        }
                    }

                    // Produce output rows for each found path
                    for (pn, pe) in found_paths {
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
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Build a `FilterRow` (borrowed string-key map) from an owned `Row`, hydrating
    /// Node/Edge references into their property maps so that `eval_expr` can handle
    /// property access like `n.name`.
    ///
    /// Each node variable `n` that holds `Value::Node(id)` is supplemented with an
    /// entry `"n.__data__"` carrying `Value::Map(properties)`.  The executor's
    /// `eval_with_storage` uses this to resolve `n.prop` correctly.
    fn build_filter_ctx<'r>(&self, _graph: &str, row: &'r Row) -> Result<FilterRow<'r>, GqlError> {
        // Simple pass-through: the FilterRow borrows directly from `row`.
        // Property resolution for Node/Edge is handled by `eval_with_storage`.
        let ctx: FilterRow<'r> = row.iter().map(|(k, v)| (k.as_str(), v)).collect();
        Ok(ctx)
    }

    /// Evaluate an expression, resolving node/edge property access via the store.
    ///
    /// For `Property(Variable("n"), "name")` where row has `n = Value::Node(id)`:
    /// 1. Fetch the node data from the store.
    /// 2. Return the property value from node data.
    ///
    /// For all other expressions, delegates to `eval_expr`.
    fn eval_with_storage(
        &self,
        graph: &str,
        expr: &Expr,
        row: &Row,
        ctx: &FilterRow<'_>,
    ) -> Result<Value, GqlError> {
        match expr {
            Expr::Property(obj_expr, key) => {
                let obj_val = self.eval_with_storage(graph, obj_expr, row, ctx)?;
                match obj_val {
                    Value::Node(node_id) => match self.store.get_node(graph, node_id)? {
                        Some(node_data) => Ok(node_data
                            .properties
                            .get(key.as_str())
                            .cloned()
                            .unwrap_or(Value::Null)),
                        None => Ok(Value::Null),
                    },
                    Value::Edge(edge_id) => match self.store.get_edge(graph, edge_id)? {
                        Some(edge_data) => Ok(edge_data
                            .properties
                            .get(key.as_str())
                            .cloned()
                            .unwrap_or(Value::Null)),
                        None => Ok(Value::Null),
                    },
                    Value::Map(m) => Ok(m.get(key.as_str()).cloned().unwrap_or(Value::Null)),
                    Value::Null => Ok(Value::Null),
                    // Temporal property access (e.g. d.year, t.hour)
                    Value::Date(_)
                    | Value::LocalTime(_)
                    | Value::LocalDateTime(_)
                    | Value::DateTime(_)
                    | Value::Duration(_) => {
                        crate::executor::filter::temporal_property_access(&obj_val, key.as_str())
                    }
                    other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: "Node, Relationship, or Map".to_string(),
                        actual: other.type_name().to_string(),
                    })),
                }
            }
            // For Variable expressions, look up in row first to handle Node/Edge.
            Expr::Variable(id) => {
                if let Some(v) = row.get(id.0.as_str()) {
                    Ok(v.clone())
                } else {
                    // Fall through to eval_expr for error handling.
                    eval_expr(expr, ctx)
                }
            }
            // For compound expressions, we need to handle nested property access.
            Expr::BinaryOp { op, left, right } => {
                // Evaluate both sides with storage-awareness.
                let lv = self.eval_with_storage(graph, left, row, ctx)?;
                let rv = self.eval_with_storage(graph, right, row, ctx)?;
                // Handle equality/comparison directly for types that value_to_expr
                // can't represent (Node, Edge, Path, temporal, spatial).
                eval_binary_values(*op, lv, rv, ctx)
            }
            Expr::UnaryOp { op, expr: inner } => {
                let v = self.eval_with_storage(graph, inner, row, ctx)?;
                let inner_lit = value_to_expr(&v);
                let rebuilt = Expr::UnaryOp {
                    op: *op,
                    expr: Box::new(inner_lit),
                };
                eval_expr(&rebuilt, ctx)
            }
            Expr::IsNull(inner) => {
                let v = self.eval_with_storage(graph, inner, row, ctx)?;
                Ok(Value::Bool(v.is_null()))
            }
            Expr::IsNotNull(inner) => {
                let v = self.eval_with_storage(graph, inner, row, ctx)?;
                Ok(Value::Bool(!v.is_null()))
            }
            // Label check: n:Label — check if node has the specified label
            Expr::HasLabel(inner, label) => {
                let v = self.eval_with_storage(graph, inner, row, ctx)?;
                match v {
                    Value::Null => Ok(Value::Null),
                    Value::Node(node_id) => {
                        match self.store.get_node(graph, node_id)? {
                            Some(node_data) => {
                                let has = node_data
                                    .labels
                                    .iter()
                                    .any(|l| l == label.as_str());
                                Ok(Value::Bool(has))
                            }
                            None => Ok(Value::Bool(false)),
                        }
                    }
                    _ => Ok(Value::Bool(false)),
                }
            }
            // FunctionCall: evaluate arguments via eval_with_storage, then handle
            // storage-aware functions or delegate to eval_expr with literal args.
            Expr::FunctionCall {
                name,
                args,
                distinct,
            } => {
                // List predicate functions (all/any/none/single) need their
                // ListComp argument evaluated with storage-awareness.
                match name.to_lowercase().as_str() {
                    "all" | "any" | "none" | "single" => {
                        if args.len() == 1 {
                            if let Expr::ListComp {
                                variable,
                                list,
                                filter,
                                ..
                            } = &args[0]
                            {
                                // Evaluate list with storage.
                                let list_val =
                                    self.eval_with_storage(graph, list, row, ctx)?;
                                let items = match list_val {
                                    Value::Null => return Ok(Value::Null),
                                    Value::List(items) => items,
                                    other => {
                                        return Err(GqlError::Execution(
                                            ExecutionError::TypeMismatch {
                                                expected: "List".to_string(),
                                                actual: other.type_name().to_string(),
                                            },
                                        ))
                                    }
                                };
                                let lc = name.to_lowercase();
                                let mut match_count: usize = 0;
                                let total = items.len();
                                for item in &items {
                                    let mut ext_row = row.clone();
                                    ext_row.insert(variable.0.clone(), item.clone());
                                    let ext_ctx =
                                        self.build_filter_ctx(graph, &ext_row)?;
                                    let pred_result = if let Some(pred) = filter {
                                        self.eval_with_storage(
                                            graph, pred, &ext_row, &ext_ctx,
                                        )?
                                    } else {
                                        item.clone()
                                    };
                                    if pred_result == Value::Bool(true) {
                                        match_count += 1;
                                    }
                                    // Short-circuit
                                    match lc.as_str() {
                                        "any" if match_count > 0 => {
                                            return Ok(Value::Bool(true))
                                        }
                                        "none" if match_count > 0 => {
                                            return Ok(Value::Bool(false))
                                        }
                                        "single" if match_count > 1 => {
                                            return Ok(Value::Bool(false))
                                        }
                                        _ => {}
                                    }
                                }
                                return match lc.as_str() {
                                    "all" => Ok(Value::Bool(match_count == total)),
                                    "any" => Ok(Value::Bool(match_count > 0)),
                                    "none" => Ok(Value::Bool(match_count == 0)),
                                    "single" => Ok(Value::Bool(match_count == 1)),
                                    _ => Ok(Value::Bool(false)),
                                };
                            }
                        }
                        // Fallback: delegate to eval_expr
                        return eval_expr(expr, ctx);
                    }
                    "exists" => {
                        if args.len() == 1 {
                            let val =
                                self.eval_with_storage(graph, &args[0], row, ctx)?;
                            return Ok(Value::Bool(!val.is_null()));
                        }
                        return eval_expr(expr, ctx);
                    }
                    _ => {}
                }

                // Evaluate all arguments through eval_with_storage.
                let evaled_args: Result<Vec<Value>, GqlError> = args
                    .iter()
                    .map(|a| self.eval_with_storage(graph, a, row, ctx))
                    .collect();
                let evaled_args = evaled_args?;

                match name.to_lowercase().as_str() {
                    // Storage-aware: check whether a node has a given label.
                    "haslabel" => {
                        if evaled_args.len() < 2 {
                            return Ok(Value::Bool(false));
                        }
                        let label_name = match &evaled_args[1] {
                            Value::Str(s) => s.as_ref().to_string(),
                            _ => return Ok(Value::Bool(false)),
                        };
                        match &evaled_args[0] {
                            Value::Node(node_id) => match self.store.get_node(graph, *node_id)? {
                                Some(data) => {
                                    Ok(Value::Bool(data.labels.iter().any(|l| l == &label_name)))
                                }
                                None => Ok(Value::Bool(false)),
                            },
                            _ => Ok(Value::Bool(false)),
                        }
                    }
                    // Storage-aware: labels(node) → list of label strings
                    "labels" => {
                        if evaled_args.len() != 1 {
                            return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                                expected: "labels() expects 1 argument".to_string(),
                                actual: format!("{} arguments", evaled_args.len()),
                            }));
                        }
                        match &evaled_args[0] {
                            Value::Null => Ok(Value::Null),
                            Value::Node(node_id) => match self.store.get_node(graph, *node_id)? {
                                Some(data) => Ok(Value::List(
                                    data.labels
                                        .iter()
                                        .map(|l| Value::Str(l.clone().into_boxed_str()))
                                        .collect(),
                                )),
                                None => Ok(Value::List(vec![])),
                            },
                            other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                                expected: "Node".to_string(),
                                actual: other.type_name().to_string(),
                            })),
                        }
                    }
                    // Storage-aware: type(relationship) → relationship type string
                    "type" => {
                        if evaled_args.len() != 1 {
                            return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                                expected: "type() expects 1 argument".to_string(),
                                actual: format!("{} arguments", evaled_args.len()),
                            }));
                        }
                        match &evaled_args[0] {
                            Value::Null => Ok(Value::Null),
                            Value::Edge(edge_id) => match self.store.get_edge(graph, *edge_id)? {
                                Some(data) => Ok(Value::Str(data.rel_type.into_boxed_str())),
                                None => Ok(Value::Null),
                            },
                            other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                                expected: "Relationship".to_string(),
                                actual: other.type_name().to_string(),
                            })),
                        }
                    }
                    // Storage-aware: id(node_or_edge) → integer ID
                    "id" => {
                        if evaled_args.len() != 1 {
                            return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                                expected: "id() expects 1 argument".to_string(),
                                actual: format!("{} arguments", evaled_args.len()),
                            }));
                        }
                        match &evaled_args[0] {
                            Value::Null => Ok(Value::Null),
                            Value::Node(node_id) => Ok(Value::Int(node_id.0 as i64)),
                            Value::Edge(edge_id) => Ok(Value::Int(edge_id.0 as i64)),
                            other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                                expected: "Node or Relationship".to_string(),
                                actual: other.type_name().to_string(),
                            })),
                        }
                    }
                    // Storage-aware: properties(node_or_edge) → map of properties
                    "properties" => {
                        if evaled_args.len() != 1 {
                            return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                                expected: "properties() expects 1 argument".to_string(),
                                actual: format!("{} arguments", evaled_args.len()),
                            }));
                        }
                        match &evaled_args[0] {
                            Value::Null => Ok(Value::Null),
                            Value::Node(node_id) => match self.store.get_node(graph, *node_id)? {
                                Some(data) => {
                                    let map: indexmap::IndexMap<Box<str>, Value> = data
                                        .properties
                                        .into_iter()
                                        .map(|(k, v)| (k.into_boxed_str(), v))
                                        .collect();
                                    Ok(Value::Map(map))
                                }
                                None => Ok(Value::Map(indexmap::IndexMap::new())),
                            },
                            Value::Edge(edge_id) => match self.store.get_edge(graph, *edge_id)? {
                                Some(data) => {
                                    let map: indexmap::IndexMap<Box<str>, Value> = data
                                        .properties
                                        .into_iter()
                                        .map(|(k, v)| (k.into_boxed_str(), v))
                                        .collect();
                                    Ok(Value::Map(map))
                                }
                                None => Ok(Value::Map(indexmap::IndexMap::new())),
                            },
                            Value::Map(m) => Ok(Value::Map(m.clone())),
                            other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                                expected: "Node, Relationship, or Map".to_string(),
                                actual: other.type_name().to_string(),
                            })),
                        }
                    }
                    // Storage-aware: keys(node_or_edge) → list of property keys
                    "keys" => {
                        if evaled_args.len() != 1 {
                            return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                                expected: "keys() expects 1 argument".to_string(),
                                actual: format!("{} arguments", evaled_args.len()),
                            }));
                        }
                        match &evaled_args[0] {
                            Value::Null => Ok(Value::Null),
                            Value::Node(node_id) => match self.store.get_node(graph, *node_id)? {
                                Some(data) => Ok(Value::List(
                                    data.properties
                                        .keys()
                                        .map(|k| Value::Str(k.clone().into_boxed_str()))
                                        .collect(),
                                )),
                                None => Ok(Value::List(vec![])),
                            },
                            Value::Edge(edge_id) => match self.store.get_edge(graph, *edge_id)? {
                                Some(data) => Ok(Value::List(
                                    data.properties
                                        .keys()
                                        .map(|k| Value::Str(k.clone().into_boxed_str()))
                                        .collect(),
                                )),
                                None => Ok(Value::List(vec![])),
                            },
                            Value::Map(m) => Ok(Value::List(
                                m.keys().map(|k| Value::Str(k.clone())).collect(),
                            )),
                            other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                                expected: "Node, Relationship, or Map".to_string(),
                                actual: other.type_name().to_string(),
                            })),
                        }
                    }
                    // Storage-aware: nodes(path) → list of nodes in a path
                    "nodes" => {
                        if evaled_args.len() != 1 {
                            return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                                expected: "nodes() expects 1 argument".to_string(),
                                actual: format!("{} arguments", evaled_args.len()),
                            }));
                        }
                        match &evaled_args[0] {
                            Value::Null => Ok(Value::Null),
                            Value::Path { nodes, .. } => {
                                Ok(Value::List(nodes.iter().map(|n| Value::Node(*n)).collect()))
                            }
                            other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                                expected: "Path".to_string(),
                                actual: other.type_name().to_string(),
                            })),
                        }
                    }
                    // Storage-aware: relationships(path) → list of edges in a path
                    "relationships" => {
                        if evaled_args.len() != 1 {
                            return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                                expected: "relationships() expects 1 argument".to_string(),
                                actual: format!("{} arguments", evaled_args.len()),
                            }));
                        }
                        match &evaled_args[0] {
                            Value::Null => Ok(Value::Null),
                            Value::Path { edges, .. } => {
                                Ok(Value::List(edges.iter().map(|e| Value::Edge(*e)).collect()))
                            }
                            other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                                expected: "Path".to_string(),
                                actual: other.type_name().to_string(),
                            })),
                        }
                    }
                    // Storage-aware: length(path) → number of edges
                    "length" => {
                        match evaled_args.first() {
                            Some(Value::Path { edges, .. }) => Ok(Value::Int(edges.len() as i64)),
                            _ => {
                                // Fall through to eval_expr for string/list length
                                let lit_args: Vec<Expr> =
                                    evaled_args.iter().map(value_to_expr).collect();
                                let rebuilt = Expr::FunctionCall {
                                    name: name.clone(),
                                    args: lit_args,
                                    distinct: *distinct,
                                };
                                eval_expr(&rebuilt, ctx)
                            }
                        }
                    }
                    // Storage-aware: startNode(relationship) → source node
                    "startnode" => {
                        if evaled_args.len() != 1 {
                            return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                                expected: "startNode() expects 1 argument".to_string(),
                                actual: format!("{} arguments", evaled_args.len()),
                            }));
                        }
                        match &evaled_args[0] {
                            Value::Null => Ok(Value::Null),
                            Value::Edge(edge_id) => match self.store.get_edge(graph, *edge_id)? {
                                Some(data) => Ok(Value::Node(data.src)),
                                None => Ok(Value::Null),
                            },
                            other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                                expected: "Relationship".to_string(),
                                actual: other.type_name().to_string(),
                            })),
                        }
                    }
                    // Storage-aware: endNode(relationship) → destination node
                    "endnode" => {
                        if evaled_args.len() != 1 {
                            return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                                expected: "endNode() expects 1 argument".to_string(),
                                actual: format!("{} arguments", evaled_args.len()),
                            }));
                        }
                        match &evaled_args[0] {
                            Value::Null => Ok(Value::Null),
                            Value::Edge(edge_id) => match self.store.get_edge(graph, *edge_id)? {
                                Some(data) => Ok(Value::Node(data.dst)),
                                None => Ok(Value::Null),
                            },
                            other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                                expected: "Relationship".to_string(),
                                actual: other.type_name().to_string(),
                            })),
                        }
                    }
                    // All other functions: call directly with evaluated args.
                    _ => eval_function_with_vals(name, evaled_args)
                }
            }
            // Case expression: evaluate with storage-awareness.
            Expr::Case {
                subject,
                alternatives,
                default,
            } => {
                if let Some(subj) = subject {
                    // Value-comparison CASE
                    let subj_val = self.eval_with_storage(graph, subj, row, ctx)?;
                    for (when_expr, then_expr) in alternatives {
                        let when_val = self.eval_with_storage(graph, when_expr, row, ctx)?;
                        if subj_val == when_val {
                            return self.eval_with_storage(graph, then_expr, row, ctx);
                        }
                    }
                } else {
                    // Generic CASE (each WHEN is a predicate)
                    for (when_expr, then_expr) in alternatives {
                        let when_val = self.eval_with_storage(graph, when_expr, row, ctx)?;
                        if when_val == Value::Bool(true) {
                            return self.eval_with_storage(graph, then_expr, row, ctx);
                        }
                    }
                }
                match default {
                    Some(d) => self.eval_with_storage(graph, d, row, ctx),
                    None => Ok(Value::Null),
                }
            }
            // List literal: evaluate each element with storage-awareness.
            Expr::List(items) => {
                let mut result = Vec::with_capacity(items.len());
                for item in items {
                    result.push(self.eval_with_storage(graph, item, row, ctx)?);
                }
                Ok(Value::List(result))
            }
            // Map literal: evaluate each value with storage-awareness.
            Expr::MapLiteral(pairs) => {
                let mut map = indexmap::IndexMap::new();
                for (key, val_expr) in pairs {
                    let val = self.eval_with_storage(graph, val_expr, row, ctx)?;
                    map.insert(key.to_string().into_boxed_str(), val);
                }
                Ok(Value::Map(map))
            }
            // ListComp: evaluate list with storage, then filter/map with storage.
            Expr::ListComp {
                variable,
                list,
                filter,
                map,
            } => {
                let list_val = self.eval_with_storage(graph, list, row, ctx)?;
                match list_val {
                    Value::Null => Ok(Value::Null),
                    Value::List(items) => {
                        let mut result = Vec::with_capacity(items.len());
                        for item in &items {
                            let mut extended_row = row.clone();
                            extended_row.insert(variable.0.clone(), item.clone());
                            let ext_ctx = self.build_filter_ctx(graph, &extended_row)?;
                            if let Some(pred) = filter {
                                match self.eval_with_storage(
                                    graph,
                                    pred,
                                    &extended_row,
                                    &ext_ctx,
                                )? {
                                    Value::Bool(true) => {}
                                    _ => continue,
                                }
                            }
                            result.push(self.eval_with_storage(
                                graph,
                                map,
                                &extended_row,
                                &ext_ctx,
                            )?);
                        }
                        Ok(Value::List(result))
                    }
                    other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: "List".to_string(),
                        actual: other.type_name().to_string(),
                    })),
                }
            }
            // For all other expressions, delegate to eval_expr.
            _ => eval_expr(expr, ctx),
        }
    }

    /// Apply a single `SetItem` (SET n.prop = val, SET n = {map}, SET n += {map}, SET n:Label).
    /// Handles both Node and Edge targets for property operations.
    fn apply_set_item(
        &self,
        graph: &str,
        row: &Row,
        item: &SetItem,
        stats: &mut MutationStats,
    ) -> Result<(), GqlError> {
        match item {
            SetItem::Property { alias, key, value } => {
                let val = row
                    .get(alias.0.as_str())
                    .ok_or_else(|| undefined_var(&alias.0))?;
                let ctx = self.build_filter_ctx(graph, row)?;
                let prop_val = self.eval_with_storage(graph, value, row, &ctx)?;
                let mut props = HashMap::new();
                props.insert(key.clone(), prop_val);
                match val {
                    Value::Node(node_id) => {
                        self.store.set_node_properties(graph, *node_id, props)?;
                    }
                    Value::Edge(edge_id) => {
                        self.store.set_edge_properties(graph, *edge_id, props)?;
                    }
                    other => {
                        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: format!("Node or Relationship for variable '{}'", alias.0),
                            actual: other.type_name().to_string(),
                        }));
                    }
                }
                stats.properties_set += 1;
            }
            SetItem::Map { alias, map, merge } => {
                let val = row
                    .get(alias.0.as_str())
                    .ok_or_else(|| undefined_var(&alias.0))?;
                let ctx = self.build_filter_ctx(graph, row)?;
                let map_val = self.eval_with_storage(graph, map, row, &ctx)?;
                let props = match map_val {
                    Value::Map(m) => m
                        .into_iter()
                        .map(|(k, v)| (k.to_string(), v))
                        .collect::<HashMap<String, Value>>(),
                    Value::Null => HashMap::new(),
                    other => {
                        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: "Map for SET assignment".to_string(),
                            actual: other.type_name().to_string(),
                        }));
                    }
                };
                let prop_count = props.len() as u64;
                match val {
                    Value::Node(node_id) => {
                        if *merge {
                            self.store.set_node_properties(graph, *node_id, props)?;
                        } else {
                            self.store.replace_node_properties(graph, *node_id, props)?;
                        }
                    }
                    Value::Edge(edge_id) => {
                        if *merge {
                            self.store.set_edge_properties(graph, *edge_id, props)?;
                        } else {
                            self.store.replace_edge_properties(graph, *edge_id, props)?;
                        }
                    }
                    other => {
                        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: format!("Node or Relationship for variable '{}'", alias.0),
                            actual: other.type_name().to_string(),
                        }));
                    }
                }
                stats.properties_set += prop_count;
            }
            SetItem::Labels { alias, labels } => {
                let val = row
                    .get(alias.0.as_str())
                    .ok_or_else(|| undefined_var(&alias.0))?;
                let node_id = extract_node_id(val, &alias.0)?;
                let label_strings: Vec<String> =
                    labels.iter().map(|l| l.as_str().to_string()).collect();
                let label_count = label_strings.len() as u64;
                self.store.add_node_labels(graph, node_id, &label_strings)?;
                stats.labels_added += label_count;
            }
        }
        Ok(())
    }
}

// ── Aggregation state ─────────────────────────────────────────────────────────

#[derive(Debug, Default)]
struct AggState {
    count: i64,
    sum: Option<f64>,
    min: Option<Value>,
    max: Option<Value>,
    collected: Vec<Value>,
    seen_distinct: HashSet<String>,
}

impl AggState {
    fn accumulate(&mut self, func: &AggFunction, val: Value, distinct: bool) {
        // For distinct aggregations, skip duplicates.
        if distinct {
            let key = format!("{:?}", val);
            if !self.seen_distinct.insert(key) {
                return;
            }
        }

        match func {
            AggFunction::Count => {
                if !val.is_null() {
                    self.count += 1;
                }
            }
            AggFunction::Sum => {
                if let Some(f) = val.as_float() {
                    self.sum = Some(self.sum.unwrap_or(0.0) + f);
                }
            }
            AggFunction::Avg => {
                if let Some(f) = val.as_float() {
                    self.sum = Some(self.sum.unwrap_or(0.0) + f);
                    self.count += 1;
                }
            }
            AggFunction::Min => {
                if !val.is_null() {
                    self.min = Some(match self.min.take() {
                        None => val,
                        Some(current) => {
                            if cmp_values(&val, &current) == std::cmp::Ordering::Less {
                                val
                            } else {
                                current
                            }
                        }
                    });
                }
            }
            AggFunction::Max => {
                if !val.is_null() {
                    self.max = Some(match self.max.take() {
                        None => val,
                        Some(current) => {
                            if cmp_values(&val, &current) == std::cmp::Ordering::Greater {
                                val
                            } else {
                                current
                            }
                        }
                    });
                }
            }
            AggFunction::Collect => {
                if !val.is_null() {
                    self.collected.push(val);
                }
            }
            // Unsupported aggregation functions: treat as count.
            _ => {
                self.count += 1;
            }
        }
    }

    fn finalize(&self, func: &AggFunction) -> Value {
        match func {
            AggFunction::Count => Value::Int(self.count),
            AggFunction::Sum => self.sum.map(Value::Float).unwrap_or(Value::Int(0)),
            AggFunction::Avg => {
                if self.count == 0 {
                    Value::Null
                } else {
                    Value::Float(self.sum.unwrap_or(0.0) / self.count as f64)
                }
            }
            AggFunction::Min => self.min.clone().unwrap_or(Value::Null),
            AggFunction::Max => self.max.clone().unwrap_or(Value::Null),
            AggFunction::Collect => Value::List(self.collected.clone()),
            _ => Value::Int(self.count),
        }
    }
}

// ── Free helpers ──────────────────────────────────────────────────────────────

/// Extract a `NodeId` from a `Value::Node(id)`, returning an error if the
/// value is not a node reference.
fn extract_node_id(val: &Value, var: &str) -> Result<NodeId, GqlError> {
    match val {
        Value::Node(id) => Ok(*id),
        other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: format!("Node for variable '{var}'"),
            actual: other.type_name().to_string(),
        })),
    }
}

/// Build an error for an undefined variable.
fn undefined_var(name: &str) -> GqlError {
    GqlError::Execution(ExecutionError::TypeMismatch {
        expected: format!("variable '{name}' in scope"),
        actual: "undefined".to_string(),
    })
}

/// Get adjacency entries for a node in the given direction and with the given type filter.
/// Check if the plan's top-level operator (or outermost chain) is a write-only operation.
/// This is used to suppress result rows for queries without RETURN (e.g., `CREATE ()`, `DELETE n`).
/// Note: MERGE is excluded because it can return rows when used with RETURN.
fn is_toplevel_write(op: &Operator) -> bool {
    match op {
        Operator::Create { .. } | Operator::Delete { .. }
        | Operator::Set { .. } | Operator::Remove { .. } => true,
        // Transparent wrappers that don't change write/read semantics
        Operator::Distinct { child }
        | Operator::Sort { child, .. }
        | Operator::Limit { child, .. }
        | Operator::Skip { child, .. } => is_toplevel_write(child),
        _ => false,
    }
}

/// Check if the operator tree contains write operations (CREATE, DELETE, SET, etc.).
fn has_write_operations(op: &Operator) -> bool {
    match op {
        Operator::Create { .. } | Operator::Delete { .. } | Operator::Merge { .. } => true,
        Operator::Set { child, .. }
        | Operator::Remove { child, .. } => true,
        Operator::Distinct { child }
        | Operator::Sort { child, .. }
        | Operator::Limit { child, .. }
        | Operator::Skip { child, .. }
        | Operator::Filter { child, .. }
        | Operator::Project { child, .. }
        | Operator::Unwind { child, .. } => has_write_operations(child),
        Operator::Aggregate { child, .. } => has_write_operations(child),
        Operator::AssemblePath { child, .. } => has_write_operations(child),
        Operator::ProduceResults { child, .. } => has_write_operations(child),
        _ => false,
    }
}

/// Check if the operator tree contains a ProduceResults node (i.e., has a RETURN clause).
fn has_produce_results(op: &Operator) -> bool {
    match op {
        Operator::ProduceResults { .. } => true,
        Operator::Distinct { child }
        | Operator::Sort { child, .. }
        | Operator::Limit { child, .. }
        | Operator::Skip { child, .. }
        | Operator::Filter { child, .. }
        | Operator::Project { child, .. }
        | Operator::Set { child, .. }
        | Operator::Remove { child, .. }
        | Operator::Delete { child, .. }
        | Operator::Unwind { child, .. } => has_produce_results(child),
        Operator::Aggregate { child, .. } => has_produce_results(child),
        Operator::AssemblePath { child, .. } => has_produce_results(child),
        _ => false,
    }
}

fn get_adj_entries(
    store: &dyn GraphStore,
    graph: &str,
    node_id: NodeId,
    direction: &Direction,
    type_strs: &[&str],
) -> Result<Vec<crate::storage::AdjEntry>, GqlError> {
    let entries = match direction {
        Direction::Outgoing => store.out_edges_by_type(graph, node_id, type_strs)?,
        Direction::Incoming => store.in_edges_by_type(graph, node_id, type_strs)?,
        Direction::Undirected => {
            let mut out = store.out_edges_by_type(graph, node_id, type_strs)?;
            let in_ = store.in_edges_by_type(graph, node_id, type_strs)?;
            out.extend(in_);
            out
        }
    };
    Ok(entries)
}

/// Discover the aliases (column names) that an operator tree would produce.
/// Used by LeftOuterApply to fill NULLs for the right side when it produces no rows.
fn discover_operator_aliases(op: &Operator) -> Vec<String> {
    let mut aliases = Vec::new();
    match op {
        Operator::NodeScan { alias, .. } => {
            aliases.push(alias.0.clone());
        }
        Operator::IndexScan { alias, .. } => {
            aliases.push(alias.0.clone());
        }
        Operator::NodeByIdSeek { alias, .. } => {
            aliases.push(alias.0.clone());
        }
        Operator::Expand {
            rel_alias,
            dst_alias,
            child,
            ..
        } => {
            aliases.extend(discover_operator_aliases(child));
            if let Some(ra) = rel_alias {
                aliases.push(ra.0.clone());
            }
            aliases.push(dst_alias.0.clone());
        }
        Operator::VarLenExpand {
            path_alias,
            dst_alias,
            child,
            ..
        } => {
            aliases.extend(discover_operator_aliases(child));
            if let Some(pa) = path_alias {
                aliases.push(pa.0.clone());
            }
            aliases.push(dst_alias.0.clone());
        }
        Operator::Filter { child, .. }
        | Operator::Sort { child, .. }
        | Operator::Limit { child, .. }
        | Operator::Skip { child, .. }
        | Operator::Distinct { child, .. }
        | Operator::Set { child, .. }
        | Operator::Remove { child, .. }
        | Operator::Delete { child, .. }
        | Operator::Unwind { child, .. } => {
            aliases.extend(discover_operator_aliases(child));
        }
        Operator::AssemblePath {
            path_alias, child, ..
        } => {
            aliases.extend(discover_operator_aliases(child));
            aliases.push(path_alias.0.clone());
        }
        Operator::Project { child, .. } => {
            aliases.extend(discover_operator_aliases(child));
        }
        Operator::Aggregate { child, .. } => {
            aliases.extend(discover_operator_aliases(child));
        }
        _ => {}
    }
    aliases
}

/// Generate a deterministic string key for a row, used by `Distinct`.
fn row_key(row: &Row) -> String {
    let mut pairs: Vec<(&str, String)> = row
        .iter()
        .map(|(k, v)| (k.as_str(), format!("{:?}", v)))
        .collect();
    pairs.sort_by_key(|(k, _)| *k);
    pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("|")
}

/// Evaluate a `count` expression (LIMIT/SKIP argument) — must be a non-negative integer.
fn eval_count_expr(expr: &Expr, _row: &Row) -> Result<usize, GqlError> {
    let empty_ctx: FilterRow<'_> = HashMap::new();
    match eval_expr(expr, &empty_ctx)? {
        Value::Int(n) if n >= 0 => Ok(n as usize),
        Value::Int(n) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "non-negative integer for LIMIT/SKIP".to_string(),
            actual: n.to_string(),
        })),
        other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "integer for LIMIT/SKIP".to_string(),
            actual: other.type_name().to_string(),
        })),
    }
}

/// Derive a column name string from an expression.
/// `n.name` → `"n.name"`, `n` → `"n"`, literal → `"literal"`.
fn expr_to_column_name(expr: &Expr) -> String {
    match expr {
        Expr::Variable(id) => id.0.clone(),
        Expr::Property(obj, key) => format!("{}.{}", expr_to_column_name(obj), key),
        Expr::Integer(n) => n.to_string(),
        Expr::Float(f) => f.to_string(),
        Expr::Str(s) => s.to_string(),
        Expr::Bool(b) => b.to_string(),
        Expr::Null => "null".to_string(),
        Expr::FunctionCall { name, args, .. } => {
            let arg_strs: Vec<String> = args.iter().map(expr_to_column_name).collect();
            format!("{}({})", name, arg_strs.join(", "))
        }
        Expr::UnaryOp {
            op: crate::ast::UnaryOp::Neg,
            expr: inner,
        } => format!("-{}", expr_to_column_name(inner)),
        Expr::UnaryOp {
            op: crate::ast::UnaryOp::Not,
            expr: inner,
        } => format!("NOT {}", expr_to_column_name(inner)),
        _ => "expr".to_string(),
    }
}

/// Evaluate a property map `[(key, Expr)]` into a `HashMap<String, Value>`.
fn eval_prop_map(
    graph: &str,
    executor: &QueryExecutor<'_>,
    pairs: &[(String, Expr)],
    row: &Row,
    ctx: &FilterRow<'_>,
) -> Result<HashMap<String, Value>, GqlError> {
    let mut map = HashMap::new();
    for (key, expr) in pairs {
        let val = executor.eval_with_storage(graph, expr, row, ctx)?;
        map.insert(key.clone(), val);
    }
    Ok(map)
}

/// Evaluate a binary operation directly on two already-evaluated `Value`s.
///
/// This handles all types including temporal, Node, Edge which cannot be
/// round-tripped through `value_to_expr` → `eval_expr`.
fn eval_binary_values(
    op: crate::ast::BinaryOp,
    lv: Value,
    rv: Value,
    ctx: &FilterRow<'_>,
) -> Result<Value, GqlError> {
    use crate::ast::BinaryOp;
    // For types that value_to_expr can represent losslessly, delegate to eval_expr.
    let needs_direct = matches!(
        (&lv, &rv),
        (Value::Date(_), _)
            | (_, Value::Date(_))
            | (Value::LocalTime(_), _)
            | (_, Value::LocalTime(_))
            | (Value::LocalDateTime(_), _)
            | (_, Value::LocalDateTime(_))
            | (Value::DateTime(_), _)
            | (_, Value::DateTime(_))
            | (Value::Duration(_), _)
            | (_, Value::Duration(_))
            | (Value::Node(_), _)
            | (_, Value::Node(_))
            | (Value::Edge(_), _)
            | (_, Value::Edge(_))
            | (Value::Path { .. }, _)
            | (_, Value::Path { .. })
    );
    if !needs_direct {
        let left_lit = value_to_expr(&lv);
        let right_lit = value_to_expr(&rv);
        let rebuilt = Expr::BinaryOp {
            op,
            left: Box::new(left_lit),
            right: Box::new(right_lit),
        };
        return eval_expr(&rebuilt, ctx);
    }
    // Handle comparison operators directly for temporal/graph types.
    match op {
        BinaryOp::Eq => Ok(match lv.cypher_eq(&rv) {
            Some(b) => Value::Bool(b),
            None => Value::Null,
        }),
        BinaryOp::Neq => Ok(match lv.cypher_eq(&rv) {
            Some(b) => Value::Bool(!b),
            None => Value::Null,
        }),
        BinaryOp::Lt | BinaryOp::Lte | BinaryOp::Gt | BinaryOp::Gte => {
            if lv.is_null() || rv.is_null() {
                return Ok(Value::Null);
            }
            let ord = cmp_values(&lv, &rv);
            let result = match op {
                BinaryOp::Lt => ord == std::cmp::Ordering::Less,
                BinaryOp::Lte => ord != std::cmp::Ordering::Greater,
                BinaryOp::Gt => ord == std::cmp::Ordering::Greater,
                BinaryOp::Gte => ord != std::cmp::Ordering::Less,
                _ => unreachable!(),
            };
            Ok(Value::Bool(result))
        }
        // Temporal arithmetic: temporal +/- duration, duration +/- duration, duration * int, etc.
        BinaryOp::Add | BinaryOp::Concat => {
            use crate::executor::filter::{temporal_add_duration, duration_add};
            match (&lv, &rv) {
                (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
                (temporal @ (Value::Date(_) | Value::LocalTime(_) | Value::LocalDateTime(_) | Value::DateTime(_)), Value::Duration(dur)) => {
                    temporal_add_duration(temporal, dur, false)
                }
                (Value::Duration(dur), temporal @ (Value::Date(_) | Value::LocalTime(_) | Value::LocalDateTime(_) | Value::DateTime(_))) => {
                    temporal_add_duration(temporal, dur, false)
                }
                (Value::Duration(a), Value::Duration(b)) => Ok(Value::Duration(duration_add(a, b))),
                _ => {
                    let left_lit = value_to_expr(&lv);
                    let right_lit = value_to_expr(&rv);
                    let rebuilt = Expr::BinaryOp { op, left: Box::new(left_lit), right: Box::new(right_lit) };
                    eval_expr(&rebuilt, ctx)
                }
            }
        }
        BinaryOp::Sub => {
            use crate::executor::filter::{temporal_add_duration, duration_sub};
            match (&lv, &rv) {
                (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
                (temporal @ (Value::Date(_) | Value::LocalTime(_) | Value::LocalDateTime(_) | Value::DateTime(_)), Value::Duration(dur)) => {
                    temporal_add_duration(temporal, dur, true)
                }
                (Value::Duration(a), Value::Duration(b)) => Ok(Value::Duration(duration_sub(a, b))),
                _ => {
                    let left_lit = value_to_expr(&lv);
                    let right_lit = value_to_expr(&rv);
                    let rebuilt = Expr::BinaryOp { op, left: Box::new(left_lit), right: Box::new(right_lit) };
                    eval_expr(&rebuilt, ctx)
                }
            }
        }
        BinaryOp::Mul => {
            use crate::executor::filter::duration_mul;
            match (&lv, &rv) {
                (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
                (Value::Duration(d), Value::Int(n)) => Ok(Value::Duration(duration_mul(d, *n))),
                (Value::Int(n), Value::Duration(d)) => Ok(Value::Duration(duration_mul(d, *n))),
                _ => {
                    let left_lit = value_to_expr(&lv);
                    let right_lit = value_to_expr(&rv);
                    let rebuilt = Expr::BinaryOp { op, left: Box::new(left_lit), right: Box::new(right_lit) };
                    eval_expr(&rebuilt, ctx)
                }
            }
        }
        BinaryOp::Div => {
            use crate::executor::filter::duration_div;
            match (&lv, &rv) {
                (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
                (Value::Duration(d), Value::Int(n)) => duration_div(d, *n).map(Value::Duration),
                _ => {
                    let left_lit = value_to_expr(&lv);
                    let right_lit = value_to_expr(&rv);
                    let rebuilt = Expr::BinaryOp { op, left: Box::new(left_lit), right: Box::new(right_lit) };
                    eval_expr(&rebuilt, ctx)
                }
            }
        }
        // For other ops (And, Or, In, etc.) fall through to eval_expr with best-effort conversion.
        _ => {
            let left_lit = value_to_expr(&lv);
            let right_lit = value_to_expr(&rv);
            let rebuilt = Expr::BinaryOp {
                op,
                left: Box::new(left_lit),
                right: Box::new(right_lit),
            };
            eval_expr(&rebuilt, ctx)
        }
    }
}

/// Convert a `Value` back to a literal `Expr` for re-evaluation.
///
/// Used by `eval_with_storage` to handle compound expressions where sub-parts
/// have already been evaluated against storage.
fn value_to_expr(val: &Value) -> Expr {
    match val {
        Value::Null => Expr::Null,
        Value::Bool(b) => Expr::Bool(*b),
        Value::Int(n) => Expr::Integer(*n),
        Value::Float(f) => Expr::Float(*f),
        Value::Str(s) => Expr::Str(s.clone()),
        Value::List(items) => Expr::List(items.iter().map(value_to_expr).collect()),
        Value::Map(m) => Expr::MapLiteral(
            m.iter()
                .map(|(k, v)| (k.as_ref().into(), value_to_expr(v)))
                .collect(),
        ),
        // Node/Edge/Path/Temporal/Spatial: return as a variable with a special sentinel.
        // These should not appear in the middle of a compound expression being rebuilt.
        _ => Expr::Null,
    }
}

/// Compare two `Value`s for ordering purposes.
///
/// Ordering: Null < Bool < Int ≈ Float < String < List < others (opaque).
pub fn cmp_values(a: &Value, b: &Value) -> std::cmp::Ordering {
    use std::cmp::Ordering::*;

    fn type_rank(v: &Value) -> u8 {
        match v {
            Value::Null => 0,
            Value::Bool(_) => 1,
            Value::Int(_) | Value::Float(_) => 2,
            Value::Str(_) => 3,
            Value::Date(_) => 4,
            Value::LocalTime(_) => 5,
            Value::LocalDateTime(_) => 6,
            Value::DateTime(_) => 7,
            Value::List(_) => 8,
            _ => 9,
        }
    }

    let ra = type_rank(a);
    let rb = type_rank(b);
    if ra != rb {
        return ra.cmp(&rb);
    }

    match (a, b) {
        (Value::Null, Value::Null) => Equal,
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        (Value::Int(x), Value::Int(y)) => x.cmp(y),
        (Value::Float(x), Value::Float(y)) => x.partial_cmp(y).unwrap_or(Equal),
        (Value::Int(x), Value::Float(y)) => (*x as f64).partial_cmp(y).unwrap_or(Equal),
        (Value::Float(x), Value::Int(y)) => x.partial_cmp(&(*y as f64)).unwrap_or(Equal),
        (Value::Str(x), Value::Str(y)) => x.cmp(y),
        (Value::Date(x), Value::Date(y)) => x.cmp(y),
        (Value::LocalTime(x), Value::LocalTime(y)) => x.cmp(y),
        (Value::LocalDateTime(x), Value::LocalDateTime(y)) => x.cmp(y),
        (Value::DateTime(x), Value::DateTime(y)) => x.cmp(y),
        (Value::List(x), Value::List(y)) => {
            for (a, b) in x.iter().zip(y.iter()) {
                let ord = cmp_values(a, b);
                if ord != Equal {
                    return ord;
                }
            }
            x.len().cmp(&y.len())
        }
        _ => Equal,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Direction, Expr, Identifier, Label, RelType};
    use crate::executor::budget::ExecutionBudget;
    use crate::planner::{AggFunction, CreatePattern, Operator};
    use crate::storage::InMemoryGraphStore;

    fn make_store() -> InMemoryGraphStore {
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();
        store
    }

    #[test]
    fn node_scan_no_label() {
        let store = make_store();
        store
            .create_node("g", vec!["Person".to_string()], HashMap::new())
            .unwrap();
        store
            .create_node("g", vec!["Movie".to_string()], HashMap::new())
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
    fn node_scan_with_label() {
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
    fn filter_operator() {
        let store = make_store();
        let mut props = HashMap::new();
        props.insert("age".to_string(), Value::Int(30));
        store
            .create_node("g", vec!["Person".to_string()], props)
            .unwrap();
        let mut props2 = HashMap::new();
        props2.insert("age".to_string(), Value::Int(20));
        store
            .create_node("g", vec!["Person".to_string()], props2)
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::Filter {
            predicate: Expr::BinaryOp {
                op: crate::ast::BinaryOp::Gte,
                left: Box::new(Expr::Property(
                    Box::new(Expr::Variable(Identifier::new("n"))),
                    "age".into(),
                )),
                right: Box::new(Expr::Integer(25)),
            },
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("Person")),
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
    }

    #[test]
    fn produce_results_with_property() {
        let store = make_store();
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::Str("Alice".into()));
        store
            .create_node("g", vec!["Person".to_string()], props)
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::ProduceResults {
            columns: vec![(
                "p.name".to_string(),
                Expr::Property(
                    Box::new(Expr::Variable(Identifier::new("p"))),
                    "name".into(),
                ),
            )],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("Person")),
                alias: Identifier::new("p"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0]["p.name"], Value::Str("Alice".into()));
    }

    #[test]
    fn sort_operator() {
        let store = make_store();
        let names = ["Charlie", "Alice", "Bob"];
        for name in &names {
            let mut props = HashMap::new();
            props.insert("name".to_string(), Value::Str((*name).into()));
            store
                .create_node("g", vec!["Person".to_string()], props)
                .unwrap();
        }

        let executor = QueryExecutor::new(&store);
        let plan = Operator::ProduceResults {
            columns: vec![(
                "p.name".to_string(),
                Expr::Property(
                    Box::new(Expr::Variable(Identifier::new("p"))),
                    "name".into(),
                ),
            )],
            child: Box::new(Operator::Sort {
                items: vec![(
                    Expr::Property(
                        Box::new(Expr::Variable(Identifier::new("p"))),
                        "name".into(),
                    ),
                    true,
                )],
                child: Box::new(Operator::NodeScan {
                    label: Some(Label::new("Person")),
                    alias: Identifier::new("p"),
                }),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 3);
        assert_eq!(result.rows[0]["p.name"], Value::Str("Alice".into()));
        assert_eq!(result.rows[1]["p.name"], Value::Str("Bob".into()));
        assert_eq!(result.rows[2]["p.name"], Value::Str("Charlie".into()));
    }

    #[test]
    fn limit_and_skip() {
        let store = make_store();
        for i in 0..5 {
            let mut props = HashMap::new();
            props.insert("i".to_string(), Value::Int(i));
            store.create_node("g", vec![], props).unwrap();
        }

        let executor = QueryExecutor::new(&store);
        let plan = Operator::Limit {
            count: Expr::Integer(2),
            child: Box::new(Operator::Skip {
                count: Expr::Integer(1),
                child: Box::new(Operator::NodeScan {
                    label: None,
                    alias: Identifier::new("n"),
                }),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 2);
    }

    #[test]
    fn expand_operator() {
        let store = make_store();
        let alice = store
            .create_node("g", vec!["Person".to_string()], HashMap::new())
            .unwrap();
        let bob = store
            .create_node("g", vec!["Person".to_string()], HashMap::new())
            .unwrap();
        store
            .create_edge("g", alice, bob, "KNOWS".to_string(), HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::Expand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("KNOWS")],
            src_alias: Identifier::new("a"),
            rel_alias: Some(Identifier::new("r")),
            dst_alias: Identifier::new("b"),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("Person")),
                alias: Identifier::new("a"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        // Only alice has an outgoing KNOWS edge.
        assert_eq!(result.rows.len(), 1);
        assert!(result.rows[0].contains_key("b"));
    }

    #[test]
    fn create_operator() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);

        let plan = Operator::Create {
            patterns: vec![CreatePattern {
                nodes: vec![(
                    Some(Identifier::new("n")),
                    vec![Label::new("Person")],
                    vec![("name".to_string(), Expr::Str("Dave".into()))],
                )],
                edges: vec![],
            }],
            child: None,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.nodes_created, 1);
        // Verify node was actually stored.
        let ids = store.find_nodes_by_label("g", "Person").unwrap();
        assert_eq!(ids.len(), 1);
    }

    #[test]
    fn aggregate_count() {
        let store = make_store();
        for _ in 0..3 {
            store
                .create_node("g", vec!["Person".to_string()], HashMap::new())
                .unwrap();
        }

        let executor = QueryExecutor::new(&store);
        let plan = Operator::ProduceResults {
            columns: vec![(
                "count".to_string(),
                Expr::Variable(Identifier::new("count")),
            )],
            child: Box::new(Operator::Aggregate {
                keys: vec![],
                aggregations: vec![(
                    AggFunction::Count,
                    Expr::Variable(Identifier::new("n")),
                    false,
                    Identifier::new("count"),
                )],
                child: Box::new(Operator::NodeScan {
                    label: Some(Label::new("Person")),
                    alias: Identifier::new("n"),
                }),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0]["count"], Value::Int(3));
    }

    // ── VarLenExpand tests ──────────────────────────────────────────────────

    #[test]
    fn varlen_expand_linear_chain() {
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
        // MATCH (a)-[*1..2]->(b) starting from all N nodes
        let plan = Operator::VarLenExpand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("R")],
            min: 1,
            max: 2,
            src_alias: Identifier::new("a"),
            path_alias: Some(Identifier::new("p")),
            dst_alias: Identifier::new("b"),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("a"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        // From a: depth1->b, depth2->c
        // From b: depth1->c
        // From c: nothing (no outgoing)
        // Total: 3 result rows
        assert_eq!(result.rows.len(), 3);
        // Check that paths are produced
        let paths: Vec<_> = result
            .rows
            .iter()
            .filter(|r| matches!(r.get("p"), Some(Value::Path { .. })))
            .collect();
        assert_eq!(paths.len(), 3);
    }

    #[test]
    fn varlen_expand_min_zero_includes_self() {
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
        // (n)-[*0..1]->(m) from a specific node
        let mut start_row = Row::new();
        start_row.insert("src".to_string(), Value::Node(a));

        let plan = Operator::VarLenExpand {
            direction: Direction::Outgoing,
            rel_types: vec![],
            min: 0,
            max: 1,
            src_alias: Identifier::new("src"),
            path_alias: None,
            dst_alias: Identifier::new("dst"),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("src"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        // From a: depth0->a, depth1->b
        // From b: depth0->b (no outgoing from b in our graph, b has no edges out except from a)
        assert!(result.rows.len() >= 2);
    }

    #[test]
    fn varlen_expand_avoids_cycles() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        // a -> b -> a (cycle)
        store
            .create_edge("g", a, b, "R".to_string(), HashMap::new())
            .unwrap();
        store
            .create_edge("g", b, a, "R".to_string(), HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let mut start_row = Row::new();
        start_row.insert("src".to_string(), Value::Node(a));

        let plan = Operator::VarLenExpand {
            direction: Direction::Outgoing,
            rel_types: vec![],
            min: 1,
            max: 5,
            src_alias: Identifier::new("src"),
            path_alias: None,
            dst_alias: Identifier::new("dst"),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("src"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        // Should terminate and not loop infinitely
        assert!(result.rows.len() > 0);
        assert!(result.rows.len() < 100); // bounded by cycle avoidance
    }

    // ── Set tests ────────────────────────────────────────────────────────────

    #[test]
    fn set_property() {
        let store = make_store();
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::Str("Alice".into()));
        let node_id = store
            .create_node("g", vec!["Person".to_string()], props)
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::Set {
            items: vec![SetItem::Property {
                alias: Identifier::new("n"),
                key: "age".to_string(),
                value: Expr::Integer(30),
            }],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("Person")),
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.properties_set, 1);

        // Verify property was set
        let node = store.get_node("g", node_id).unwrap().unwrap();
        assert_eq!(node.properties.get("age"), Some(&Value::Int(30)));
        assert_eq!(
            node.properties.get("name"),
            Some(&Value::Str("Alice".into()))
        );
    }

    #[test]
    fn set_labels() {
        let store = make_store();
        let node_id = store
            .create_node("g", vec!["Person".to_string()], HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::Set {
            items: vec![SetItem::Labels {
                alias: Identifier::new("n"),
                labels: vec![Label::new("Employee")],
            }],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("Person")),
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.labels_added, 1);

        let node = store.get_node("g", node_id).unwrap().unwrap();
        assert!(node.labels.contains(&"Employee".to_string()));
        assert!(node.labels.contains(&"Person".to_string()));
    }

    #[test]
    fn set_map_merge() {
        let store = make_store();
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::Str("Alice".into()));
        let node_id = store
            .create_node("g", vec!["Person".to_string()], props)
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::Set {
            items: vec![SetItem::Map {
                alias: Identifier::new("n"),
                map: Expr::MapLiteral(vec![(smol_str::SmolStr::new("age"), Expr::Integer(25))]),
                merge: true,
            }],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("Person")),
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.properties_set, 1);

        let node = store.get_node("g", node_id).unwrap().unwrap();
        // Merge preserves existing properties
        assert_eq!(
            node.properties.get("name"),
            Some(&Value::Str("Alice".into()))
        );
        assert_eq!(node.properties.get("age"), Some(&Value::Int(25)));
    }

    // ── Remove tests ─────────────────────────────────────────────────────────

    #[test]
    fn remove_property() {
        let store = make_store();
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::Str("Alice".into()));
        props.insert("age".to_string(), Value::Int(30));
        let node_id = store
            .create_node("g", vec!["Person".to_string()], props)
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::Remove {
            items: vec![RemoveItem::Property {
                alias: Identifier::new("n"),
                key: "age".to_string(),
            }],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("Person")),
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.properties_set, 1);

        let node = store.get_node("g", node_id).unwrap().unwrap();
        assert!(!node.properties.contains_key("age"));
        assert_eq!(
            node.properties.get("name"),
            Some(&Value::Str("Alice".into()))
        );
    }

    #[test]
    fn remove_labels() {
        let store = make_store();
        let node_id = store
            .create_node(
                "g",
                vec!["Person".to_string(), "Employee".to_string()],
                HashMap::new(),
            )
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::Remove {
            items: vec![RemoveItem::Labels {
                alias: Identifier::new("n"),
                labels: vec![Label::new("Employee")],
            }],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("Person")),
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let _result = executor.execute("g", &plan, &mut budget).unwrap();

        let node = store.get_node("g", node_id).unwrap().unwrap();
        assert!(node.labels.contains(&"Person".to_string()));
        assert!(!node.labels.contains(&"Employee".to_string()));
    }

    // ── Delete tests ─────────────────────────────────────────────────────────

    #[test]
    fn delete_node_detach() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["Person".to_string()], HashMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["Person".to_string()], HashMap::new())
            .unwrap();
        store
            .create_edge("g", a, b, "KNOWS".to_string(), HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::Delete {
            exprs: vec![Expr::Variable(Identifier::new("n"))],
            detach: true,
            child: Box::new(Operator::Filter {
                predicate: Expr::Bool(true),
                child: Box::new(Operator::NodeScan {
                    label: Some(Label::new("Person")),
                    alias: Identifier::new("n"),
                }),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.nodes_deleted, 2);
    }

    #[test]
    fn delete_null_is_noop() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        // Build a row with a null and try to delete it
        let plan = Operator::Delete {
            exprs: vec![Expr::Null],
            detach: false,
            child: Box::new(Operator::NodeScan {
                label: None,
                alias: Identifier::new("n"),
            }),
        };
        // Need at least one row for the delete to iterate
        store.create_node("g", vec![], HashMap::new()).unwrap();
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.nodes_deleted, 0);
    }

    #[test]
    fn delete_edge() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        let eid = store
            .create_edge("g", a, b, "R".to_string(), HashMap::new())
            .unwrap();

        // Build a plan that scans and expands, then deletes the edge
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Delete {
            exprs: vec![Expr::Variable(Identifier::new("r"))],
            detach: false,
            child: Box::new(Operator::Expand {
                direction: Direction::Outgoing,
                rel_types: vec![RelType::new("R")],
                src_alias: Identifier::new("a"),
                rel_alias: Some(Identifier::new("r")),
                dst_alias: Identifier::new("b"),
                child: Box::new(Operator::NodeScan {
                    label: Some(Label::new("N")),
                    alias: Identifier::new("a"),
                }),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.edges_deleted, 1);
        assert!(store.get_edge("g", eid).unwrap().is_none());
    }

    // ── Merge tests ──────────────────────────────────────────────────────────

    #[test]
    fn merge_creates_when_not_found() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);

        let plan = Operator::Merge {
            node_alias: Some(Identifier::new("n")),
            labels: vec![Label::new("Person")],
            on_create: vec![SetItem::Property {
                alias: Identifier::new("n"),
                key: "name".to_string(),
                value: Expr::Str("Alice".into()),
            }],
            on_match: vec![],
            child: None,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.nodes_created, 1);
        assert_eq!(result.stats.properties_set, 1);

        // Verify node created
        let ids = store.find_nodes_by_label("g", "Person").unwrap();
        assert_eq!(ids.len(), 1);
        let node = store.get_node("g", ids[0]).unwrap().unwrap();
        assert_eq!(
            node.properties.get("name"),
            Some(&Value::Str("Alice".into()))
        );
    }

    #[test]
    fn merge_matches_existing() {
        let store = make_store();
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::Str("Alice".into()));
        let existing = store
            .create_node("g", vec!["Person".to_string()], props)
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::Merge {
            node_alias: Some(Identifier::new("n")),
            labels: vec![Label::new("Person")],
            on_create: vec![],
            on_match: vec![SetItem::Property {
                alias: Identifier::new("n"),
                key: "seen".to_string(),
                value: Expr::Bool(true),
            }],
            child: None,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.nodes_created, 0);
        assert_eq!(result.stats.properties_set, 1);

        let node = store.get_node("g", existing).unwrap().unwrap();
        assert_eq!(node.properties.get("seen"), Some(&Value::Bool(true)));
    }

    #[test]
    fn merge_without_child() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);

        let plan = Operator::Merge {
            node_alias: Some(Identifier::new("n")),
            labels: vec![Label::new("City")],
            on_create: vec![],
            on_match: vec![],
            child: None,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.nodes_created, 1);
        assert_eq!(result.rows.len(), 1);
        assert!(result.rows[0].contains_key("n"));
    }

    // ── Unwind tests ─────────────────────────────────────────────────────────

    #[test]
    fn unwind_list() {
        let store = make_store();
        store.create_node("g", vec![], HashMap::new()).unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::Unwind {
            list_expr: Expr::List(vec![Expr::Integer(1), Expr::Integer(2), Expr::Integer(3)]),
            alias: Identifier::new("x"),
            child: Box::new(Operator::NodeScan {
                label: None,
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 3);
        let values: Vec<&Value> = result.rows.iter().map(|r| &r["x"]).collect();
        assert_eq!(values, vec![&Value::Int(1), &Value::Int(2), &Value::Int(3)]);
    }

    #[test]
    fn unwind_null_produces_zero_rows() {
        let store = make_store();
        store.create_node("g", vec![], HashMap::new()).unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::Unwind {
            list_expr: Expr::Null,
            alias: Identifier::new("x"),
            child: Box::new(Operator::NodeScan {
                label: None,
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 0);
    }

    #[test]
    fn unwind_empty_list_produces_zero_rows() {
        let store = make_store();
        store.create_node("g", vec![], HashMap::new()).unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::Unwind {
            list_expr: Expr::List(vec![]),
            alias: Identifier::new("x"),
            child: Box::new(Operator::NodeScan {
                label: None,
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 0);
    }

    // ── Union tests ──────────────────────────────────────────────────────────

    #[test]
    fn union_all_concatenates() {
        let store = make_store();
        store
            .create_node("g", vec!["Person".to_string()], HashMap::new())
            .unwrap();
        store
            .create_node("g", vec!["Movie".to_string()], HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::Union {
            left: Box::new(Operator::NodeScan {
                label: Some(Label::new("Person")),
                alias: Identifier::new("n"),
            }),
            right: Box::new(Operator::NodeScan {
                label: Some(Label::new("Movie")),
                alias: Identifier::new("n"),
            }),
            all: true,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 2);
    }

    #[test]
    fn union_deduplicates() {
        let store = make_store();
        let _node = store
            .create_node("g", vec!["Both".to_string()], HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        // Both sides return the same node
        let plan = Operator::Union {
            left: Box::new(Operator::NodeScan {
                label: Some(Label::new("Both")),
                alias: Identifier::new("n"),
            }),
            right: Box::new(Operator::NodeScan {
                label: Some(Label::new("Both")),
                alias: Identifier::new("n"),
            }),
            all: false,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        // Should deduplicate to 1 row
        assert_eq!(result.rows.len(), 1);
    }

    // ── LeftOuterApply tests ─────────────────────────────────────────────────

    #[test]
    fn left_outer_apply_with_match() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["Person".to_string()], HashMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["Person".to_string()], HashMap::new())
            .unwrap();
        store
            .create_edge("g", a, b, "KNOWS".to_string(), HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        // OPTIONAL MATCH (a)-[:KNOWS]->(b) — a has a match, b does not
        let plan = Operator::LeftOuterApply {
            preserved: Box::new(Operator::NodeScan {
                label: Some(Label::new("Person")),
                alias: Identifier::new("a"),
            }),
            optional_pattern: Box::new(Operator::Expand {
                direction: Direction::Outgoing,
                rel_types: vec![RelType::new("KNOWS")],
                src_alias: Identifier::new("a"),
                rel_alias: Some(Identifier::new("r")),
                dst_alias: Identifier::new("friend"),
                child: Box::new(Operator::NodeScan {
                    label: Some(Label::new("Person")),
                    alias: Identifier::new("a"),
                }),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        // 2 Person nodes: a has 1 KNOWS edge, b has 0 KNOWS edges
        // a produces a row with friend, b produces a row with NULL friend
        assert_eq!(result.rows.len(), 2);
    }

    #[test]
    fn left_outer_apply_no_match_produces_nulls() {
        let store = make_store();
        store
            .create_node("g", vec!["Person".to_string()], HashMap::new())
            .unwrap();
        // No edges at all

        let executor = QueryExecutor::new(&store);
        let plan = Operator::LeftOuterApply {
            preserved: Box::new(Operator::NodeScan {
                label: Some(Label::new("Person")),
                alias: Identifier::new("a"),
            }),
            optional_pattern: Box::new(Operator::Expand {
                direction: Direction::Outgoing,
                rel_types: vec![RelType::new("KNOWS")],
                src_alias: Identifier::new("a"),
                rel_alias: None,
                dst_alias: Identifier::new("b"),
                child: Box::new(Operator::NodeScan {
                    label: Some(Label::new("Person")),
                    alias: Identifier::new("a"),
                }),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
        // 'b' should be Null
        assert_eq!(result.rows[0].get("b"), Some(&Value::Null));
    }

    // ── Apply tests ──────────────────────────────────────────────────────────

    #[test]
    fn apply_combines_outer_inner() {
        let store = make_store();
        store
            .create_node("g", vec!["A".to_string()], HashMap::new())
            .unwrap();
        store
            .create_node("g", vec!["B".to_string()], HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::Apply {
            outer: Box::new(Operator::NodeScan {
                label: Some(Label::new("A")),
                alias: Identifier::new("a"),
            }),
            inner: Box::new(Operator::NodeScan {
                label: Some(Label::new("B")),
                alias: Identifier::new("b"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        // 1 outer * 1 inner = 1 combined row
        assert_eq!(result.rows.len(), 1);
        assert!(result.rows[0].contains_key("a"));
        assert!(result.rows[0].contains_key("b"));
    }

    #[test]
    fn apply_empty_inner_passes_outer() {
        let store = make_store();
        store
            .create_node("g", vec!["A".to_string()], HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::Apply {
            outer: Box::new(Operator::NodeScan {
                label: Some(Label::new("A")),
                alias: Identifier::new("a"),
            }),
            inner: Box::new(Operator::NodeScan {
                label: Some(Label::new("NonExistent")),
                alias: Identifier::new("b"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        // Cross-product semantics: inner produces nothing → outer row is dropped.
        assert_eq!(result.rows.len(), 0);
    }

    // ── IndexScan tests ──────────────────────────────────────────────────────

    #[test]
    fn index_scan_finds_nodes() {
        let store = make_store();
        let mut p1 = HashMap::new();
        p1.insert("age".to_string(), Value::Int(30));
        let _a = store
            .create_node("g", vec!["Person".to_string()], p1)
            .unwrap();

        let mut p2 = HashMap::new();
        p2.insert("age".to_string(), Value::Int(25));
        let _b = store
            .create_node("g", vec!["Person".to_string()], p2)
            .unwrap();

        // Create without the age property
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
        // Should find 2 nodes (those with the 'age' property)
        assert_eq!(result.rows.len(), 2);
    }

    #[test]
    fn index_scan_empty_result() {
        let store = make_store();
        store
            .create_node("g", vec!["Person".to_string()], HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::IndexScan {
            label: Label::new("Person"),
            property: "nonexistent".to_string(),
            alias: Identifier::new("n"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 0);
    }

    // ── NodeByIdSeek tests ───────────────────────────────────────────────────

    #[test]
    fn node_by_id_seek_found() {
        let store = make_store();
        let node_id = store
            .create_node("g", vec!["Person".to_string()], HashMap::new())
            .unwrap();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::NodeByIdSeek {
            id_expr: Expr::Integer(node_id.0 as i64),
            alias: Identifier::new("n"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0]["n"], Value::Node(node_id));
    }

    #[test]
    fn node_by_id_seek_not_found() {
        let store = make_store();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::NodeByIdSeek {
            id_expr: Expr::Integer(99999),
            alias: Identifier::new("n"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 0);
    }

    #[test]
    fn node_by_id_seek_negative_id_is_error() {
        let store = make_store();

        let executor = QueryExecutor::new(&store);
        let plan = Operator::NodeByIdSeek {
            id_expr: Expr::Integer(-1),
            alias: Identifier::new("n"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget);
        assert!(result.is_err());
    }

    // ── ShortestPath tests ───────────────────────────────────────────────────

    #[test]
    fn shortest_path_finds_path() {
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
        // Also add a direct edge a->c (shorter path)
        store
            .create_edge("g", a, c, "R".to_string(), HashMap::new())
            .unwrap();

        // We need src and dst bound in context
        let mut start_row = Row::new();
        start_row.insert("src".to_string(), Value::Node(a));
        start_row.insert("dst".to_string(), Value::Node(c));

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
        // The shortest path should be a->c (length 1, not a->b->c length 2)
        match &rows[0]["p"] {
            Value::Path { nodes, edges } => {
                assert_eq!(edges.len(), 1); // shortest path is direct
                assert_eq!(nodes[0], a);
                assert_eq!(nodes[1], c);
            }
            other => panic!("expected Path, got {:?}", other),
        }
    }

    #[test]
    fn shortest_path_all_paths() {
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
        // Two paths of length 2: a->b->c and a->X->c... but let's make two
        // equal-length paths. a->b->c and a->d->c
        let d = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        store
            .create_edge("g", a, b, "R".to_string(), HashMap::new())
            .unwrap();
        store
            .create_edge("g", b, c, "R".to_string(), HashMap::new())
            .unwrap();
        store
            .create_edge("g", a, d, "R".to_string(), HashMap::new())
            .unwrap();
        store
            .create_edge("g", d, c, "R".to_string(), HashMap::new())
            .unwrap();

        let mut start_row = Row::new();
        start_row.insert("src".to_string(), Value::Node(a));
        start_row.insert("dst".to_string(), Value::Node(c));

        let executor = QueryExecutor::new(&store);
        let plan = Operator::ShortestPath {
            src_alias: Identifier::new("src"),
            dst_alias: Identifier::new("dst"),
            rel_types: vec![],
            direction: Direction::Outgoing,
            min: 1,
            max: 10,
            path_alias: Identifier::new("p"),
            all_paths: true,
        };
        let mut budget = ExecutionBudget::unlimited();
        let mut stats = MutationStats::default();
        let rows = executor
            .exec_operator("g", &plan, &mut budget, &[start_row], &mut stats)
            .unwrap();
        // Should find both paths of length 2
        assert_eq!(rows.len(), 2);
        for row in &rows {
            match &row["p"] {
                Value::Path { edges, .. } => assert_eq!(edges.len(), 2),
                other => panic!("expected Path, got {:?}", other),
            }
        }
    }

    #[test]
    fn shortest_path_no_path_exists() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        // No edges at all

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
            max: 5,
            path_alias: Identifier::new("p"),
            all_paths: false,
        };
        let mut budget = ExecutionBudget::unlimited();
        let mut stats = MutationStats::default();
        let rows = executor
            .exec_operator("g", &plan, &mut budget, &[start_row], &mut stats)
            .unwrap();
        assert_eq!(rows.len(), 0);
    }
}
