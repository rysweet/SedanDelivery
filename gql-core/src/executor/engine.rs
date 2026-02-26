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

use std::collections::{HashMap, HashSet};

use crate::ast::{Direction, Expr};
use crate::error::{ExecutionError, GqlError};
use crate::executor::budget::ExecutionBudget;
use crate::executor::filter::eval_expr;
use crate::executor::filter::Row as FilterRow;
use crate::planner::{AggFunction, Operator};
use crate::storage::{GraphStore, NodeId};
use crate::types::Value;

// ── Public types ──────────────────────────────────────────────────────────────

/// A single result row: owned map from column name to value.
pub type Row = HashMap<String, Value>;

/// The final result of executing a query.
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
                        let arg_val = self.eval_with_storage(graph, arg_expr, row, &ctx)?;
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
                            let node_id = self.store.create_node(graph, label_strings, props)?;
                            stats.nodes_created += 1;
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

            // ── ProduceResults ────────────────────────────────────────────────
            Operator::ProduceResults { columns, child } => {
                let input = self.exec_operator(graph, child, budget, outer_rows, stats)?;
                let mut out = Vec::with_capacity(input.len());

                for row in &input {
                    budget.check_or_err()?;
                    let ctx = self.build_filter_ctx(graph, row)?;
                    let mut new_row = Row::new();
                    for (col_name, expr) in columns {
                        let val = self.eval_with_storage(graph, expr, row, &ctx)?;
                        new_row.insert(col_name.clone(), val);
                    }
                    budget.record_row();
                    out.push(new_row);
                }
                Ok(out)
            }

            // ── Unsupported operators ─────────────────────────────────────────
            Operator::VarLenExpand { .. } => Err(unsupported("VarLenExpand")),
            Operator::ShortestPath { .. } => Err(unsupported("ShortestPath")),
            Operator::Merge { .. } => Err(unsupported("Merge")),
            Operator::Apply { .. } => Err(unsupported("Apply")),
            Operator::LeftOuterApply { .. } => Err(unsupported("LeftOuterApply")),
            Operator::Union { .. } => Err(unsupported("Union")),
            Operator::Unwind { .. } => Err(unsupported("Unwind")),
            Operator::Set { .. } => Err(unsupported("Set")),
            Operator::Remove { .. } => Err(unsupported("Remove")),
            Operator::Delete { .. } => Err(unsupported("Delete")),
            Operator::IndexScan { .. } => Err(unsupported("IndexScan")),
            Operator::NodeByIdSeek { .. } => Err(unsupported("NodeByIdSeek")),
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
                // Build an inline expression with literal values so eval_expr can operate on it.
                let left_lit = value_to_expr(&lv);
                let right_lit = value_to_expr(&rv);
                let rebuilt = Expr::BinaryOp {
                    op: *op,
                    left: Box::new(left_lit),
                    right: Box::new(right_lit),
                };
                eval_expr(&rebuilt, ctx)
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
            // FunctionCall: evaluate arguments via eval_with_storage, then handle
            // storage-aware functions or delegate to eval_expr with literal args.
            Expr::FunctionCall {
                name,
                args,
                distinct,
            } => {
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
                    // All other functions: rebuild with literal args and delegate.
                    _ => {
                        let lit_args: Vec<Expr> = evaled_args.iter().map(value_to_expr).collect();
                        let rebuilt = Expr::FunctionCall {
                            name: name.clone(),
                            args: lit_args,
                            distinct: *distinct,
                        };
                        eval_expr(&rebuilt, ctx)
                    }
                }
            }
            // For all other expressions, delegate to eval_expr.
            _ => eval_expr(expr, ctx),
        }
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

/// Return an `UnsupportedFeature` error for the given operator name.
fn unsupported(op: &str) -> GqlError {
    GqlError::Execution(ExecutionError::unsupported(op))
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
        Expr::FunctionCall { name, .. } => name.to_string(),
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
            Value::List(_) => 4,
            _ => 5,
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
}
