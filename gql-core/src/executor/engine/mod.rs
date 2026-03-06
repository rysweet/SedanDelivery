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

mod aggregation;
pub(crate) mod helpers;
mod mutations;
mod scan;
mod shaping;

use std::collections::{HashMap, HashSet};

use crate::ast::Expr;
use crate::error::{ExecutionError, GqlError};
use crate::executor::budget::ExecutionBudget;
use crate::executor::filter::Row as FilterRow;
use crate::executor::filter::{eval_expr, eval_function_with_vals, quantifier_result};
use crate::planner::{Operator, QueryPlanner};
use crate::procedure::ProcedureRegistry;
use crate::storage::{EdgeId, GraphStore, NodeId};
use crate::types::Value;

use helpers::{
    discover_operator_aliases, eval_binary_values, inject_parameters, row_key, value_to_expr,
};

pub use helpers::cmp_values;

// ── Public types ──────────────────────────────────────────────────────────────

/// A single result row: owned map from column name to value.
pub type Row = HashMap<String, Value>;

// ── Private types ─────────────────────────────────────────────────────────────

/// BFS queue entry for `VarLenExpand`: (current_node, path_nodes, path_edges, path_edge_set).
/// `path_edge_set` mirrors `path_edges` as a `HashSet` for O(1) duplicate-edge checks.
pub(crate) type VarLenBfsEntry = (NodeId, Vec<NodeId>, Vec<EdgeId>, HashSet<EdgeId>);

/// The final result of executing a query.
#[derive(Debug)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Row>,
    pub stats: MutationStats,
    /// When a per-row error occurs during streaming evaluation (e.g. division
    /// by zero on one row of an UNWIND), the successfully-computed rows are
    /// returned in `rows` and the error that terminated evaluation is stored
    /// here. This allows Bolt-level streaming: the server can emit RECORD
    /// messages for the partial rows and then emit FAILURE for the trailing
    /// error.
    pub trailing_error: Option<GqlError>,
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
    pub labels_removed: u64,
    /// Internal: distinct label types seen during this execution.
    pub(crate) labels_seen: HashSet<String>,
    /// Internal: label types that existed in the graph before this query.
    pub(crate) pre_existing_labels: HashSet<String>,
    /// Internal: distinct (is_edge, entity_raw_id, prop_key) triples seen.
    pub(crate) props_seen: HashSet<(bool, u64, String)>,
    /// Internal: IDs of nodes created in this query (for net-effect tracking).
    pub(crate) nodes_created_ids: HashSet<u64>,
    /// Internal: IDs of edges created in this query (for net-effect tracking).
    pub(crate) edges_created_ids: HashSet<u64>,
}

impl MutationStats {
    /// Finalise derived counters from the tracking sets.
    /// Must be called once at the end of `execute()`.
    fn finalize(&mut self) {
        self.labels_added = self
            .labels_seen
            .difference(&self.pre_existing_labels)
            .count() as u64;
        self.properties_set = self.props_seen.len() as u64;
    }
}

// ── QueryExecutor ─────────────────────────────────────────────────────────────

/// Executes a physical plan against a [`GraphStore`].
pub struct QueryExecutor<'a> {
    pub(crate) store: &'a dyn GraphStore,
    pub(crate) registry: &'a ProcedureRegistry,
    /// Query parameters stored as `$name` → value.
    pub(crate) parameters: HashMap<String, Value>,
    /// Cache of deleted edge ID → rel_type string.
    /// Allows `type(r)` to work on a relationship deleted in the same query.
    pub(crate) deleted_edge_types: std::cell::RefCell<HashMap<u64, String>>,
    /// When a per-row error occurs in exec_project, the partial rows are
    /// returned as the "successful" result and the error is stashed here so
    /// that `execute()` can attach it to `QueryResult::trailing_error`.
    pub(crate) streaming_error: std::cell::RefCell<Option<GqlError>>,
}

impl<'a> QueryExecutor<'a> {
    /// Create a new executor backed by `store`.
    pub fn new(store: &'a dyn GraphStore) -> Self {
        static EMPTY_REG: std::sync::OnceLock<ProcedureRegistry> = std::sync::OnceLock::new();
        Self {
            store,
            registry: EMPTY_REG.get_or_init(ProcedureRegistry::new),
            parameters: HashMap::new(),
            deleted_edge_types: std::cell::RefCell::new(HashMap::new()),
            streaming_error: std::cell::RefCell::new(None),
        }
    }

    /// Create an executor with a procedure registry and query parameters.
    pub fn with_context(
        store: &'a dyn GraphStore,
        registry: &'a ProcedureRegistry,
        parameters: HashMap<String, Value>,
    ) -> Self {
        // Convert parameter keys to $-prefixed format.
        let params = parameters
            .into_iter()
            .map(|(k, v)| {
                let key = if k.starts_with('$') {
                    k
                } else {
                    format!("${k}")
                };
                (key, v)
            })
            .collect();
        Self {
            store,
            registry,
            parameters: params,
            deleted_edge_types: std::cell::RefCell::new(HashMap::new()),
            streaming_error: std::cell::RefCell::new(None),
        }
    }

    /// Execute `plan` against the named `graph`, enforcing `budget`.
    pub fn execute(
        &self,
        graph: &str,
        plan: &Operator,
        budget: &mut ExecutionBudget,
    ) -> Result<QueryResult, GqlError> {
        // RAII guard: initializes query-level "now" and ensures clear_query_now() is called
        // even if execution returns early via `?`.
        let _query_time_guard = crate::executor::filter::QueryTimeGuard::new();
        let pre_existing_labels: HashSet<String> =
            self.store.all_label_names(graph)?.into_iter().collect();
        let mut stats = MutationStats {
            pre_existing_labels,
            ..Default::default()
        };
        let rows = self.exec_operator(graph, plan, budget, &[], &mut stats)?;
        stats.finalize();

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

        // Check if a per-row streaming error was stashed during execution.
        let trailing_error = self.streaming_error.borrow_mut().take();

        Ok(QueryResult {
            columns,
            rows,
            stats,
            trailing_error,
        })
    }

    // ── Internal dispatch ─────────────────────────────────────────────────────

    /// Recursively execute an operator, returning its output rows.
    #[allow(clippy::only_used_in_recursion)]
    pub(crate) fn exec_operator(
        &self,
        graph: &str,
        op: &Operator,
        budget: &mut ExecutionBudget,
        outer_rows: &[Row],
        stats: &mut MutationStats,
    ) -> Result<Vec<Row>, GqlError> {
        budget.check_or_err()?;

        match op {
            Operator::SingleRow => self.exec_single_row(graph, op, budget, outer_rows, stats),
            Operator::NodeScan { .. } => self.exec_node_scan(graph, op, budget, outer_rows, stats),
            Operator::Filter { .. } => self.exec_filter(graph, op, budget, outer_rows, stats),
            Operator::Expand { .. } => self.exec_expand(graph, op, budget, outer_rows, stats),
            Operator::Project { .. } => self.exec_project(graph, op, budget, outer_rows, stats),
            Operator::Distinct { .. } => self.exec_distinct(graph, op, budget, outer_rows, stats),
            Operator::Sort { .. } => self.exec_sort(graph, op, budget, outer_rows, stats),
            Operator::Limit { .. } => self.exec_limit(graph, op, budget, outer_rows, stats),
            Operator::Skip { .. } => self.exec_skip(graph, op, budget, outer_rows, stats),
            Operator::Aggregate { .. } => self.exec_aggregate(graph, op, budget, outer_rows, stats),
            Operator::Create { .. } => self.exec_create(graph, op, budget, outer_rows, stats),
            Operator::AssemblePath { .. } => {
                self.exec_assemble_path(graph, op, budget, outer_rows, stats)
            }
            Operator::Call { .. } => self.exec_call(graph, op, budget, outer_rows, stats),
            Operator::ProduceResults { .. } => {
                self.exec_produce_results(graph, op, budget, outer_rows, stats)
            }
            Operator::VarLenExpand { .. } => {
                self.exec_var_len_expand(graph, op, budget, outer_rows, stats)
            }
            Operator::Set { .. } => self.exec_set(graph, op, budget, outer_rows, stats),
            Operator::Remove { .. } => self.exec_remove(graph, op, budget, outer_rows, stats),
            Operator::Delete { .. } => self.exec_delete(graph, op, budget, outer_rows, stats),
            Operator::Merge { .. } => self.exec_merge(graph, op, budget, outer_rows, stats),
            Operator::MergeRelationship { .. } => {
                self.exec_merge_rel(graph, op, budget, outer_rows, stats)
            }
            Operator::Unwind { .. } => self.exec_unwind(graph, op, budget, outer_rows, stats),
            Operator::Union { .. } => self.exec_union(graph, op, budget, outer_rows, stats),
            Operator::LeftOuterApply { .. } => {
                self.exec_left_outer_apply(graph, op, budget, outer_rows, stats)
            }
            Operator::Apply { .. } => self.exec_apply(graph, op, budget, outer_rows, stats),
            Operator::IndexScan { .. } => {
                self.exec_index_scan(graph, op, budget, outer_rows, stats)
            }
            Operator::NodeByIdSeek { .. } => {
                self.exec_node_by_id_seek(graph, op, budget, outer_rows, stats)
            }
            Operator::ShortestPath { .. } => {
                self.exec_shortest_path(graph, op, budget, outer_rows, stats)
            }
        }
    }

    // ── Operators kept in mod.rs ──────────────────────────────────────────────

    fn exec_single_row(
        &self,
        _graph: &str,
        _op: &Operator,
        budget: &mut ExecutionBudget,
        _outer_rows: &[Row],
        _stats: &mut MutationStats,
    ) -> Result<Vec<Row>, GqlError> {
        budget.record_row();
        let mut row = Row::new();
        inject_parameters(&mut row, &self.parameters);
        Ok(vec![row])
    }

    fn exec_assemble_path(
        &self,
        graph: &str,
        op: &Operator,
        budget: &mut ExecutionBudget,
        outer_rows: &[Row],
        stats: &mut MutationStats,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::AssemblePath {
            path_alias,
            node_aliases,
            edge_aliases,
            child,
        } = op
        else {
            unreachable!()
        };
        use crate::storage::{EdgeId, NodeId};
        let input = self.exec_operator(graph, child, budget, outer_rows, stats)?;
        let mut out = Vec::with_capacity(input.len());
        for mut row in input {
            budget.check_or_err()?;

            let mut nodes: Vec<NodeId> = Vec::new();
            let mut edges: Vec<EdgeId> = Vec::new();

            let mut fixed_edges_before_varlen: usize = 0;
            let mut varlen_path_found = false;
            let mut varlen_edge_aliases: Vec<String> = Vec::new();
            for ea in edge_aliases {
                match row.get(&ea.0) {
                    Some(Value::Edge(eid)) => {
                        edges.push(*eid);
                        if !varlen_path_found {
                            fixed_edges_before_varlen += 1;
                        }
                    }
                    Some(Value::Path {
                        nodes: pnodes,
                        edges: pedges,
                    }) => {
                        if !varlen_path_found {
                            for na in node_aliases.iter().take(fixed_edges_before_varlen) {
                                if let Some(Value::Node(nid)) = row.get(&na.0) {
                                    nodes.push(*nid);
                                }
                            }
                            nodes.extend_from_slice(pnodes);
                            varlen_path_found = true;
                        } else {
                            nodes.extend_from_slice(&pnodes[1..]);
                        }
                        edges.extend_from_slice(pedges);
                        varlen_edge_aliases.push(ea.0.clone());
                    }
                    Some(Value::List(items)) => {
                        for item in items {
                            if let Value::Edge(eid) = item {
                                edges.push(*eid);
                            }
                        }
                    }
                    _ => {}
                }
            }

            if !varlen_path_found {
                for na in node_aliases {
                    if let Some(Value::Node(nid)) = row.get(&na.0) {
                        nodes.push(*nid);
                    }
                }
            }

            let path_val = Value::Path { nodes, edges };
            row.insert(path_alias.0.clone(), path_val);

            for ea_name in &varlen_edge_aliases {
                if let Some(Value::Path { edges: pedges, .. }) = row.get(ea_name).cloned() {
                    let edge_list: Vec<Value> = pedges.iter().map(|e| Value::Edge(*e)).collect();
                    row.insert(ea_name.clone(), Value::List(edge_list));
                }
            }

            out.push(row);
        }
        Ok(out)
    }

    fn exec_call(
        &self,
        graph: &str,
        op: &Operator,
        budget: &mut ExecutionBudget,
        outer_rows: &[Row],
        stats: &mut MutationStats,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Call {
            procedure,
            args,
            yield_items,
            yield_star,
            standalone,
            child,
        } = op
        else {
            unreachable!()
        };
        let proc_def = self.registry.get(procedure).ok_or_else(|| {
            GqlError::Execution(ExecutionError::unsupported(format!(
                "ProcedureNotFound: {procedure}"
            )))
        })?;

        let input_rows = if let Some(ch) = child {
            self.exec_operator(graph, ch, budget, outer_rows, stats)?
        } else {
            let mut row = Row::new();
            inject_parameters(&mut row, &self.parameters);
            vec![row]
        };

        let mut out = Vec::new();
        for input_row in &input_rows {
            budget.check_or_err()?;

            let ctx = self.build_filter_ctx(graph, input_row)?;
            let mut arg_vals = Vec::with_capacity(args.len());
            for arg_expr in args {
                let val = self.eval_with_storage(graph, arg_expr, input_row, &ctx)?;
                arg_vals.push(val);
            }

            let proc_rows = proc_def.execute(&arg_vals);

            if proc_def.output_columns.is_empty() && child.is_none() {
                // Standalone void procedure: produce no output rows.
            } else if proc_def.output_columns.is_empty() {
                budget.record_row();
                out.push(input_row.clone());
            } else if proc_rows.is_empty() {
                // Procedure returned no rows: drop this input row.
            } else {
                for proc_row in proc_rows {
                    budget.check_or_err()?;
                    let mut new_row = input_row.clone();
                    if !yield_items.is_empty() {
                        for (name, alias) in yield_items {
                            let col_name = alias.as_ref().unwrap_or(name).0.clone();
                            let val = proc_row.get(&name.0).cloned().unwrap_or(Value::Null);
                            new_row.insert(col_name, val);
                        }
                    } else if *standalone || *yield_star {
                        for (k, v) in &proc_row {
                            new_row.insert(k.clone(), v.clone());
                        }
                    }
                    budget.record_row();
                    out.push(new_row);
                }
            }
        }
        Ok(out)
    }

    fn exec_produce_results(
        &self,
        graph: &str,
        op: &Operator,
        budget: &mut ExecutionBudget,
        outer_rows: &[Row],
        stats: &mut MutationStats,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::ProduceResults { columns, child } = op else {
            unreachable!()
        };
        let input = self.exec_operator(graph, child, budget, outer_rows, stats)?;

        if columns.is_empty() {
            return Ok(vec![]);
        }

        let mut out = Vec::with_capacity(input.len());

        let has_star = columns.iter().any(|(name, _)| name == "*");

        macro_rules! try_eval_streaming {
            ($expr_result:expr, $out:ident) => {
                match $expr_result {
                    Ok(val) => val,
                    Err(e) => {
                        if !$out.is_empty() {
                            *self.streaming_error.borrow_mut() = Some(e);
                            return Ok($out);
                        }
                        return Err(e);
                    }
                }
            };
        }

        for row in &input {
            budget.check_or_err()?;
            let ctx = self.build_filter_ctx(graph, row)?;
            let mut new_row = Row::new();

            if has_star {
                for (col_name, expr) in columns {
                    if col_name == "*" {
                        for (var_name, val) in row {
                            if !var_name.starts_with("__") {
                                new_row.insert(var_name.clone(), val.clone());
                            }
                        }
                    } else {
                        let val = try_eval_streaming!(
                            self.eval_with_storage(graph, expr, row, &ctx),
                            out
                        );
                        new_row.insert(col_name.clone(), val);
                    }
                }
            } else {
                for (col_name, expr) in columns {
                    let val =
                        try_eval_streaming!(self.eval_with_storage(graph, expr, row, &ctx), out);
                    new_row.insert(col_name.clone(), val);
                }
            }
            budget.record_row();
            out.push(new_row);
        }
        Ok(out)
    }

    fn exec_unwind(
        &self,
        graph: &str,
        op: &Operator,
        budget: &mut ExecutionBudget,
        outer_rows: &[Row],
        stats: &mut MutationStats,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Unwind {
            list_expr,
            alias,
            child,
        } = op
        else {
            unreachable!()
        };
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

    fn exec_union(
        &self,
        graph: &str,
        op: &Operator,
        budget: &mut ExecutionBudget,
        outer_rows: &[Row],
        stats: &mut MutationStats,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Union { left, right, all } = op else {
            unreachable!()
        };
        let left_rows = self.exec_operator(graph, left, budget, outer_rows, stats)?;
        let right_rows = self.exec_operator(graph, right, budget, outer_rows, stats)?;

        let mut out = Vec::with_capacity(left_rows.len() + right_rows.len());
        if *all {
            out.extend(left_rows);
            out.extend(right_rows);
        } else {
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

    fn exec_left_outer_apply(
        &self,
        graph: &str,
        op: &Operator,
        budget: &mut ExecutionBudget,
        outer_rows: &[Row],
        stats: &mut MutationStats,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::LeftOuterApply {
            preserved,
            optional_pattern,
        } = op
        else {
            unreachable!()
        };
        let left_rows = self.exec_operator(graph, preserved, budget, outer_rows, stats)?;
        let mut out = Vec::new();

        for left_row in left_rows {
            budget.check_or_err()?;
            let right_rows = self.exec_operator(
                graph,
                optional_pattern,
                budget,
                std::slice::from_ref(&left_row),
                stats,
            )?;

            let compatible_rows: Vec<Row> = right_rows
                .into_iter()
                .filter(|right_row| {
                    for (k, v) in right_row {
                        if k.starts_with("__anon") {
                            continue;
                        }
                        if let Some(left_v) = left_row.get(k) {
                            if left_v != v {
                                return false;
                            }
                        }
                    }
                    true
                })
                .collect();

            if compatible_rows.is_empty() {
                let mut result_row = left_row;
                let null_cols = discover_operator_aliases(optional_pattern);
                for col in null_cols {
                    result_row.entry(col).or_insert(Value::Null);
                }
                budget.record_row();
                out.push(result_row);
            } else {
                for right_row in compatible_rows {
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

    fn exec_apply(
        &self,
        graph: &str,
        op: &Operator,
        budget: &mut ExecutionBudget,
        outer_rows: &[Row],
        stats: &mut MutationStats,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Apply { outer, inner } = op else {
            unreachable!()
        };
        let outer_rows = self.exec_operator(graph, outer, budget, outer_rows, stats)?;
        let mut out = Vec::new();

        for outer_row in &outer_rows {
            budget.check_or_err()?;
            let inner_rows =
                self.exec_operator(graph, inner, budget, std::slice::from_ref(outer_row), stats)?;

            for inner_row in inner_rows {
                budget.check_or_err()?;
                let mut compatible = true;
                for (k, v) in &inner_row {
                    if k.starts_with("__anon") {
                        continue;
                    }
                    if let Some(outer_v) = outer_row.get(k) {
                        if outer_v != v {
                            compatible = false;
                            break;
                        }
                    }
                }
                if !compatible {
                    continue;
                }
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

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Build a `FilterRow` (borrowed string-key map) from an owned `Row`.
    pub(crate) fn build_filter_ctx<'r>(
        &self,
        _graph: &str,
        row: &'r Row,
    ) -> Result<FilterRow<'r>, GqlError> {
        let ctx: FilterRow<'r> = row.iter().map(|(k, v)| (k.as_str(), v)).collect();
        Ok(ctx)
    }

    /// Evaluate an expression, resolving node/edge property access via the store.
    pub(crate) fn eval_with_storage(
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
                        None => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: "accessible Node".to_string(),
                            actual: "deleted or missing Node".to_string(),
                        })),
                    },
                    Value::Edge(edge_id) => match self.store.get_edge(graph, edge_id)? {
                        Some(edge_data) => Ok(edge_data
                            .properties
                            .get(key.as_str())
                            .cloned()
                            .unwrap_or(Value::Null)),
                        None => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: "accessible Relationship".to_string(),
                            actual: "deleted or missing Relationship".to_string(),
                        })),
                    },
                    Value::Map(m) => Ok(m.get(key.as_str()).cloned().unwrap_or(Value::Null)),
                    Value::Null => Ok(Value::Null),
                    Value::Date(_)
                    | Value::LocalTime(_)
                    | Value::Time(_, _)
                    | Value::LocalDateTime(_)
                    | Value::DateTime(_)
                    | Value::Duration(_) => {
                        crate::executor::filter::temporal_property_access(&obj_val, key.as_str())
                    }
                    Value::Point(_) => {
                        crate::executor::filter::temporal_property_access(&obj_val, key.as_str())
                    }
                    other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: "Node, Relationship, or Map".to_string(),
                        actual: other.type_name().to_string(),
                    })),
                }
            }
            Expr::Variable(id) => {
                if let Some(v) = row.get(id.0.as_str()) {
                    Ok(v.clone())
                } else {
                    eval_expr(expr, ctx)
                }
            }
            Expr::BinaryOp { op, left, right } => {
                let lv = self.eval_with_storage(graph, left, row, ctx)?;
                let rv = self.eval_with_storage(graph, right, row, ctx)?;
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
            Expr::HasLabel(inner, label) => {
                let mut base_expr: &Expr = inner.as_ref();
                let mut labels_to_check: Vec<&crate::ast::Label> = vec![label];
                while let Expr::HasLabel(inner_inner, inner_label) = base_expr {
                    labels_to_check.push(inner_label);
                    base_expr = inner_inner.as_ref();
                }

                let base_val = self.eval_with_storage(graph, base_expr, row, ctx)?;
                match base_val {
                    Value::Null => Ok(Value::Null),
                    Value::Node(node_id) => match self.store.get_node(graph, node_id)? {
                        Some(node_data) => {
                            let has = labels_to_check
                                .iter()
                                .all(|l| node_data.labels.iter().any(|nl| nl == l.as_str()));
                            Ok(Value::Bool(has))
                        }
                        None => Ok(Value::Bool(false)),
                    },
                    Value::Edge(edge_id) => match self.store.get_edge(graph, edge_id)? {
                        Some(edge_data) => {
                            Ok(Value::Bool(edge_data.rel_type.as_str() == label.as_str()))
                        }
                        None => Ok(Value::Bool(false)),
                    },
                    _ => Ok(Value::Bool(false)),
                }
            }
            Expr::FunctionCall {
                name,
                args,
                distinct,
            } => {
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
                                let list_val = self.eval_with_storage(graph, list, row, ctx)?;
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
                                let mut true_count: usize = 0;
                                let mut false_count: usize = 0;
                                let mut null_count: usize = 0;
                                for item in &items {
                                    let mut ext_row = row.clone();
                                    ext_row.insert(variable.0.clone(), item.clone());
                                    let ext_ctx = self.build_filter_ctx(graph, &ext_row)?;
                                    let pred_result = if let Some(pred) = filter {
                                        self.eval_with_storage(graph, pred, &ext_row, &ext_ctx)?
                                    } else {
                                        item.clone()
                                    };
                                    match pred_result {
                                        Value::Bool(true) => true_count += 1,
                                        Value::Bool(false) => false_count += 1,
                                        v if v.is_null() => null_count += 1,
                                        _ => false_count += 1,
                                    }
                                    match lc.as_str() {
                                        "all" if false_count > 0 => return Ok(Value::Bool(false)),
                                        "any" if true_count > 0 => return Ok(Value::Bool(true)),
                                        "none" if true_count > 0 => return Ok(Value::Bool(false)),
                                        "single" if true_count > 1 => {
                                            return Ok(Value::Bool(false))
                                        }
                                        _ => {}
                                    }
                                }
                                return Ok(quantifier_result(
                                    lc.as_str(),
                                    true_count,
                                    false_count,
                                    null_count,
                                ));
                            }
                        }
                        return eval_expr(expr, ctx);
                    }
                    "exists" => {
                        if args.len() == 1 {
                            let val = self.eval_with_storage(graph, &args[0], row, ctx)?;
                            return Ok(Value::Bool(!val.is_null()));
                        }
                        return eval_expr(expr, ctx);
                    }
                    "__patterncomp" => {
                        if args.len() >= 2 {
                            if let Expr::PatternExpr(patterns) = &args[0] {
                                let map_expr = &args[1];
                                let where_expr = args.get(2);
                                return self.eval_pattern_comprehension(
                                    graph, patterns, map_expr, where_expr, row,
                                );
                            }
                        }
                        return Ok(Value::List(vec![]));
                    }
                    _ => {}
                }

                let evaled_args: Result<Vec<Value>, GqlError> = args
                    .iter()
                    .map(|a| self.eval_with_storage(graph, a, row, ctx))
                    .collect();
                let evaled_args = evaled_args?;

                match name.to_lowercase().as_str() {
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
                    "propertiesmatch" => {
                        if evaled_args.len() < 2 {
                            return Ok(Value::Bool(false));
                        }
                        let param_map = match &evaled_args[1] {
                            Value::Map(m) => m,
                            Value::Null => return Ok(Value::Null),
                            other => {
                                return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                                    expected: "Map for propertiesMatch".to_string(),
                                    actual: other.type_name().to_string(),
                                }));
                            }
                        };
                        let entity_props = match &evaled_args[0] {
                            Value::Node(node_id) => match self.store.get_node(graph, *node_id)? {
                                Some(data) => data.properties.clone(),
                                None => return Ok(Value::Bool(false)),
                            },
                            Value::Edge(edge_id) => match self.store.get_edge(graph, *edge_id)? {
                                Some(data) => data.properties.clone(),
                                None => return Ok(Value::Bool(false)),
                            },
                            _ => return Ok(Value::Bool(false)),
                        };
                        let all_match = param_map.iter().all(|(key, expected)| {
                            entity_props
                                .get(&**key)
                                .map(|v| v == expected)
                                .unwrap_or(false)
                        });
                        Ok(Value::Bool(all_match))
                    }
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
                                None => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                                    expected: "accessible Node".to_string(),
                                    actual: "deleted or missing Node".to_string(),
                                })),
                            },
                            other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                                expected: "Node".to_string(),
                                actual: other.type_name().to_string(),
                            })),
                        }
                    }
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
                                None => {
                                    let cached = self
                                        .deleted_edge_types
                                        .borrow()
                                        .get(&edge_id.0)
                                        .map(|s| Value::Str(s.clone().into_boxed_str()));
                                    Ok(cached.unwrap_or(Value::Null))
                                }
                            },
                            other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
                                expected: "Relationship".to_string(),
                                actual: other.type_name().to_string(),
                            })),
                        }
                    }
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
                                        .iter()
                                        .filter(|(_, v)| !v.is_null())
                                        .map(|(k, _)| Value::Str(k.clone().into_boxed_str()))
                                        .collect(),
                                )),
                                None => Ok(Value::List(vec![])),
                            },
                            Value::Edge(edge_id) => match self.store.get_edge(graph, *edge_id)? {
                                Some(data) => Ok(Value::List(
                                    data.properties
                                        .iter()
                                        .filter(|(_, v)| !v.is_null())
                                        .map(|(k, _)| Value::Str(k.clone().into_boxed_str()))
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
                    "length" => match evaled_args.first() {
                        Some(Value::Path { edges, .. }) => Ok(Value::Int(edges.len() as i64)),
                        _ => {
                            let lit_args: Vec<Expr> =
                                evaled_args.iter().map(value_to_expr).collect();
                            let rebuilt = Expr::FunctionCall {
                                name: name.clone(),
                                args: lit_args,
                                distinct: *distinct,
                            };
                            eval_expr(&rebuilt, ctx)
                        }
                    },
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
                    _ => eval_function_with_vals(name, evaled_args),
                }
            }
            Expr::Case {
                subject,
                alternatives,
                default,
            } => {
                if let Some(subj) = subject {
                    let subj_val = self.eval_with_storage(graph, subj, row, ctx)?;
                    for (when_expr, then_expr) in alternatives {
                        let when_val = self.eval_with_storage(graph, when_expr, row, ctx)?;
                        if subj_val == when_val {
                            return self.eval_with_storage(graph, then_expr, row, ctx);
                        }
                    }
                } else {
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
            Expr::List(items) => {
                let mut result = Vec::with_capacity(items.len());
                for item in items {
                    result.push(self.eval_with_storage(graph, item, row, ctx)?);
                }
                Ok(Value::List(result))
            }
            Expr::MapLiteral(pairs) => {
                let mut map = indexmap::IndexMap::new();
                for (key, val_expr) in pairs {
                    let val = self.eval_with_storage(graph, val_expr, row, ctx)?;
                    map.insert(key.to_string().into_boxed_str(), val);
                }
                Ok(Value::Map(map))
            }
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
            Expr::ExistsSubquery {
                patterns,
                where_clause,
                full_query,
            } => {
                if let Some(inner_query) = full_query {
                    self.eval_full_exists_subquery(graph, inner_query, row)
                } else {
                    self.eval_exists_patterns(graph, patterns, where_clause.as_deref(), row)
                }
            }
            Expr::PatternExpr(patterns) => self.eval_exists_patterns(graph, patterns, None, row),
            Expr::Subscript(obj_expr, idx_expr) => {
                let obj_val = self.eval_with_storage(graph, obj_expr, row, ctx)?;
                let idx_val = self.eval_with_storage(graph, idx_expr, row, ctx)?;
                match (obj_val, idx_val) {
                    (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
                    (Value::Node(node_id), Value::Str(key)) => {
                        match self.store.get_node(graph, node_id)? {
                            Some(node_data) => Ok(node_data
                                .properties
                                .get(key.as_ref())
                                .cloned()
                                .unwrap_or(Value::Null)),
                            None => Ok(Value::Null),
                        }
                    }
                    (Value::Edge(edge_id), Value::Str(key)) => {
                        match self.store.get_edge(graph, edge_id)? {
                            Some(edge_data) => Ok(edge_data
                                .properties
                                .get(key.as_ref())
                                .cloned()
                                .unwrap_or(Value::Null)),
                            None => Ok(Value::Null),
                        }
                    }
                    (Value::List(list), Value::Int(i)) => {
                        let len = list.len() as i64;
                        let real_idx = if i < 0 { len + i } else { i };
                        if real_idx < 0 || real_idx as usize >= list.len() {
                            Ok(Value::Null)
                        } else {
                            Ok(list[real_idx as usize].clone())
                        }
                    }
                    (Value::Map(m), Value::Str(key)) => {
                        Ok(m.get(key.as_ref()).cloned().unwrap_or(Value::Null))
                    }
                    (obj, idx) => {
                        let rebuilt = Expr::Subscript(
                            Box::new(value_to_expr(&obj)),
                            Box::new(value_to_expr(&idx)),
                        );
                        eval_expr(&rebuilt, ctx)
                    }
                }
            }
            _ => eval_expr(expr, ctx),
        }
    }

    fn eval_exists_patterns(
        &self,
        graph: &str,
        patterns: &[crate::ast::PathPattern],
        where_clause: Option<&Expr>,
        outer_row: &Row,
    ) -> Result<Value, GqlError> {
        let planner = QueryPlanner::new(self.store, graph);
        let sub_op = planner.plan_exists_subquery(patterns, where_clause)?;

        let outer = vec![outer_row.clone()];
        let mut budget = ExecutionBudget::default_budget();
        let mut stats = MutationStats::default();
        let sub_rows = self.exec_operator(graph, &sub_op, &mut budget, &outer, &mut stats)?;

        let has_match = sub_rows.iter().any(|sub_row| {
            outer_row
                .iter()
                .filter(|(k, _)| !k.starts_with("__anon"))
                .all(|(k, v)| sub_row.get(k).is_none_or(|sv| sv == v))
        });
        Ok(Value::Bool(has_match))
    }

    fn eval_full_exists_subquery(
        &self,
        graph: &str,
        inner_query: &crate::ast::Query,
        outer_row: &Row,
    ) -> Result<Value, GqlError> {
        let planner =
            QueryPlanner::with_context(self.store, graph, self.registry, &self.parameters);
        let inner_op = planner.plan_unchecked(inner_query)?;
        let outer = vec![outer_row.clone()];
        let mut budget = ExecutionBudget::default_budget();
        let mut stats = MutationStats::default();
        let sub_rows = self.exec_operator(graph, &inner_op, &mut budget, &outer, &mut stats)?;
        let has_match = sub_rows.iter().any(|sub_row| {
            outer_row
                .iter()
                .filter(|(k, _)| !k.starts_with('$') && !k.starts_with("__anon"))
                .all(|(k, v)| sub_row.get(k).is_none_or(|sv| sv == v))
        });
        Ok(Value::Bool(has_match))
    }

    fn eval_pattern_comprehension(
        &self,
        graph: &str,
        patterns: &[crate::ast::PathPattern],
        map_expr: &Expr,
        where_expr: Option<&Expr>,
        outer_row: &Row,
    ) -> Result<Value, GqlError> {
        let planner =
            QueryPlanner::with_context(self.store, graph, self.registry, &self.parameters);
        let sub_op = planner.plan_exists_subquery(patterns, None)?;

        let outer = vec![outer_row.clone()];
        let mut budget = ExecutionBudget::default_budget();
        let mut stats = MutationStats::default();
        let sub_rows = self.exec_operator(graph, &sub_op, &mut budget, &outer, &mut stats)?;

        let mut result = Vec::new();
        for sub_row in &sub_rows {
            let compatible = outer_row
                .iter()
                .filter(|(k, _)| !k.starts_with('$') && !k.starts_with("__anon"))
                .all(|(k, v)| sub_row.get(k.as_str()).is_none_or(|sv| sv == v));
            if !compatible {
                continue;
            }
            if let Some(pred) = where_expr {
                if !matches!(pred, Expr::Bool(true)) {
                    let ctx = self.build_filter_ctx(graph, sub_row)?;
                    match self.eval_with_storage(graph, pred, sub_row, &ctx)? {
                        Value::Bool(true) => {}
                        _ => continue,
                    }
                }
            }
            let ctx = self.build_filter_ctx(graph, sub_row)?;
            result.push(self.eval_with_storage(graph, map_expr, sub_row, &ctx)?);
        }
        Ok(Value::List(result))
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Direction, Expr, Identifier, Label, RelType};
    use crate::executor::budget::ExecutionBudget;
    use crate::planner::{AggFunction, CreatePattern, Operator, SetItem};
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
                    None,
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
        let mut start_row = Row::new();
        start_row.insert("src".to_string(), Value::Node(a));

        let plan = Operator::VarLenExpand {
            direction: Direction::Outgoing,
            rel_types: vec![],
            min: 0,
            max: 1,
            src_alias: Identifier::new("src"),
            path_alias: None,
            path_mode: false,
            dst_alias: Identifier::new("dst"),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("src"),
            }),
            rel_props: None,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
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
            path_mode: false,
            dst_alias: Identifier::new("dst"),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("src"),
            }),
            rel_props: None,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert!(!result.rows.is_empty());
        assert!(result.rows.len() < 100);
    }

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
        assert_eq!(
            node.properties.get("name"),
            Some(&Value::Str("Alice".into()))
        );
        assert_eq!(node.properties.get("age"), Some(&Value::Int(25)));
    }

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
            items: vec![crate::planner::RemoveItem::Property {
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
        assert_eq!(result.stats.properties_set, 0);

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
            items: vec![crate::planner::RemoveItem::Labels {
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
        let plan = Operator::Delete {
            exprs: vec![Expr::Null],
            detach: false,
            child: Box::new(Operator::NodeScan {
                label: None,
                alias: Identifier::new("n"),
            }),
        };
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

    #[test]
    fn merge_creates_when_not_found() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);

        let plan = Operator::Merge {
            node_alias: Some(Identifier::new("n")),
            labels: vec![Label::new("Person")],
            properties: vec![],
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
            properties: vec![],
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
            properties: vec![],
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
        assert_eq!(result.rows.len(), 1);
    }

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
        assert_eq!(result.rows.len(), 2);
    }

    #[test]
    fn left_outer_apply_no_match_produces_nulls() {
        let store = make_store();
        store
            .create_node("g", vec!["Person".to_string()], HashMap::new())
            .unwrap();

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
        assert_eq!(result.rows[0].get("b"), Some(&Value::Null));
    }

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
        assert_eq!(result.rows.len(), 0);
    }

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
        store
            .create_edge("g", a, c, "R".to_string(), HashMap::new())
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
            all_paths: false,
        };
        let mut budget = ExecutionBudget::unlimited();
        let mut stats = MutationStats::default();
        let rows = executor
            .exec_operator("g", &plan, &mut budget, &[start_row], &mut stats)
            .unwrap();
        assert_eq!(rows.len(), 1);
        match &rows[0]["p"] {
            Value::Path { nodes, edges } => {
                assert_eq!(edges.len(), 1);
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

    #[test]
    fn cmp_values_cross_type_ordering() {
        use std::cmp::Ordering;
        assert_eq!(
            cmp_values(&Value::Str("z".into()), &Value::Bool(true)),
            Ordering::Less
        );
        assert_eq!(
            cmp_values(&Value::Bool(false), &Value::Str("a".into())),
            Ordering::Greater
        );
    }

    #[test]
    fn cmp_values_nan_sorts_after_regular_float() {
        use std::cmp::Ordering;
        let nan = Value::Float(f64::NAN);
        let regular = Value::Float(1.0);
        assert_eq!(cmp_values(&nan, &regular), Ordering::Greater);
        assert_eq!(cmp_values(&regular, &nan), Ordering::Less);
    }

    #[test]
    fn cmp_values_list_lexicographic() {
        use std::cmp::Ordering;
        let short = Value::List(vec![Value::Int(1), Value::Int(2)]);
        let long = Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
        let diff = Value::List(vec![Value::Int(1), Value::Int(3)]);
        assert_eq!(cmp_values(&short, &long), Ordering::Less);
        assert_eq!(cmp_values(&short, &diff), Ordering::Less);
        assert_eq!(cmp_values(&short, &short.clone()), Ordering::Equal);
    }

    #[test]
    fn cmp_values_null_sorts_last() {
        use std::cmp::Ordering;
        assert_eq!(cmp_values(&Value::Null, &Value::Int(0)), Ordering::Greater);
        assert_eq!(
            cmp_values(&Value::Null, &Value::Str("z".into())),
            Ordering::Greater
        );
        assert_eq!(cmp_values(&Value::Bool(true), &Value::Null), Ordering::Less);
        assert_eq!(cmp_values(&Value::Null, &Value::Null), Ordering::Equal);
    }

    #[test]
    fn cmp_values_time_utc_normalized() {
        use std::cmp::Ordering;
        let a = Value::Time(jiff::civil::Time::new(10, 35, 0, 0).unwrap(), -28800);
        let d = Value::Time(jiff::civil::Time::new(12, 35, 15, 0).unwrap(), 18000);
        assert_eq!(
            cmp_values(&a, &d),
            Ordering::Greater,
            "A(18:35Z) should be > D(07:35Z)"
        );
        assert_eq!(
            cmp_values(&d, &a),
            Ordering::Less,
            "D(07:35Z) should be < A(18:35Z)"
        );

        let t1 = Value::Time(jiff::civil::Time::new(14, 0, 0, 0).unwrap(), 7200);
        let t2 = Value::Time(jiff::civil::Time::new(13, 0, 0, 0).unwrap(), 3600);
        assert_eq!(cmp_values(&t1, &t2), Ordering::Equal);
    }

    #[test]
    fn e2e_varlen_expand_through_planner() {
        use crate::parser::parse_query;
        use crate::planner::QueryPlanner;

        let store = make_store();
        let setup = "CREATE (a {name: 'A'}), (b {name: 'B'}), (c {name: 'C'}), (d {name: 'D'}) CREATE (a)-[:CONTAINS]->(b), (b)-[:CONTAINS]->(c), (c)-[:CONTAINS]->(d)";
        let ast = parse_query(setup).unwrap();
        let planner = QueryPlanner::new(&store, "g");
        let plan = planner.plan(&ast).unwrap();
        let mut budget = ExecutionBudget::unlimited();
        QueryExecutor::new(&store)
            .execute("g", &plan, &mut budget)
            .unwrap();

        let query = "MATCH (a {name: 'A'})-[*]->(x) RETURN x";
        let ast2 = parse_query(query).unwrap();
        let plan2 = planner.plan(&ast2).unwrap();
        let mut budget2 = ExecutionBudget::unlimited();
        let result = QueryExecutor::new(&store)
            .execute("g", &plan2, &mut budget2)
            .unwrap();
        assert_eq!(result.rows.len(), 3, "Expected 3 rows: B, C, D");
    }

    #[test]
    fn e2e_create_path_variable_returns_path() {
        use crate::parser::parse_query;
        use crate::planner::QueryPlanner;

        let store = make_store();
        let planner = QueryPlanner::new(&store, "g");

        let query = "CREATE p = (a:X)-[r:Y]->(b:Z) RETURN p";
        let ast = parse_query(query).unwrap();
        let plan = planner.plan(&ast).unwrap();
        let mut budget = ExecutionBudget::unlimited();
        let result = QueryExecutor::new(&store)
            .execute("g", &plan, &mut budget)
            .unwrap();

        assert_eq!(
            result.rows.len(),
            1,
            "CREATE should produce exactly one row"
        );

        let row = &result.rows[0];
        let p = row.get("p").expect("row should contain path variable 'p'");
        match p {
            Value::Path { nodes, edges } => {
                assert_eq!(nodes.len(), 2, "path should have 2 nodes");
                assert_eq!(edges.len(), 1, "path should have 1 edge");
            }
            other => panic!("expected Value::Path, got {:?}", other),
        }
    }
}
