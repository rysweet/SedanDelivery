//! Query planner — translates a parsed [`Query`] AST into a physical [`Operator`] tree.
//!
//! ## Strategy
//!
//! The planner walks the clause list left-to-right:
//! 1. `MATCH` → `NodeScan` (or `NodeScan + Expand` for multi-hop patterns) + optional `Filter`.
//! 2. `WHERE` (standalone clause) → `Filter` wrapping current pipeline.
//! 3. `CREATE` → `Create` operator, optionally wrapping a `MATCH` pipeline.
//! 4. `RETURN` → `ProduceResults` wrapping `Project` or `Aggregate`.
//!    - `DISTINCT` → `Distinct` wrapper.
//!    - `ORDER BY` → `Sort` wrapper.
//!    - `SKIP` / `LIMIT` → `Skip` / `Limit` wrappers.
//! 5. `DELETE` / `DETACH DELETE` → `Delete` operator wrapping the current pipeline.
//! 6. `WITH` → mid-query projection/aggregation with optional `Filter`, `Sort`, `Skip`, `Limit`.
//! 7. `UNWIND` → `Unwind` operator for list expansion.
//! 8. `SET` → `Set` operator for property/label mutation.
//! 9. `REMOVE` → `Remove` operator for property/label removal.
//! 10. `MERGE` → `Merge` operator with `ON CREATE SET` / `ON MATCH SET` actions.
//! 11. `OPTIONAL MATCH` → `LeftOuterApply` wrapping the match pipeline.

use std::collections::HashMap;

use crate::ast::{
    CallClause, Clause, CreateClause, DeleteClause, Expr, Identifier, Label, MatchClause,
    MergeClause, PathPattern, PathStep, Query, RemoveClause, RemoveClauseItem, ReturnClause,
    ReturnItem, SetClause, SetClauseItem, UnwindClause, WithClause,
};
use crate::error::{ExecutionError, GqlError, PlanError};
use crate::planner::{AggFunction, CreatePattern, Operator, RemoveItem, SetItem};
use crate::procedure::ProcedureRegistry;
use crate::storage::GraphStore;
use crate::types::Value;

// ── Planner ───────────────────────────────────────────────────────────────────

/// Translates a [`Query`] AST into a physical [`Operator`] tree.
pub struct QueryPlanner<'s> {
    store: &'s dyn GraphStore,
    graph: &'s str,
    registry: &'s ProcedureRegistry,
    parameters: &'s HashMap<String, Value>,
}

impl<'s> QueryPlanner<'s> {
    /// Create a new planner backed by `store` targeting `graph`.
    pub fn new(store: &'s dyn GraphStore, graph: &'s str) -> Self {
        Self {
            store,
            graph,
            registry: empty_registry(),
            parameters: empty_params(),
        }
    }

    /// Create a planner with a procedure registry and query parameters.
    pub fn with_context(
        store: &'s dyn GraphStore,
        graph: &'s str,
        registry: &'s ProcedureRegistry,
        parameters: &'s HashMap<String, Value>,
    ) -> Self {
        Self {
            store,
            graph,
            registry,
            parameters,
        }
    }

    /// Plan a query: produce the root [`Operator`] for execution.
    pub fn plan(&self, query: &Query) -> Result<Operator, GqlError> {
        // Run compile-time validation before planning.
        crate::planner::validator::validate_query(query)?;

        let mut pipeline: Option<Operator> = None;
        let mut return_clause: Option<&ReturnClause> = None;

        for (idx, clause) in query.clauses.iter().enumerate() {
            match clause {
                Clause::Match(mc) => {
                    let op = self.plan_match(mc)?;
                    pipeline = Some(match pipeline {
                        None => op,
                        Some(prev) => {
                            // Chain: apply operator on top of previous pipeline.
                            // Multiple MATCH clauses (whether single- or multi-pattern)
                            // are joined via Apply (cross-product semantics).
                            Operator::Apply {
                                outer: Box::new(prev),
                                inner: Box::new(op),
                            }
                        }
                    });
                }

                Clause::OptionalMatch(mc) => {
                    let match_op = self.plan_match(mc)?;
                    // When no prior pipeline exists, use SingleRow so that
                    // OPTIONAL MATCH still produces one row with NULLs even
                    // on an empty graph.
                    let preserved = pipeline.unwrap_or(Operator::SingleRow);
                    pipeline = Some(Operator::LeftOuterApply {
                        preserved: Box::new(preserved),
                        optional_pattern: Box::new(match_op),
                    });
                }

                Clause::Where(pred) => {
                    let child = pipeline.ok_or_else(|| {
                        GqlError::Plan(PlanError::UndefinedVariable {
                            name: "WHERE without preceding MATCH".to_string(),
                            offset: 0,
                        })
                    })?;
                    pipeline = Some(Operator::Filter {
                        predicate: pred.clone(),
                        child: Box::new(child),
                    });
                }

                Clause::Return(rc) => {
                    return_clause = Some(rc);
                }

                Clause::Create(cc) => {
                    let create_op = self.plan_create(cc, pipeline.take())?;
                    pipeline = Some(create_op);
                }

                Clause::Delete(dc) => {
                    let delete_op = self.plan_delete(dc, pipeline.take())?;
                    pipeline = Some(delete_op);
                }

                Clause::With(wc) => {
                    let with_op = self.plan_with(wc, pipeline.take())?;
                    pipeline = Some(with_op);
                }

                Clause::Unwind(uc) => {
                    let unwind_op = self.plan_unwind(uc, pipeline.take())?;
                    pipeline = Some(unwind_op);
                }

                Clause::Set(sc) => {
                    let set_op = self.plan_set(sc, pipeline.take())?;
                    pipeline = Some(set_op);
                }

                Clause::Remove(rc) => {
                    let remove_op = self.plan_remove(rc, pipeline.take())?;
                    pipeline = Some(remove_op);
                }

                Clause::Merge(mc) => {
                    let merge_op = self.plan_merge(mc, pipeline.take())?;
                    pipeline = Some(merge_op);
                }

                Clause::Call(cc) => {
                    let call_op = self.plan_call(cc, pipeline.take(), idx, query)?;
                    pipeline = Some(call_op);
                }

                Clause::Foreach(_) => {
                    return Err(GqlError::Execution(ExecutionError::unsupported(
                        "FOREACH clause",
                    )));
                }
            }
        }

        // Build the main pipeline result.
        let main_op: Operator = if let Some(rc) = return_clause {
            // RETURN without MATCH: produce exactly one row with no bindings.
            let child = pipeline.unwrap_or(Operator::SingleRow);
            self.plan_return(rc, child)?
        } else if let Some(op) = pipeline {
            // For standalone CALL, derive output columns from the procedure.
            let columns = self.derive_standalone_call_columns(&op);
            Operator::ProduceResults {
                columns,
                child: Box::new(op),
            }
        } else {
            return Err(GqlError::Plan(PlanError::ReturnStarNoVariables));
        };

        // Handle UNION / UNION ALL parts.
        if query.unions.is_empty() {
            return Ok(main_op);
        }

        let mut result = main_op;
        for (all, part_clauses) in &query.unions {
            let part_query = Query {
                clauses: part_clauses.clone(),
                unions: Vec::new(),
            };
            let right = self.plan(&part_query)?;
            result = Operator::Union {
                left: Box::new(result),
                right: Box::new(right),
                all: *all,
            };
        }
        Ok(result)
    }

    // ── CALL planning ─────────────────────────────────────────────────────────

    fn plan_call(
        &self,
        cc: &CallClause,
        child: Option<Operator>,
        clause_idx: usize,
        query: &Query,
    ) -> Result<Operator, GqlError> {
        let has_return = query
            .clauses
            .iter()
            .skip(clause_idx + 1)
            .any(|c| matches!(c, Clause::Return(_)));
        let is_in_query = has_return || child.is_some();

        // YIELD * in in-query mode is an error.
        if cc.yield_star && is_in_query {
            return Err(GqlError::Execution(ExecutionError::unsupported(
                "YIELD * in in-query CALL".to_string(),
            )));
        }

        // Look up the procedure.
        let proc_def = match self.registry.get(&cc.procedure) {
            Some(p) => p,
            None => {
                return Err(GqlError::Execution(ExecutionError::unsupported(format!(
                    "ProcedureNotFound: {}",
                    cc.procedure
                ))));
            }
        };

        // Implicit arguments (no parens) are not allowed in in-query CALL with YIELD.
        if !cc.explicit_args && !cc.yield_items.is_empty() {
            return Err(GqlError::Execution(ExecutionError::unsupported(
                "InvalidArgumentPassingMode: implicit arguments not allowed in in-query CALL"
                    .to_string(),
            )));
        }

        // Resolve arguments.
        let args = if cc.explicit_args {
            // Explicit arguments: validate count.
            if cc.args.len() != proc_def.input_params.len() {
                return Err(GqlError::Execution(ExecutionError::unsupported(format!(
                    "InvalidNumberOfArguments: expected {}, got {}",
                    proc_def.input_params.len(),
                    cc.args.len()
                ))));
            }
            // Basic type checking for literal arguments.
            for (i, arg) in cc.args.iter().enumerate() {
                if let Some(val) = expr_to_literal_value(arg) {
                    if !proc_def.check_arg_type(i, &val) {
                        return Err(GqlError::Execution(ExecutionError::unsupported(
                            "InvalidArgumentType".to_string(),
                        )));
                    }
                }
            }
            cc.args.clone()
        } else {
            // Implicit arguments from query parameters.
            let mut args = Vec::new();
            for param in &proc_def.input_params {
                if self.parameters.contains_key(&param.name) {
                    args.push(Expr::Parameter(param.name.clone()));
                } else {
                    return Err(GqlError::Execution(ExecutionError::unsupported(format!(
                        "MissingParameter: {}",
                        param.name
                    ))));
                }
            }
            args
        };

        // In-query CALL without YIELD: output columns are not exposed.
        // Check if subsequent RETURN references procedure output columns → UndefinedVariable.
        if is_in_query && cc.yield_items.is_empty() && !cc.yield_star && cc.explicit_args {
            let output_names: std::collections::HashSet<&str> = proc_def
                .output_columns
                .iter()
                .map(|c| c.name.as_str())
                .collect();
            if !output_names.is_empty() {
                for future_clause in query.clauses.iter().skip(clause_idx + 1) {
                    if let Clause::Return(rc) = future_clause {
                        for item in &rc.items {
                            if let Expr::Variable(Identifier(name)) = &item.expr {
                                if output_names.contains(name.as_str()) {
                                    return Err(GqlError::Plan(PlanError::UndefinedVariable {
                                        name: name.clone(),
                                        offset: 0,
                                    }));
                                }
                            }
                        }
                    }
                }
            }
        }

        // Check YIELD variable shadowing.
        if !cc.yield_items.is_empty() {
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
            // Collect variables already in scope from the pipeline.
            if let Some(ref ch) = child {
                for alias in crate::planner::collect_visible_aliases(ch) {
                    seen.insert(alias);
                }
            }
            for (name, alias) in &cc.yield_items {
                let effective_name = alias.as_ref().unwrap_or(name).0.clone();
                if !seen.insert(effective_name.clone()) {
                    return Err(GqlError::Execution(ExecutionError::unsupported(format!(
                        "VariableAlreadyBound: {}",
                        effective_name
                    ))));
                }
            }
        }

        // Check for aggregation in arguments (not allowed).
        for arg in &args {
            if contains_aggregation(arg) {
                return Err(GqlError::Execution(ExecutionError::unsupported(
                    "InvalidAggregation: cannot use aggregation in CALL arguments".to_string(),
                )));
            }
        }

        Ok(Operator::Call {
            procedure: cc.procedure.clone(),
            args,
            yield_items: cc.yield_items.clone(),
            yield_star: cc.yield_star,
            standalone: !has_return,
            child: child.map(Box::new),
        })
    }

    /// Derive ProduceResults columns for standalone CALL (no RETURN).
    fn derive_standalone_call_columns(&self, op: &Operator) -> Vec<(String, Expr)> {
        match op {
            Operator::Call {
                procedure,
                yield_items,
                yield_star: _,
                ..
            } => {
                if !yield_items.is_empty() {
                    // YIELD specific columns.
                    yield_items
                        .iter()
                        .map(|(name, alias)| {
                            let col = alias.as_ref().unwrap_or(name);
                            (col.0.clone(), Expr::Variable(col.clone()))
                        })
                        .collect()
                } else if let Some(proc_def) = self.registry.get(procedure) {
                    // Standalone or YIELD * → output all columns.
                    proc_def
                        .output_columns
                        .iter()
                        .map(|col| {
                            (
                                col.name.clone(),
                                Expr::Variable(Identifier::new(col.name.clone())),
                            )
                        })
                        .collect()
                } else {
                    vec![]
                }
            }
            _ => vec![], // Non-CALL: empty columns (same as before for CREATE/DELETE/etc.)
        }
    }

    // ── MATCH planning ────────────────────────────────────────────────────────

    fn plan_match(&self, mc: &MatchClause) -> Result<Operator, GqlError> {
        if mc.patterns.is_empty() {
            return Err(GqlError::Plan(PlanError::ReturnStarNoVariables));
        }

        // Plan each pattern, then join them (multiple patterns = cross-product
        // via Apply for disconnected patterns).
        let mut pattern_ops: Vec<Operator> = mc
            .patterns
            .iter()
            .map(|p| self.plan_path_pattern(p))
            .collect::<Result<Vec<_>, _>>()?;

        let mut op = pattern_ops.remove(0);
        for other in pattern_ops {
            op = Operator::Apply {
                outer: Box::new(op),
                inner: Box::new(other),
            };
        }

        // Append WHERE clause from MatchClause if present.
        if let Some(pred) = &mc.where_clause {
            op = Operator::Filter {
                predicate: pred.clone(),
                child: Box::new(op),
            };
        }

        Ok(op)
    }

    /// Plan an existential subquery: `EXISTS { patterns [WHERE predicate] }`.
    ///
    /// Called by the executor when evaluating `Expr::ExistsSubquery` at runtime.
    /// Returns an operator that produces rows if the patterns match.
    pub fn plan_exists_subquery(
        &self,
        patterns: &[PathPattern],
        where_clause: Option<&Expr>,
    ) -> Result<Operator, GqlError> {
        if patterns.is_empty() {
            return Ok(Operator::SingleRow);
        }
        let mut pattern_ops: Vec<Operator> = patterns
            .iter()
            .map(|p| self.plan_path_pattern(p))
            .collect::<Result<Vec<_>, _>>()?;
        let mut op = pattern_ops.remove(0);
        for other in pattern_ops {
            op = Operator::Apply {
                outer: Box::new(op),
                inner: Box::new(other),
            };
        }
        if let Some(pred) = where_clause {
            op = Operator::Filter {
                predicate: pred.clone(),
                child: Box::new(op),
            };
        }
        Ok(op)
    }

    /// Plan a single `PathPattern` into nested `NodeScan` + `Expand` operators.
    ///
    /// When the pattern has a named path variable (e.g. `p = (a)-[r]->(b)`),
    /// wraps the result with an `AssemblePath` operator that collects node and
    /// edge IDs into a `Value::Path`.
    fn plan_path_pattern(&self, pattern: &PathPattern) -> Result<Operator, GqlError> {
        let has_path_var = pattern.variable.is_some();
        let start = &pattern.start;
        let start_alias = start
            .variable
            .clone()
            .unwrap_or_else(|| Identifier::new("__anon0__"));
        let start_label = start.labels.first().cloned();

        // Track node/edge aliases for path assembly when a path variable is present.
        let mut path_node_aliases: Vec<Identifier> = Vec::new();
        let mut path_edge_aliases: Vec<Identifier> = Vec::new();
        if has_path_var {
            path_node_aliases.push(start_alias.clone());
        }

        // Build the node scan for the first node.
        // If the node pattern has a label and a single equality property filter,
        // check whether a matching index exists. If so, emit IndexScan directly
        // (skipping the post-filter); otherwise fall back to NodeScan + Filter.
        let mut index_consumed_prop: Option<smol_str::SmolStr> = None;
        let mut op = if let (Some(label), Some(props)) = (&start_label, &start.properties) {
            // Look for a single-property equality that maps to a literal value.
            // We take the first one that has an index; remaining props become a Filter.
            let indexes = self.store.list_indexes(self.graph).unwrap_or_default();
            let indexed_prop = props.iter().find(|(prop_key, _val)| {
                indexes
                    .iter()
                    .any(|idx| idx.label == label.as_str() && idx.property == prop_key.as_str())
            });
            if let Some((prop_key, _)) = indexed_prop {
                index_consumed_prop = Some(prop_key.clone());
                Operator::IndexScan {
                    label: label.clone(),
                    property: prop_key.to_string(),
                    alias: start_alias.clone(),
                }
            } else {
                self.plan_node_scan(start_alias.clone(), start_label.clone())
            }
        } else {
            self.plan_node_scan(start_alias.clone(), start_label.clone())
        };

        // Wrap with a property filter for any inline properties that weren't
        // consumed by an IndexScan above.
        if let Some(props) = &start.properties {
            let remaining: Vec<_> = props
                .iter()
                .filter(|(k, _)| {
                    index_consumed_prop
                        .as_ref()
                        .map(|consumed| consumed != k)
                        .unwrap_or(true)
                })
                .cloned()
                .collect();
            if !remaining.is_empty() {
                let pred = props_to_filter_expr(&start_alias, &remaining);
                op = Operator::Filter {
                    predicate: pred,
                    child: Box::new(op),
                };
            }
        }

        // Add label filters for ALL labels beyond the first (which NodeScan handles).
        // Cypher semantics: (n:A:B:C) matches nodes with ALL of labels A, B, C.
        if start.labels.len() > 1 {
            for extra_label in &start.labels[1..] {
                let pred = label_filter_expr(&start_alias, extra_label);
                op = Operator::Filter {
                    predicate: pred,
                    child: Box::new(op),
                };
            }
        }

        // Inline WHERE clause on the start node pattern.
        if let Some(pred) = &start.where_clause {
            op = Operator::Filter {
                predicate: pred.clone(),
                child: Box::new(op),
            };
        }

        // Track the most recent src alias for multi-hop paths.
        let mut current_src = start_alias.clone();

        // Process each step (rel + node) in the path.
        for (step_idx, step) in pattern.steps.iter().enumerate() {
            let PathStep { rel, node } = step;
            let dst_alias = node
                .variable
                .clone()
                .unwrap_or_else(|| Identifier::new(format!("__anon{}__", step_idx + 1)));

            // Variable-length expand.
            if let Some(range) = &rel.range {
                let (min, max) =
                    crate::planner::validate_varlen_bounds(range.min.unwrap_or(1), range.max)?;
                let path_alias =
                    if has_path_var {
                        // Ensure a path alias exists for VarLenExpand when tracking a named path.
                        Some(rel.variable.clone().unwrap_or_else(|| {
                            Identifier::new(format!("__path_rel_{}__", step_idx))
                        }))
                    } else {
                        rel.variable.clone()
                    };
                // For VarLenExpand with a named path, the VarLenExpand itself
                // produces a Path value; the AssemblePath wrapper will merge it.
                if has_path_var {
                    if let Some(ref pa) = path_alias {
                        path_edge_aliases.push(pa.clone());
                    }
                    path_node_aliases.push(dst_alias.clone());
                }
                // Inline property constraints on a variable-length relationship
                // must be checked for every edge in the path during BFS traversal.
                // We pass them as `rel_props` using the sentinel alias `__varlen_edge__`.
                let rel_props_expr = rel.properties.as_ref().filter(|p| !p.is_empty()).map(|props| {
                    props_to_filter_expr(
                        &Identifier::new("__varlen_edge__"),
                        props,
                    )
                });
                op = Operator::VarLenExpand {
                    direction: rel.direction,
                    rel_types: rel.rel_types.clone(),
                    min,
                    max,
                    src_alias: current_src.clone(),
                    path_alias,
                    path_mode: has_path_var,
                    dst_alias: dst_alias.clone(),
                    child: Box::new(op),
                    rel_props: rel_props_expr,
                };
            } else {
                // Single-hop expand.
                // Always assign a rel_alias so that the edge ID is stored in the
                // row, which enables edge isomorphism checking across multi-hop
                // patterns (openCypher spec: each relationship can only be bound
                // once per path traversal). For named paths and inline properties
                // we also need the alias for path assembly / filtering.
                let rel_alias = Some(rel.variable.clone().unwrap_or_else(|| {
                    Identifier::new(format!("__anon_rel_{}__", step_idx))
                }));
                if has_path_var {
                    if let Some(ref ra) = rel_alias {
                        path_edge_aliases.push(ra.clone());
                    }
                    path_node_aliases.push(dst_alias.clone());
                }
                op = Operator::Expand {
                    direction: rel.direction,
                    rel_types: rel.rel_types.clone(),
                    src_alias: current_src.clone(),
                    rel_alias,
                    dst_alias: dst_alias.clone(),
                    child: Box::new(op),
                };
            }

            // If the relationship has inline property constraints, add a property filter.
            // For variable-length patterns, this is handled inside VarLenExpand via
            // rel_props (each traversed edge is checked during BFS), so skip the Filter.
            if let Some(props) = &rel.properties {
                if !props.is_empty() && rel.range.is_none() {
                    // rel_alias is guaranteed to exist because we always assign one above.
                    let filter_alias = rel.variable.clone().unwrap_or_else(|| {
                        Identifier::new(format!("__anon_rel_{}__", step_idx))
                    });
                    let pred = props_to_filter_expr(&filter_alias, props);
                    op = Operator::Filter {
                        predicate: pred,
                        child: Box::new(op),
                    };
                }
            }

            // Inline WHERE clause on the relationship pattern.
            if let Some(pred) = &rel.where_clause {
                op = Operator::Filter {
                    predicate: pred.clone(),
                    child: Box::new(op),
                };
            }

            // If the destination node has label constraints, add label filters for ALL labels.
            // Cypher semantics: (n:A:B:C) matches nodes with ALL of labels A, B, C.
            for dst_label in &node.labels {
                let pred = label_filter_expr(&dst_alias, dst_label);
                op = Operator::Filter {
                    predicate: pred,
                    child: Box::new(op),
                };
            }

            // If the destination node has property constraints, add a property filter.
            if let Some(props) = &node.properties {
                if !props.is_empty() {
                    let pred = props_to_filter_expr(&dst_alias, props);
                    op = Operator::Filter {
                        predicate: pred,
                        child: Box::new(op),
                    };
                }
            }

            // Inline WHERE clause on the destination node pattern.
            if let Some(pred) = &node.where_clause {
                op = Operator::Filter {
                    predicate: pred.clone(),
                    child: Box::new(op),
                };
            }

            // Update current_src for multi-hop patterns.
            current_src = dst_alias;
        }

        // If the pattern has a named path variable, wrap with AssemblePath.
        if let Some(path_var) = &pattern.variable {
            op = Operator::AssemblePath {
                path_alias: path_var.clone(),
                node_aliases: path_node_aliases,
                edge_aliases: path_edge_aliases,
                child: Box::new(op),
            };
        }

        Ok(op)
    }

    fn plan_node_scan(&self, alias: Identifier, label: Option<Label>) -> Operator {
        Operator::NodeScan { label, alias }
    }

    // ── CREATE planning ───────────────────────────────────────────────────────

    fn plan_create(
        &self,
        cc: &CreateClause,
        child: Option<Operator>,
    ) -> Result<Operator, GqlError> {
        let mut patterns = Vec::new();
        let mut anon_counter: usize = 0;

        for (pat_idx, path_pattern) in cc.patterns.iter().enumerate() {
            let mut nodes: Vec<crate::planner::CreateNodeSpec> = Vec::new();
            let mut edges: Vec<crate::planner::CreateEdgeSpec> = Vec::new();

            // Extract nodes from the pattern.
            // Generate unique aliases for anonymous nodes so the executor can
            // track them across node creation and edge creation.
            let start = &path_pattern.start;
            let start_alias = start.variable.clone().unwrap_or_else(|| {
                anon_counter += 1;
                Identifier::new(format!("__anon_{pat_idx}_{anon_counter}__"))
            });
            let node_props = start
                .properties
                .as_ref()
                .map(|p| {
                    p.iter()
                        .map(|(k, v)| (k.to_string(), v.clone()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            nodes.push((Some(start_alias.clone()), start.labels.clone(), node_props));

            // Track the resolved alias for the previous node in the path.
            let mut prev_alias = start_alias.clone();

            for (step_idx, step) in path_pattern.steps.iter().enumerate() {
                let PathStep { rel, node } = step;

                // Destination node — generate alias if anonymous.
                let dst_alias = node.variable.clone().unwrap_or_else(|| {
                    anon_counter += 1;
                    Identifier::new(format!("__anon_{pat_idx}_{anon_counter}__"))
                });
                let dst_props = node
                    .properties
                    .as_ref()
                    .map(|p| {
                        p.iter()
                            .map(|(k, v)| (k.to_string(), v.clone()))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                nodes.push((Some(dst_alias.clone()), node.labels.clone(), dst_props));

                // Edge specification.
                let rel_type = rel.rel_types.first().ok_or_else(|| {
                    GqlError::Plan(PlanError::UndefinedVariable {
                        name: format!("CREATE relationship at step {step_idx} has no type"),
                        offset: 0,
                    })
                })?;

                let src_for_edge = prev_alias.clone();
                let dst_for_edge = dst_alias.clone();

                let rel_props = rel
                    .properties
                    .as_ref()
                    .map(|p| {
                        p.iter()
                            .map(|(k, v)| (k.to_string(), v.clone()))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();

                edges.push((
                    rel.variable.clone(),
                    src_for_edge,
                    dst_for_edge,
                    rel_type.clone(),
                    rel.direction,
                    rel_props,
                ));

                prev_alias = dst_alias;
            }

            patterns.push(CreatePattern { nodes, edges });
        }

        Ok(Operator::Create {
            patterns,
            child: child.map(Box::new),
        })
    }

    // ── DELETE planning ───────────────────────────────────────────────────────

    fn plan_delete(
        &self,
        dc: &DeleteClause,
        child: Option<Operator>,
    ) -> Result<Operator, GqlError> {
        let child = child.ok_or_else(|| {
            GqlError::Plan(PlanError::UndefinedVariable {
                name: "DELETE without preceding MATCH or WITH".to_string(),
                offset: 0,
            })
        })?;

        Ok(Operator::Delete {
            exprs: dc.exprs.clone(),
            detach: dc.detach,
            child: Box::new(child),
        })
    }

    // ── WITH planning ─────────────────────────────────────────────────────────

    /// Plan a WITH clause. WITH is semantically identical to RETURN but feeds
    /// the next clause instead of producing final output.
    ///
    /// WITH a, b WHERE a.age > 25  →  Project then Filter
    /// WITH a, count(b) AS cnt     →  Aggregate
    /// WITH a ORDER BY a.name SKIP 1 LIMIT 5  →  Project + Sort + Skip + Limit
    fn plan_with(&self, wc: &WithClause, child: Option<Operator>) -> Result<Operator, GqlError> {
        let child = child.unwrap_or(Operator::SingleRow);

        // Detect WITH * — if so, skip the projection entirely (passthrough).
        // WITH * means "pass through all row bindings from the previous clause".
        let is_star = wc.items.len() == 1
            && matches!(&wc.items[0].expr, Expr::Variable(id) if id.0 == "*")
            && wc.items[0].alias.is_none();

        let has_aggregation = if is_star {
            false
        } else {
            wc.items.iter().any(|item| has_agg_function(&item.expr))
        };

        // For non-aggregation WITH, Sort/Skip/Limit must come BEFORE Project
        // so that ORDER BY expressions can reference pre-projection variables.
        // For aggregation, Sort/Skip/Limit come after Aggregate (which already
        // produces alias columns).
        let needs_late_sort = has_aggregation || is_star;

        // Projected items (used for non-aggregation WITH).
        let projected_items: Vec<(Expr, Option<Identifier>)> = if !is_star && !has_aggregation {
            wc.items
                .iter()
                .map(|item| (item.expr.clone(), item.alias.clone()))
                .collect()
        } else {
            vec![]
        };

        // Build the projection or aggregation pipeline.
        let mut pipeline = if is_star {
            // WITH * is a passthrough — no projection needed.
            child
        } else if has_aggregation {
            self.plan_with_aggregation(&wc.items, child)?
        } else {
            // For plain projection, Sort/Skip/Limit run on raw rows first.
            // However, if ORDER BY references projected aliases, we need to augment
            // the rows with those aliases before sorting.
            let mut raw = child;

            // Determine if ORDER BY references any projected aliases.
            let order_by_aliases: std::collections::HashSet<String> = wc
                .order_by
                .iter()
                .filter_map(|s| {
                    if let Expr::Variable(v) = &s.expr { Some(v.0.clone()) } else { None }
                })
                .collect();
            let needs_augment_for_sort = !order_by_aliases.is_empty()
                && projected_items.iter().any(|(_, alias)| {
                    alias.as_ref().map(|a| order_by_aliases.contains(a.0.as_str())).unwrap_or(false)
                });

            // If ORDER BY references projected aliases, pre-augment rows.
            if needs_augment_for_sort && wc.where_clause.is_none() {
                let child_aliases = crate::planner::collect_visible_aliases(&raw);
                let mut augmented_items: Vec<(Expr, Option<Identifier>)> = child_aliases
                    .iter()
                    .map(|a| (Expr::Variable(Identifier::new(a.clone())), None))
                    .collect();
                for item in &projected_items {
                    augmented_items.push(item.clone());
                }
                raw = Operator::Project {
                    items: augmented_items,
                    child: Box::new(raw),
                };
            }

            // ORDER BY (can now reference both raw vars and projected aliases).
            if !wc.order_by.is_empty() {
                raw = Operator::Sort {
                    items: wc
                        .order_by
                        .iter()
                        .map(|s| (s.expr.clone(), s.ascending))
                        .collect(),
                    child: Box::new(raw),
                };
            }

            // SKIP.
            if let Some(skip_expr) = &wc.skip {
                raw = Operator::Skip {
                    count: skip_expr.clone(),
                    child: Box::new(raw),
                };
            }

            // LIMIT.
            if let Some(limit_expr) = &wc.limit {
                raw = Operator::Limit {
                    count: limit_expr.clone(),
                    child: Box::new(raw),
                };
            }

            // WHERE in WITH can reference BOTH pre-projection variables AND
            // projected aliases. When there's a WHERE clause, we:
            // 1. First augment raw rows with projected aliases (preserving raw vars)
            // 2. Filter with WHERE (can see both scopes)
            // 3. Final project to restrict to only projected aliases
            if wc.where_clause.is_some() {
                // Augmented project: adds projected aliases while keeping raw vars.
                let child_aliases = crate::planner::collect_visible_aliases(&raw);
                let mut augmented_items: Vec<(Expr, Option<Identifier>)> = child_aliases
                    .iter()
                    .map(|a| (Expr::Variable(Identifier::new(a.clone())), None))
                    .collect();
                for item in &projected_items {
                    augmented_items.push(item.clone());
                }
                raw = Operator::Project {
                    items: augmented_items,
                    child: Box::new(raw),
                };

                // Filter (WHERE) — can see both raw vars and projected aliases.
                raw = Operator::Filter {
                    predicate: wc.where_clause.clone().unwrap(),
                    child: Box::new(raw),
                };

                // Final project: restrict to only projected aliases.
                Operator::Project {
                    items: projected_items.clone(),
                    child: Box::new(raw),
                }
            } else {
                // No WHERE — just project after Sort/Skip/Limit.
                Operator::Project {
                    items: projected_items.clone(),
                    child: Box::new(raw),
                }
            }
        };

        // ORDER BY (for aggregation/star — after projection).
        if needs_late_sort && !wc.order_by.is_empty() {
            let sort_items: Vec<(Expr, bool)> = wc
                .order_by
                .iter()
                .map(|s| {
                    if has_aggregation {
                        (rewrite_with_order_expr(&s.expr, &wc.items), s.ascending)
                    } else {
                        (s.expr.clone(), s.ascending)
                    }
                })
                .collect();
            pipeline = Operator::Sort {
                items: sort_items,
                child: Box::new(pipeline),
            };
        }

        // SKIP (for aggregation/star).
        if needs_late_sort {
            if let Some(skip_expr) = &wc.skip {
                pipeline = Operator::Skip {
                    count: skip_expr.clone(),
                    child: Box::new(pipeline),
                };
            }
        }

        // LIMIT (for aggregation/star).
        if needs_late_sort {
            if let Some(limit_expr) = &wc.limit {
                pipeline = Operator::Limit {
                    count: limit_expr.clone(),
                    child: Box::new(pipeline),
                };
            }
        }

        // DISTINCT.
        if wc.distinct && !has_aggregation {
            pipeline = Operator::Distinct {
                child: Box::new(pipeline),
            };
        }

        // WHERE clause for aggregation/star paths (not handled above).
        if needs_late_sort {
            if let Some(pred) = &wc.where_clause {
                pipeline = Operator::Filter {
                    predicate: pred.clone(),
                    child: Box::new(pipeline),
                };
            }
        }

        Ok(pipeline)
    }

    /// Build an Aggregate operator from WITH items that contain aggregate functions.
    fn plan_with_aggregation(
        &self,
        items: &[ReturnItem],
        child: Operator,
    ) -> Result<Operator, GqlError> {
        let mut keys: Vec<Expr> = Vec::new();
        let mut aggregations: Vec<(AggFunction, Expr, Option<Expr>, bool, Identifier)> = Vec::new();
        // Track key aliases: (generated_column_name, user_alias) pairs.
        let mut key_renames: Vec<(String, String)> = Vec::new();
        let mut post_projections: Vec<(Expr, Option<Identifier>)> = Vec::new();
        let mut needs_post_project = false;
        let mut agg_counter: usize = 0;

        for item in items {
            if has_agg_function(&item.expr) {
                if let Ok((func, arg, second_arg, distinct)) = extract_agg_function(&item.expr) {
                    let alias = item
                        .alias
                        .clone()
                        .unwrap_or_else(|| Identifier::new(return_item_name(item)));
                    aggregations.push((func, arg, second_arg, distinct, alias.clone()));
                    post_projections.push((Expr::Variable(alias), None));
                } else {
                    needs_post_project = true;
                    let temp_alias = Identifier::new(format!("__agg_{agg_counter}"));
                    agg_counter += 1;
                    if let Some((func, arg, second_arg, distinct)) =
                        find_inner_aggregate(&item.expr)
                    {
                        aggregations.push((func, arg, second_arg, distinct, temp_alias.clone()));
                        let rewritten = rewrite_agg_to_var(&item.expr, &temp_alias);
                        let final_alias = item
                            .alias
                            .clone()
                            .unwrap_or_else(|| Identifier::new(return_item_name(item)));
                        post_projections.push((rewritten, Some(final_alias)));
                    } else {
                        keys.push(item.expr.clone());
                        let alias = item
                            .alias
                            .clone()
                            .unwrap_or_else(|| Identifier::new(return_item_name(item)));
                        post_projections.push((Expr::Variable(alias), None));
                    }
                }
            } else {
                let gen_name = expr_to_col_name(&item.expr);
                if let Some(a) = &item.alias {
                    if a.0 != gen_name {
                        key_renames.push((gen_name.clone(), a.0.clone()));
                    }
                }
                keys.push(item.expr.clone());
                post_projections.push((
                    Expr::Variable(Identifier::new(gen_name)),
                    item.alias.clone(),
                ));
            }
        }

        let agg = Operator::Aggregate {
            keys,
            aggregations,
            child: Box::new(child),
        };

        // If post-project needed (complex agg expressions) or key renames needed
        if needs_post_project || !key_renames.is_empty() {
            let project_items: Vec<(Expr, Option<Identifier>)> = if needs_post_project {
                post_projections
            } else {
                // Simple rename case
                let mut items_out = Vec::new();
                for item in items {
                    let name = return_item_name(item);
                    if has_agg_function(&item.expr) {
                        let agg_alias = item
                            .alias
                            .clone()
                            .unwrap_or_else(|| Identifier::new(name.clone()));
                        items_out.push((
                            Expr::Variable(agg_alias.clone()),
                            item.alias.clone(),
                        ));
                    } else {
                        let gen_name = expr_to_col_name(&item.expr);
                        items_out.push((
                            Expr::Variable(Identifier::new(gen_name)),
                            item.alias.clone(),
                        ));
                    }
                }
                items_out
            };
            Ok(Operator::Project {
                items: project_items,
                child: Box::new(agg),
            })
        } else {
            Ok(agg)
        }
    }

    // ── UNWIND planning ───────────────────────────────────────────────────────

    fn plan_unwind(
        &self,
        uc: &UnwindClause,
        child: Option<Operator>,
    ) -> Result<Operator, GqlError> {
        let child = child.unwrap_or(Operator::SingleRow);

        Ok(Operator::Unwind {
            list_expr: uc.expr.clone(),
            alias: uc.alias.clone(),
            child: Box::new(child),
        })
    }

    // ── SET planning ──────────────────────────────────────────────────────────

    fn plan_set(&self, sc: &SetClause, child: Option<Operator>) -> Result<Operator, GqlError> {
        let child = child.ok_or_else(|| {
            GqlError::Plan(PlanError::UndefinedVariable {
                name: "SET without preceding MATCH or WITH".to_string(),
                offset: 0,
            })
        })?;

        let items: Vec<SetItem> = sc.items.iter().map(map_set_clause_item).collect();

        Ok(Operator::Set {
            items,
            child: Box::new(child),
        })
    }

    // ── REMOVE planning ───────────────────────────────────────────────────────

    fn plan_remove(
        &self,
        rc: &RemoveClause,
        child: Option<Operator>,
    ) -> Result<Operator, GqlError> {
        let child = child.ok_or_else(|| {
            GqlError::Plan(PlanError::UndefinedVariable {
                name: "REMOVE without preceding MATCH or WITH".to_string(),
                offset: 0,
            })
        })?;

        let items: Vec<RemoveItem> = rc.items.iter().map(map_remove_clause_item).collect();

        Ok(Operator::Remove {
            items,
            child: Box::new(child),
        })
    }

    // ── MERGE planning ────────────────────────────────────────────────────────

    fn plan_merge(&self, mc: &MergeClause, child: Option<Operator>) -> Result<Operator, GqlError> {
        // Check if this is a relationship merge: MERGE (a)-[r:TYPE]->(b)
        if !mc.pattern.steps.is_empty() {
            return self.plan_merge_relationship(mc, child);
        }

        let node_alias = mc.pattern.start.variable.clone();
        let labels = mc.pattern.start.labels.clone();
        let properties: Vec<(String, Expr)> = mc
            .pattern
            .start
            .properties
            .as_ref()
            .map(|p| {
                p.iter()
                    .map(|(k, v)| (k.to_string(), v.clone()))
                    .collect()
            })
            .unwrap_or_default();

        let on_create: Vec<SetItem> = mc.on_create.iter().map(map_set_clause_item).collect();
        let on_match: Vec<SetItem> = mc.on_match.iter().map(map_set_clause_item).collect();

        let merge_op = Operator::Merge {
            node_alias: node_alias.clone(),
            labels,
            properties,
            on_create,
            on_match,
            child: child.map(Box::new),
        };

        // If the MERGE clause has a path variable (e.g. `MERGE p = (a {...})`),
        // wrap with AssemblePath so `p` is available in subsequent RETURN.
        if let Some(ref path_var) = mc.pattern.variable {
            let node_aliases = node_alias.into_iter().collect();
            Ok(Operator::AssemblePath {
                path_alias: path_var.clone(),
                node_aliases,
                edge_aliases: vec![],
                child: Box::new(merge_op),
            })
        } else {
            Ok(merge_op)
        }
    }

    fn plan_merge_relationship(
        &self,
        mc: &MergeClause,
        child: Option<Operator>,
    ) -> Result<Operator, GqlError> {
        let child = child.ok_or_else(|| {
            GqlError::Plan(PlanError::UndefinedVariable {
                name: "MERGE relationship requires preceding MATCH".to_string(),
                offset: 0,
            })
        })?;

        let step = &mc.pattern.steps[0];
        let src_alias = mc
            .pattern
            .start
            .variable
            .clone()
            .unwrap_or_else(|| Identifier::new("__merge_src__"));
        let dst_alias = step
            .node
            .variable
            .clone()
            .unwrap_or_else(|| Identifier::new("__merge_dst__"));
        // When a path variable exists on the MERGE pattern, ensure we capture the
        // edge ID by assigning a synthetic alias if none was specified by the user.
        let rel_alias = if mc.pattern.variable.is_some() {
            Some(step.rel.variable.clone().unwrap_or_else(|| {
                Identifier::new("__merge_rel__")
            }))
        } else {
            step.rel.variable.clone()
        };
        let rel_type = step
            .rel
            .rel_types
            .first()
            .cloned()
            .unwrap_or_else(|| crate::ast::RelType::new(""));
        let direction = step.rel.direction;
        let properties: Vec<(String, Expr)> = step
            .rel
            .properties
            .as_ref()
            .map(|props| {
                props
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.clone()))
                    .collect()
            })
            .unwrap_or_default();

        let on_create: Vec<SetItem> = mc.on_create.iter().map(map_set_clause_item).collect();
        let on_match: Vec<SetItem> = mc.on_match.iter().map(map_set_clause_item).collect();

        let merge_op = Operator::MergeRelationship {
            src_alias: src_alias.clone(),
            dst_alias: dst_alias.clone(),
            rel_alias: rel_alias.clone(),
            rel_type,
            direction,
            properties,
            on_create,
            on_match,
            child: Box::new(child),
        };

        // If the MERGE clause has a path variable (e.g. `MERGE p = (a)-[:R]->(b)`),
        // wrap with AssemblePath so `p` is available in subsequent RETURN.
        if let Some(ref path_var) = mc.pattern.variable {
            let node_aliases = vec![src_alias, dst_alias];
            // rel_alias is guaranteed Some here (forced above when path_var is Some).
            let edge_aliases = rel_alias.into_iter().collect();
            Ok(Operator::AssemblePath {
                path_alias: path_var.clone(),
                node_aliases,
                edge_aliases,
                child: Box::new(merge_op),
            })
        } else {
            Ok(merge_op)
        }
    }

    // ── RETURN planning ───────────────────────────────────────────────────────

    fn plan_return(&self, rc: &ReturnClause, child: Operator) -> Result<Operator, GqlError> {
        // Expand RETURN * to all user-visible variables in scope.
        let items = expand_star_items(&rc.items, &child);

        // Determine whether any item uses an aggregate function.
        let has_aggregation = items.iter().any(|item| has_agg_function(&item.expr));

        // For aggregation, wrap the pipeline now so ORDER BY can sort on aggregate output.
        // For plain queries, Sort/Distinct/Skip/Limit all run on the raw variable rows,
        // BEFORE ProduceResults does the final projection.  This matches Cypher semantics:
        // ORDER BY `n.name` sorts on the real value, not on the projection alias.
        let pipeline = if has_aggregation {
            self.plan_aggregation_from_items(&items, child)?
        } else {
            child
        };

        // ORDER BY -- sort expressions reference raw variable names.
        // When aggregation is active, rewrite aggregate function calls in ORDER BY
        // to reference their output aliases (the Aggregate operator stores results
        // under alias names, not as live function calls).
        // For non-aggregation, ORDER BY may reference RETURN aliases that don't
        // exist in the raw rows yet; we add a pre-projection to expose them.
        let pipeline = if !rc.order_by.is_empty() {
            // For non-aggregation: if ORDER BY references any RETURN alias,
            // pre-project those aliases onto the raw rows before sorting.
            let pipeline = if !has_aggregation {
                // Collect all RETURN aliases referenced in ORDER BY (as Variable or Property root).
                let needs_pre_project = rc.order_by.iter().any(|s| {
                    // Extract the root variable name from the ORDER BY expression.
                    let root_var = match &s.expr {
                        Expr::Variable(v) => Some(v.0.as_str()),
                        Expr::Property(obj, _) => {
                            if let Expr::Variable(v) = obj.as_ref() {
                                Some(v.0.as_str())
                            } else {
                                None
                            }
                        }
                        _ => None,
                    };
                    if let Some(root) = root_var {
                        items.iter().any(|item| {
                            let alias = item.alias.as_ref().map(|a| a.0.as_str())
                                .unwrap_or_else(|| "");
                            root == alias && alias != expr_to_col_name(&item.expr).as_str()
                        })
                    } else {
                        false
                    }
                });
                if needs_pre_project {
                    // Add a Project that exposes RETURN aliases alongside raw vars.
                    let child_aliases = crate::planner::collect_visible_aliases(&pipeline);
                    let mut aug_items: Vec<(Expr, Option<Identifier>)> = child_aliases
                        .iter()
                        .map(|a| (Expr::Variable(Identifier::new(a.clone())), None))
                        .collect();
                    for item in &items {
                        if let Some(alias) = &item.alias {
                            if alias.0 != expr_to_col_name(&item.expr) {
                                aug_items.push((item.expr.clone(), Some(alias.clone())));
                            }
                        }
                    }
                    Operator::Project {
                        items: aug_items,
                        child: Box::new(pipeline),
                    }
                } else {
                    pipeline
                }
            } else {
                pipeline
            };
            let sort_items: Vec<(Expr, bool)> = rc
                .order_by
                .iter()
                .map(|s| {
                    if has_aggregation {
                        (rewrite_agg_order_expr(&s.expr, &items), s.ascending)
                    } else {
                        (s.expr.clone(), s.ascending)
                    }
                })
                .collect();
            Operator::Sort {
                items: sort_items,
                child: Box::new(pipeline),
            }
        } else {
            pipeline
        };

        // SKIP.
        let pipeline = if let Some(skip_expr) = &rc.skip {
            Operator::Skip {
                count: skip_expr.clone(),
                child: Box::new(pipeline),
            }
        } else {
            pipeline
        };

        // LIMIT.
        let pipeline = if let Some(limit_expr) = &rc.limit {
            Operator::Limit {
                count: limit_expr.clone(),
                child: Box::new(pipeline),
            }
        } else {
            pipeline
        };

        // ProduceResults does the final projection from raw rows (or aggregate output rows)
        // to the output column names the caller asked for.
        let columns: Vec<(String, Expr)> = items
            .iter()
            .map(|item| {
                let name = return_item_name(item);
                if has_aggregation {
                    // After aggregation the alias is already in the row; just reference it.
                    let alias = item
                        .alias
                        .clone()
                        .map(|a| a.0)
                        .unwrap_or_else(|| name.clone());
                    (name, Expr::Variable(Identifier::new(alias)))
                } else {
                    (name, item.expr.clone())
                }
            })
            .collect();

        let produce = Operator::ProduceResults {
            columns,
            child: Box::new(pipeline),
        };

        // DISTINCT is applied AFTER ProduceResults so deduplication is on projected values.
        if rc.distinct && !has_aggregation {
            Ok(Operator::Distinct {
                child: Box::new(produce),
            })
        } else {
            Ok(produce)
        }
    }

    fn plan_aggregation_from_items(
        &self,
        items: &[ReturnItem],
        child: Operator,
    ) -> Result<Operator, GqlError> {
        let mut keys: Vec<Expr> = Vec::new();
        let mut aggregations: Vec<(AggFunction, Expr, Option<Expr>, bool, Identifier)> = Vec::new();
        // Track post-aggregation projections for expressions wrapping aggregates
        // (e.g., `count(a) + 3`, `count(a) > 0`).
        let mut post_projections: Vec<(Expr, Option<Identifier>)> = Vec::new();
        let mut needs_post_project = false;
        let mut agg_counter: usize = 0;

        for item in items {
            if has_agg_function(&item.expr) {
                if let Ok((func, arg, second_arg, distinct)) = extract_agg_function(&item.expr) {
                    // Simple case: expr is a bare aggregate function
                    let alias = item
                        .alias
                        .clone()
                        .unwrap_or_else(|| Identifier::new(return_item_name(item)));
                    aggregations.push((func, arg, second_arg, distinct, alias.clone()));
                    post_projections.push((Expr::Variable(alias), None));
                } else {
                    // Complex case: aggregate is wrapped (e.g., count(a) + 3, count(a) * 10 + count(b) * 5)
                    // Iteratively extract ALL aggregate calls and replace with temp aliases.
                    needs_post_project = true;
                    let mut rewritten = item.expr.clone();
                    // Keep extracting aggregates until no more remain in the expression.
                    while has_agg_function(&rewritten) {
                        let temp_alias = Identifier::new(format!("__agg_{agg_counter}"));
                        agg_counter += 1;
                        if let Some((func, arg, second_arg, distinct)) =
                            find_inner_aggregate(&rewritten)
                        {
                            aggregations.push((func, arg, second_arg, distinct, temp_alias.clone()));
                            rewritten = rewrite_agg_to_var(&rewritten, &temp_alias);
                        } else {
                            break;
                        }
                    }
                    let final_alias = item
                        .alias
                        .clone()
                        .unwrap_or_else(|| Identifier::new(return_item_name(item)));
                    post_projections.push((rewritten, Some(final_alias)));
                }
            } else {
                // Non-aggregate: use as group key.
                // If the user gave an alias different from the generated column name,
                // we must add a post-project step to rename the key column.
                let gen_name = expr_to_col_name(&item.expr);
                let alias = item
                    .alias
                    .clone()
                    .unwrap_or_else(|| Identifier::new(return_item_name(item)));
                keys.push(item.expr.clone());
                if gen_name != alias.0 {
                    needs_post_project = true;
                    post_projections
                        .push((Expr::Variable(Identifier::new(gen_name)), Some(alias)));
                } else {
                    post_projections.push((Expr::Variable(alias), None));
                }
            }
        }

        let agg_op = Operator::Aggregate {
            keys,
            aggregations,
            child: Box::new(child),
        };

        if needs_post_project {
            // Wrap with a Project to compute the post-agg expressions
            let project_items: Vec<(Expr, Option<Identifier>)> = post_projections;
            Ok(Operator::Project {
                items: project_items,
                child: Box::new(agg_op),
            })
        } else {
            Ok(agg_op)
        }
    }
}

// ── Free helpers ──────────────────────────────────────────────────────────────

/// If `items` contains a `RETURN *` / `WITH *` placeholder, expand it to
/// all user-visible variables discovered from the child operator tree.
/// Non-star items are returned unchanged.  Handles both pure `RETURN *`
/// and mixed forms like `RETURN *, n.name AS extra`.
fn expand_star_items(items: &[ReturnItem], child: &Operator) -> Vec<ReturnItem> {
    let has_star = items
        .iter()
        .any(|item| matches!(&item.expr, Expr::Variable(id) if id.0 == "*") && item.alias.is_none());

    if !has_star {
        return items.to_vec();
    }

    let aliases = crate::planner::collect_visible_aliases(child);
    let star_items: Vec<ReturnItem> = aliases
        .into_iter()
        .map(|name| ReturnItem {
            expr: Expr::Variable(Identifier::new(name)),
            alias: None,
        })
        .collect();

    // Replace each `*` placeholder with the expanded aliases; keep non-star items.
    let mut result = Vec::new();
    for item in items {
        if matches!(&item.expr, Expr::Variable(id) if id.0 == "*") && item.alias.is_none() {
            result.extend(star_items.clone());
        } else {
            result.push(item.clone());
        }
    }
    result
}

/// Rewrite an ORDER BY expression when aggregation is active.
/// If the expression is an aggregate function call that matches a RETURN item,
/// replace it with a variable reference to the output alias.
/// Rewrite an ORDER BY expression in a WITH clause that uses aggregation.
/// Maps pre-projection expressions (like `a.name`) to their post-aggregation aliases
/// (like `name` if the WITH has `a.name AS name`). Also handles aggregate functions.
fn rewrite_with_order_expr(expr: &Expr, with_items: &[ReturnItem]) -> Expr {
    // First check if the expression exactly matches a WITH item's expression.
    for ri in with_items {
        if *expr == ri.expr {
            let alias = ri
                .alias
                .clone()
                .map(|a| a.0)
                .unwrap_or_else(|| return_item_name(ri));
            return Expr::Variable(Identifier::new(alias));
        }
    }
    // For compound expressions, try to rewrite sub-expressions.
    // E.g., `a.name + 'C'` where `a.name AS name` → `name + 'C'`.
    match expr {
        Expr::BinaryOp { op, left, right } => {
            let new_left = rewrite_with_order_expr(left, with_items);
            let new_right = rewrite_with_order_expr(right, with_items);
            Expr::BinaryOp {
                op: *op,
                left: Box::new(new_left),
                right: Box::new(new_right),
            }
        }
        Expr::FunctionCall { name, args: _, distinct: _ } => {
            if is_agg_name(name.as_str()) {
                // Map aggregate functions to their output aliases.
                for ri in with_items {
                    if let Expr::FunctionCall { name: rn, .. } = &ri.expr {
                        if name == rn {
                            let alias = ri
                                .alias
                                .clone()
                                .map(|a| a.0)
                                .unwrap_or_else(|| return_item_name(ri));
                            return Expr::Variable(Identifier::new(alias));
                        }
                    }
                }
            }
            expr.clone()
        }
        _ => expr.clone(),
    }
}

fn rewrite_agg_order_expr(expr: &Expr, return_items: &[ReturnItem]) -> Expr {
    // After aggregation, ORDER BY expressions must reference output column names.
    // Try to find the matching RETURN item and rewrite to Variable(output_column_name).
    let expr_col = expr_to_col_name(expr);
    for ri in return_items {
        let ri_col = expr_to_col_name(&ri.expr);
        let ri_alias = return_item_name(ri);
        // Match if the ORDER BY expression equals the return item expression,
        // or if it matches the alias.
        if expr_col == ri_col || expr_col == ri_alias {
            return Expr::Variable(Identifier::new(ri_alias));
        }
    }
    // For compound expressions (e.g. `$age + avg(person.age) - 1000`), recursively
    // rewrite sub-expressions that match return items, replacing agg calls with aliases.
    match expr {
        Expr::BinaryOp { op, left, right } => Expr::BinaryOp {
            op: *op,
            left: Box::new(rewrite_agg_order_expr(left, return_items)),
            right: Box::new(rewrite_agg_order_expr(right, return_items)),
        },
        Expr::UnaryOp { op, expr: inner } => Expr::UnaryOp {
            op: *op,
            expr: Box::new(rewrite_agg_order_expr(inner, return_items)),
        },
        _ => expr.clone(),
    }
}

/// Map an AST `SetClauseItem` to a planner `SetItem`.
fn map_set_clause_item(item: &SetClauseItem) -> SetItem {
    match item {
        SetClauseItem::SetProperty { alias, prop, value } => SetItem::Property {
            alias: alias.clone(),
            key: prop.to_string(),
            value: value.clone(),
        },
        SetClauseItem::SetMap { alias, map, merge } => SetItem::Map {
            alias: alias.clone(),
            map: map.clone(),
            merge: *merge,
        },
        SetClauseItem::SetLabels { alias, labels } => SetItem::Labels {
            alias: alias.clone(),
            labels: labels.clone(),
        },
    }
}

/// Map an AST `RemoveClauseItem` to a planner `RemoveItem`.
fn map_remove_clause_item(item: &RemoveClauseItem) -> RemoveItem {
    match item {
        RemoveClauseItem::RemoveProperty { alias, prop } => RemoveItem::Property {
            alias: alias.clone(),
            key: prop.to_string(),
        },
        RemoveClauseItem::RemoveLabels { alias, labels } => RemoveItem::Labels {
            alias: alias.clone(),
            labels: labels.clone(),
        },
    }
}

/// Return the output column name for a RETURN item.
fn return_item_name(item: &ReturnItem) -> String {
    if let Some(alias) = &item.alias {
        return alias.0.clone();
    }
    expr_to_col_name(&item.expr)
}

#[allow(dead_code)]
fn expr_name(expr: &Expr) -> String {
    match expr {
        Expr::Variable(id) => id.0.clone(),
        Expr::Property(obj, key) => format!("{}.{}", expr_name(obj), key),
        Expr::FunctionCall { name, args, .. } => {
            let arg_strs: Vec<String> = args.iter().map(expr_name).collect();
            format!("{}({})", name, arg_strs.join(", "))
        }
        Expr::Integer(n) => n.to_string(),
        Expr::Float(f) => f.to_string(),
        Expr::Str(s) => s.to_string(),
        Expr::Bool(b) => b.to_string(),
        Expr::Null => "null".to_string(),
        _ => "expr".to_string(),
    }
}

/// Returns `true` if the expression is or contains an aggregate function call.
fn has_agg_function(expr: &Expr) -> bool {
    match expr {
        Expr::FunctionCall { name, args, .. } => {
            // The function itself is an aggregate, OR one of its args contains an aggregate
            // (e.g. size(collect(a)), all(x IN collect(...) WHERE x))
            is_agg_name(name.as_str()) || args.iter().any(has_agg_function)
        }
        Expr::BinaryOp { left, right, .. } => has_agg_function(left) || has_agg_function(right),
        Expr::UnaryOp { expr, .. } => has_agg_function(expr),
        Expr::Property(obj, _) => has_agg_function(obj),
        Expr::List(items) => items.iter().any(has_agg_function),
        Expr::MapLiteral(pairs) => pairs.iter().any(|(_, v)| has_agg_function(v)),
        Expr::ListComp {
            list, filter, map, ..
        } => {
            has_agg_function(list)
                || filter.as_deref().map(has_agg_function).unwrap_or(false)
                || has_agg_function(map)
        }
        Expr::Case {
            subject,
            alternatives,
            default,
        } => {
            subject.as_deref().map(has_agg_function).unwrap_or(false)
                || alternatives
                    .iter()
                    .any(|(c, r)| has_agg_function(c) || has_agg_function(r))
                || default.as_deref().map(has_agg_function).unwrap_or(false)
        }
        Expr::IsNull(e) | Expr::IsNotNull(e) => has_agg_function(e),
        Expr::Subscript(a, b) => has_agg_function(a) || has_agg_function(b),
        Expr::Slice(a, b, c) => {
            has_agg_function(a)
                || b.as_deref().map(has_agg_function).unwrap_or(false)
                || c.as_deref().map(has_agg_function).unwrap_or(false)
        }
        _ => false,
    }
}

fn is_agg_name(name: &str) -> bool {
    matches!(
        name.to_uppercase().as_str(),
        "COUNT"
            | "SUM"
            | "AVG"
            | "MIN"
            | "MAX"
            | "COLLECT"
            | "STDEV"
            | "STDEVP"
            | "PERCENTILECONT"
            | "PERCENTILEDISC"
    )
}

/// Extract the aggregate function, first argument, optional second argument, and distinct flag
/// from a top-level `FunctionCall` expression.
fn extract_agg_function(expr: &Expr) -> Result<(AggFunction, Expr, Option<Expr>, bool), GqlError> {
    match expr {
        Expr::FunctionCall {
            name,
            args,
            distinct,
        } => {
            let func = match name.to_uppercase().as_str() {
                "COUNT" => AggFunction::Count,
                "SUM" => AggFunction::Sum,
                "AVG" => AggFunction::Avg,
                "MIN" => AggFunction::Min,
                "MAX" => AggFunction::Max,
                "COLLECT" => AggFunction::Collect,
                "STDEV" => AggFunction::StDev,
                "STDEVP" => AggFunction::StDevP,
                "PERCENTILECONT" => AggFunction::PercentileCont,
                "PERCENTILEDISC" => AggFunction::PercentileDisc,
                other => {
                    return Err(GqlError::Execution(ExecutionError::unsupported(format!(
                        "aggregate function '{other}'"
                    ))));
                }
            };
            let arg = args.first().cloned().unwrap_or(Expr::Null);
            let second_arg = args.get(1).cloned();
            Ok((func, arg, second_arg, *distinct))
        }
        _ => Err(GqlError::Execution(ExecutionError::unsupported(
            "non-function aggregate expression",
        ))),
    }
}

/// Find the innermost aggregate function call in a compound expression.
/// Returns (AggFunction, arg_expr, second_arg_expr, distinct) if found.
fn find_inner_aggregate(expr: &Expr) -> Option<(AggFunction, Expr, Option<Expr>, bool)> {
    match expr {
        Expr::FunctionCall {
            name,
            args,
            distinct,
        } => {
            let func_opt = match name.to_uppercase().as_str() {
                "COUNT" => Some(AggFunction::Count),
                "SUM" => Some(AggFunction::Sum),
                "AVG" => Some(AggFunction::Avg),
                "MIN" => Some(AggFunction::Min),
                "MAX" => Some(AggFunction::Max),
                "COLLECT" => Some(AggFunction::Collect),
                "STDEV" => Some(AggFunction::StDev),
                "STDEVP" => Some(AggFunction::StDevP),
                "PERCENTILECONT" => Some(AggFunction::PercentileCont),
                "PERCENTILEDISC" => Some(AggFunction::PercentileDisc),
                _ => None,
            };
            if let Some(func) = func_opt {
                let arg = args.first().cloned().unwrap_or(Expr::Null);
                let second_arg = args.get(1).cloned();
                Some((func, arg, second_arg, *distinct))
            } else {
                // Non-aggregate function: search inside args for nested aggregate
                args.iter().find_map(find_inner_aggregate)
            }
        }
        Expr::BinaryOp { left, right, .. } => {
            find_inner_aggregate(left).or_else(|| find_inner_aggregate(right))
        }
        Expr::UnaryOp { expr: inner, .. } => find_inner_aggregate(inner),
        Expr::Property(inner, _) => find_inner_aggregate(inner),
        Expr::MapLiteral(pairs) => pairs.iter().find_map(|(_, v)| find_inner_aggregate(v)),
        Expr::ListComp { list, map, .. } => {
            find_inner_aggregate(list).or_else(|| find_inner_aggregate(map))
        }
        _ => None,
    }
}

/// Rewrite an expression by replacing the first aggregate function call with
/// a variable reference to `alias`.
fn rewrite_agg_to_var(expr: &Expr, alias: &Identifier) -> Expr {
    match expr {
        Expr::FunctionCall { name, .. } if is_agg_name(name) => {
            // This IS the aggregate: replace the whole call with a variable.
            Expr::Variable(alias.clone())
        }
        Expr::FunctionCall { name, args, distinct } => {
            // Non-aggregate function: rewrite inside args if any contain an aggregate.
            if args.iter().any(has_agg_function) {
                let new_args = args
                    .iter()
                    .map(|a| {
                        if has_agg_function(a) {
                            rewrite_agg_to_var(a, alias)
                        } else {
                            a.clone()
                        }
                    })
                    .collect();
                Expr::FunctionCall {
                    name: name.clone(),
                    args: new_args,
                    distinct: *distinct,
                }
            } else {
                expr.clone()
            }
        }
        Expr::BinaryOp { op, left, right } => {
            if has_agg_function(left) {
                Expr::BinaryOp {
                    op: *op,
                    left: Box::new(rewrite_agg_to_var(left, alias)),
                    right: right.clone(),
                }
            } else if has_agg_function(right) {
                Expr::BinaryOp {
                    op: *op,
                    left: left.clone(),
                    right: Box::new(rewrite_agg_to_var(right, alias)),
                }
            } else {
                expr.clone()
            }
        }
        Expr::UnaryOp { op, expr: inner } => Expr::UnaryOp {
            op: *op,
            expr: Box::new(rewrite_agg_to_var(inner, alias)),
        },
        Expr::MapLiteral(pairs) => {
            // Rewrite inside map values
            let new_pairs = pairs
                .iter()
                .map(|(k, v)| {
                    if has_agg_function(v) {
                        (k.clone(), rewrite_agg_to_var(v, alias))
                    } else {
                        (k.clone(), v.clone())
                    }
                })
                .collect();
            Expr::MapLiteral(new_pairs)
        }
        Expr::ListComp {
            variable,
            list,
            filter,
            map,
        } => {
            // Rewrite inside list if it contains an aggregate
            Expr::ListComp {
                variable: variable.clone(),
                list: if has_agg_function(list) {
                    Box::new(rewrite_agg_to_var(list, alias))
                } else {
                    list.clone()
                },
                filter: filter.clone(),
                map: if has_agg_function(map) {
                    Box::new(rewrite_agg_to_var(map, alias))
                } else {
                    map.clone()
                },
            }
        }
        _ => expr.clone(),
    }
}

/// Build a filter expression that checks `alias.prop = val` for each property
/// in an inline map literal `{prop: val, ...}`.
fn props_to_filter_expr(alias: &Identifier, props: &[(smol_str::SmolStr, Expr)]) -> Expr {
    let mut parts: Vec<Expr> = props
        .iter()
        .map(|(key, val)| Expr::BinaryOp {
            op: crate::ast::BinaryOp::Eq,
            left: Box::new(Expr::Property(
                Box::new(Expr::Variable(alias.clone())),
                key.clone(),
            )),
            right: Box::new(val.clone()),
        })
        .collect();

    if parts.len() == 1 {
        parts.remove(0)
    } else {
        parts
            .into_iter()
            .reduce(|acc, e| Expr::BinaryOp {
                op: crate::ast::BinaryOp::And,
                left: Box::new(acc),
                right: Box::new(e),
            })
            .unwrap_or(Expr::Bool(true))
    }
}

/// Build a filter expression that checks whether a node has a specific label.
///
/// In Phase 2, label membership is enforced at the `NodeScan` level.  After an
/// `Expand`, the executor does not re-check labels automatically.  We synthesise
/// a `HAS LABEL` predicate using `FunctionCall { name: "hasLabel", ... }` which
/// `eval_expr` understands (see filter.rs).
fn label_filter_expr(alias: &Identifier, label: &Label) -> Expr {
    // Use hasLabel() built-in which is evaluated by eval_expr in filter.rs.
    Expr::FunctionCall {
        name: "hasLabel".into(),
        args: vec![
            Expr::Variable(alias.clone()),
            Expr::Str(label.as_str().into()),
        ],
        distinct: false,
    }
}

/// Derive a column name from an expression (mirrors executor's `expr_to_column_name`).
fn binop_col_prec(op: &crate::ast::BinaryOp) -> i32 {
    use crate::ast::BinaryOp;
    match op {
        BinaryOp::Pow => 6,
        BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod => 5,
        BinaryOp::Add | BinaryOp::Sub | BinaryOp::Concat => 4,
        _ => 3,
    }
}

fn binop_col_nonassoc(op: &crate::ast::BinaryOp) -> bool {
    use crate::ast::BinaryOp;
    matches!(op, BinaryOp::Sub | BinaryOp::Div | BinaryOp::Mod)
}

fn expr_to_col_name(expr: &Expr) -> String {
    match expr {
        Expr::Variable(id) => id.0.clone(),
        Expr::Property(obj, key) => {
            let obj_str = match obj.as_ref() {
                Expr::Subscript(_, _) => format!("({})", expr_to_col_name(obj)),
                _ => expr_to_col_name(obj),
            };
            format!("{}.{}", obj_str, key)
        }
        Expr::FunctionCall {
            name,
            args,
            distinct,
            ..
        } => {
            let arg_strs: Vec<String> = args.iter().map(expr_to_col_name).collect();
            if *distinct {
                format!("{}(DISTINCT {})", name, arg_strs.join(", "))
            } else {
                format!("{}({})", name, arg_strs.join(", "))
            }
        }
        Expr::Integer(n) => n.to_string(),
        Expr::Float(f) => f.to_string(),
        Expr::Str(s) => format!("'{s}'"),
        Expr::BinaryOp { op, left, right } => {
            let op_str = match op {
                crate::ast::BinaryOp::Add => "+",
                crate::ast::BinaryOp::Sub => "-",
                crate::ast::BinaryOp::Mul => "*",
                crate::ast::BinaryOp::Div => "/",
                crate::ast::BinaryOp::Mod => "%",
                crate::ast::BinaryOp::Eq => "=",
                crate::ast::BinaryOp::Neq => "<>",
                crate::ast::BinaryOp::Lt => "<",
                crate::ast::BinaryOp::Lte => "<=",
                crate::ast::BinaryOp::Gt => ">",
                crate::ast::BinaryOp::Gte => ">=",
                crate::ast::BinaryOp::And => "AND",
                crate::ast::BinaryOp::Or => "OR",
                crate::ast::BinaryOp::Xor => "XOR",
                crate::ast::BinaryOp::In => "IN",
                crate::ast::BinaryOp::Contains => "CONTAINS",
                crate::ast::BinaryOp::StartsWith => "STARTS WITH",
                crate::ast::BinaryOp::EndsWith => "ENDS WITH",
                crate::ast::BinaryOp::Pow => "^",
                crate::ast::BinaryOp::RegexMatch => "=~",
                crate::ast::BinaryOp::Concat => "++",
            };
            let parent_prec = binop_col_prec(op);
            let left_str = match left.as_ref() {
                Expr::BinaryOp { op: child_op, .. } if binop_col_prec(child_op) < parent_prec => {
                    format!("({})", expr_to_col_name(left))
                }
                _ => expr_to_col_name(left),
            };
            let right_str = match right.as_ref() {
                Expr::BinaryOp { op: child_op, .. }
                    if binop_col_prec(child_op) < parent_prec
                        || (binop_col_prec(child_op) == parent_prec && binop_col_nonassoc(op)) =>
                {
                    format!("({})", expr_to_col_name(right))
                }
                _ => expr_to_col_name(right),
            };
            format!("{} {} {}", left_str, op_str, right_str)
        }
        Expr::Bool(b) => b.to_string(),
        Expr::Null => "null".to_string(),
        Expr::HasLabel(e, label) => format!("{}:{}", expr_to_col_name(e), label.0),
        Expr::UnaryOp { op, expr: e } => match op {
            crate::ast::UnaryOp::Not => format!("NOT {}", expr_to_col_name(e)),
            crate::ast::UnaryOp::Neg => format!("-{}", expr_to_col_name(e)),
        },
        Expr::IsNull(e) => format!("{} IS NULL", expr_to_col_name(e)),
        Expr::IsNotNull(e) => format!("{} IS NOT NULL", expr_to_col_name(e)),
        Expr::MapLiteral(pairs) => {
            let pairs_str: Vec<String> = pairs
                .iter()
                .map(|(k, v)| format!("{}: {}", k, expr_to_col_name_quoted(v)))
                .collect();
            format!("{{{}}}", pairs_str.join(", "))
        }
        Expr::List(items) => {
            let items_str: Vec<String> = items.iter().map(expr_to_col_name).collect();
            format!("[{}]", items_str.join(", "))
        }
        Expr::Subscript(list, idx) => {
            format!("{}[{}]", expr_to_col_name(list), expr_to_col_name(idx))
        }
        Expr::Parameter(name) => format!("${name}"),
        Expr::ListComp {
            variable,
            list,
            filter,
            map,
        } => {
            let list_str = expr_to_col_name(list);
            let var_str = &variable.0;
            if let Some(f) = filter {
                let filter_str = expr_to_col_name(f);
                if **map != Expr::Variable(variable.clone()) {
                    format!(
                        "[{} IN {} WHERE {} | {}]",
                        var_str,
                        list_str,
                        filter_str,
                        expr_to_col_name(map)
                    )
                } else {
                    format!("[{} IN {} WHERE {}]", var_str, list_str, filter_str)
                }
            } else {
                format!("[{} IN {}]", var_str, list_str)
            }
        }
        _ => format!("{:?}", expr),
    }
}

/// Like `expr_to_col_name` but quotes string literals (for map literal values).
fn expr_to_col_name_quoted(expr: &Expr) -> String {
    match expr {
        Expr::Str(s) => format!("'{s}'"),
        _ => expr_to_col_name(expr),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{
        BinaryOp, DeleteClause, Direction, Expr, Identifier, Label, MatchClause, MergeClause,
        NodePattern, PathPattern, PathStep, Query, RelPattern, RemoveClause, RemoveClauseItem,
        ReturnClause, ReturnItem, SetClause, SetClauseItem, SortItem, UnwindClause, WithClause,
    };
    use crate::storage::InMemoryGraphStore;

    fn make_store() -> InMemoryGraphStore {
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();
        store
    }

    fn make_simple_match_query(label: &str, alias: &str) -> Query {
        Query { unions: Vec::new(),
            clauses: vec![
                Clause::Match(MatchClause {
                    patterns: vec![PathPattern {
                        variable: None,
                        start: NodePattern {
                            variable: Some(Identifier::new(alias)),
                            labels: vec![Label::new(label)],
                            properties: None,
                            where_clause: None,
                        },
                        steps: vec![],
                    }],
                    where_clause: None,
                }),
                Clause::Return(ReturnClause {
                    distinct: false,
                    items: vec![ReturnItem {
                        expr: Expr::Variable(Identifier::new(alias)),
                        alias: None,
                    }],
                    order_by: vec![],
                    skip: None,
                    limit: None,
                }),
            ],
        }
    }

    /// Build a simple MATCH (n:Label) pipeline (no RETURN) for use in write-clause tests.
    fn make_match_clause(label: &str, alias: &str) -> Clause {
        Clause::Match(MatchClause {
            patterns: vec![PathPattern {
                variable: None,
                start: NodePattern {
                    variable: Some(Identifier::new(alias)),
                    labels: vec![Label::new(label)],
                    properties: None,
                    where_clause: None,
                },
                steps: vec![],
            }],
            where_clause: None,
        })
    }

    #[test]
    fn plan_simple_match_return() {
        let store = make_store();
        let planner = QueryPlanner::new(&store, "g");
        let query = make_simple_match_query("Person", "p");
        let plan = planner.plan(&query).unwrap();
        // Root must be ProduceResults.
        assert!(matches!(plan, Operator::ProduceResults { .. }));
    }

    #[test]
    fn plan_match_with_where() {
        let store = make_store();
        let planner = QueryPlanner::new(&store, "g");
        let query = Query { unions: Vec::new(),
            clauses: vec![
                Clause::Match(MatchClause {
                    patterns: vec![PathPattern {
                        variable: None,
                        start: NodePattern {
                            variable: Some(Identifier::new("n")),
                            labels: vec![Label::new("Person")],
                            properties: None,
                            where_clause: None,
                        },
                        steps: vec![],
                    }],
                    where_clause: Some(Expr::BinaryOp {
                        op: crate::ast::BinaryOp::Gt,
                        left: Box::new(Expr::Property(
                            Box::new(Expr::Variable(Identifier::new("n"))),
                            "age".into(),
                        )),
                        right: Box::new(Expr::Integer(18)),
                    }),
                }),
                Clause::Return(ReturnClause {
                    distinct: false,
                    items: vec![ReturnItem {
                        expr: Expr::Variable(Identifier::new("n")),
                        alias: None,
                    }],
                    order_by: vec![],
                    skip: None,
                    limit: None,
                }),
            ],
        };
        let plan = planner.plan(&query).unwrap();
        // Should have a Filter in the tree.
        assert!(plan_contains_filter(&plan));
    }

    #[test]
    fn plan_return_with_limit_skip() {
        let store = make_store();
        let planner = QueryPlanner::new(&store, "g");
        let query = Query { unions: Vec::new(),
            clauses: vec![
                Clause::Match(MatchClause {
                    patterns: vec![PathPattern {
                        variable: None,
                        start: NodePattern {
                            variable: Some(Identifier::new("n")),
                            labels: vec![Label::new("Person")],
                            properties: None,
                            where_clause: None,
                        },
                        steps: vec![],
                    }],
                    where_clause: None,
                }),
                Clause::Return(ReturnClause {
                    distinct: false,
                    items: vec![ReturnItem {
                        expr: Expr::Variable(Identifier::new("n")),
                        alias: None,
                    }],
                    order_by: vec![],
                    skip: Some(Expr::Integer(2)),
                    limit: Some(Expr::Integer(5)),
                }),
            ],
        };
        let plan = planner.plan(&query).unwrap();
        assert!(plan_contains_limit(&plan));
        assert!(plan_contains_skip(&plan));
    }

    #[test]
    fn plan_return_with_order_by() {
        let store = make_store();
        let planner = QueryPlanner::new(&store, "g");
        let query = Query { unions: Vec::new(),
            clauses: vec![
                Clause::Match(MatchClause {
                    patterns: vec![PathPattern {
                        variable: None,
                        start: NodePattern {
                            variable: Some(Identifier::new("n")),
                            labels: vec![Label::new("Person")],
                            properties: None,
                            where_clause: None,
                        },
                        steps: vec![],
                    }],
                    where_clause: None,
                }),
                Clause::Return(ReturnClause {
                    distinct: false,
                    items: vec![ReturnItem {
                        expr: Expr::Variable(Identifier::new("n")),
                        alias: None,
                    }],
                    order_by: vec![SortItem {
                        expr: Expr::Property(
                            Box::new(Expr::Variable(Identifier::new("n"))),
                            "name".into(),
                        ),
                        ascending: true,
                    }],
                    skip: None,
                    limit: None,
                }),
            ],
        };
        let plan = planner.plan(&query).unwrap();
        assert!(plan_contains_sort(&plan));
    }

    // ── DELETE tests ──────────────────────────────────────────────────────────

    #[test]
    fn plan_delete_basic() {
        // MATCH (n:Person) DELETE n
        let store = make_store();
        let planner = QueryPlanner::new(&store, "g");
        let query = Query { unions: Vec::new(),
            clauses: vec![
                make_match_clause("Person", "n"),
                Clause::Delete(DeleteClause {
                    exprs: vec![Expr::Variable(Identifier::new("n"))],
                    detach: false,
                }),
            ],
        };
        let plan = planner.plan(&query).unwrap();
        assert!(plan_contains_delete(&plan));
    }

    #[test]
    fn plan_detach_delete() {
        // MATCH (n:Person) DETACH DELETE n
        let store = make_store();
        let planner = QueryPlanner::new(&store, "g");
        let query = Query { unions: Vec::new(),
            clauses: vec![
                make_match_clause("Person", "n"),
                Clause::Delete(DeleteClause {
                    exprs: vec![Expr::Variable(Identifier::new("n"))],
                    detach: true,
                }),
            ],
        };
        let plan = planner.plan(&query).unwrap();
        assert!(plan_contains_delete(&plan));
        // Verify detach flag is set.
        assert!(plan_delete_is_detach(&plan));
    }

    #[test]
    fn plan_delete_without_match_is_error() {
        let store = make_store();
        let planner = QueryPlanner::new(&store, "g");
        let query = Query { unions: Vec::new(),
            clauses: vec![Clause::Delete(DeleteClause {
                exprs: vec![Expr::Variable(Identifier::new("n"))],
                detach: false,
            })],
        };
        assert!(planner.plan(&query).is_err());
    }

    // ── WITH tests ────────────────────────────────────────────────────────────

    #[test]
    fn plan_with_simple_projection() {
        // MATCH (n:Person) WITH n RETURN n
        let store = make_store();
        let planner = QueryPlanner::new(&store, "g");
        let query = Query { unions: Vec::new(),
            clauses: vec![
                make_match_clause("Person", "n"),
                Clause::With(WithClause {
                    distinct: false,
                    items: vec![ReturnItem {
                        expr: Expr::Variable(Identifier::new("n")),
                        alias: None,
                    }],
                    where_clause: None,
                    order_by: vec![],
                    skip: None,
                    limit: None,
                }),
                Clause::Return(ReturnClause {
                    distinct: false,
                    items: vec![ReturnItem {
                        expr: Expr::Variable(Identifier::new("n")),
                        alias: None,
                    }],
                    order_by: vec![],
                    skip: None,
                    limit: None,
                }),
            ],
        };
        let plan = planner.plan(&query).unwrap();
        assert!(matches!(plan, Operator::ProduceResults { .. }));
        assert!(plan_contains_project(&plan));
    }

    #[test]
    fn plan_with_where_filter() {
        // MATCH (n:Person) WITH n WHERE n.age > 25 RETURN n
        let store = make_store();
        let planner = QueryPlanner::new(&store, "g");
        let query = Query { unions: Vec::new(),
            clauses: vec![
                make_match_clause("Person", "n"),
                Clause::With(WithClause {
                    distinct: false,
                    items: vec![ReturnItem {
                        expr: Expr::Variable(Identifier::new("n")),
                        alias: None,
                    }],
                    where_clause: Some(Expr::BinaryOp {
                        op: BinaryOp::Gt,
                        left: Box::new(Expr::Property(
                            Box::new(Expr::Variable(Identifier::new("n"))),
                            "age".into(),
                        )),
                        right: Box::new(Expr::Integer(25)),
                    }),
                    order_by: vec![],
                    skip: None,
                    limit: None,
                }),
                Clause::Return(ReturnClause {
                    distinct: false,
                    items: vec![ReturnItem {
                        expr: Expr::Variable(Identifier::new("n")),
                        alias: None,
                    }],
                    order_by: vec![],
                    skip: None,
                    limit: None,
                }),
            ],
        };
        let plan = planner.plan(&query).unwrap();
        assert!(plan_contains_filter(&plan));
        assert!(plan_contains_project(&plan));
    }

    #[test]
    fn plan_with_aggregation() {
        // MATCH (n:Person) WITH n.city AS city, count(n) AS cnt RETURN city, cnt
        let store = make_store();
        let planner = QueryPlanner::new(&store, "g");
        let query = Query { unions: Vec::new(),
            clauses: vec![
                make_match_clause("Person", "n"),
                Clause::With(WithClause {
                    distinct: false,
                    items: vec![
                        ReturnItem {
                            expr: Expr::Property(
                                Box::new(Expr::Variable(Identifier::new("n"))),
                                "city".into(),
                            ),
                            alias: Some(Identifier::new("city")),
                        },
                        ReturnItem {
                            expr: Expr::FunctionCall {
                                name: "count".into(),
                                args: vec![Expr::Variable(Identifier::new("n"))],
                                distinct: false,
                            },
                            alias: Some(Identifier::new("cnt")),
                        },
                    ],
                    where_clause: None,
                    order_by: vec![],
                    skip: None,
                    limit: None,
                }),
                Clause::Return(ReturnClause {
                    distinct: false,
                    items: vec![
                        ReturnItem {
                            expr: Expr::Variable(Identifier::new("city")),
                            alias: None,
                        },
                        ReturnItem {
                            expr: Expr::Variable(Identifier::new("cnt")),
                            alias: None,
                        },
                    ],
                    order_by: vec![],
                    skip: None,
                    limit: None,
                }),
            ],
        };
        let plan = planner.plan(&query).unwrap();
        assert!(plan_contains_aggregate(&plan));
    }

    #[test]
    fn plan_with_order_skip_limit() {
        // MATCH (n:Person) WITH n ORDER BY n.name SKIP 1 LIMIT 5 RETURN n
        let store = make_store();
        let planner = QueryPlanner::new(&store, "g");
        let query = Query { unions: Vec::new(),
            clauses: vec![
                make_match_clause("Person", "n"),
                Clause::With(WithClause {
                    distinct: false,
                    items: vec![ReturnItem {
                        expr: Expr::Variable(Identifier::new("n")),
                        alias: None,
                    }],
                    where_clause: None,
                    order_by: vec![SortItem {
                        expr: Expr::Property(
                            Box::new(Expr::Variable(Identifier::new("n"))),
                            "name".into(),
                        ),
                        ascending: true,
                    }],
                    skip: Some(Expr::Integer(1)),
                    limit: Some(Expr::Integer(5)),
                }),
                Clause::Return(ReturnClause {
                    distinct: false,
                    items: vec![ReturnItem {
                        expr: Expr::Variable(Identifier::new("n")),
                        alias: None,
                    }],
                    order_by: vec![],
                    skip: None,
                    limit: None,
                }),
            ],
        };
        let plan = planner.plan(&query).unwrap();
        assert!(plan_contains_sort(&plan));
        assert!(plan_contains_skip(&plan));
        assert!(plan_contains_limit(&plan));
    }

    #[test]
    fn plan_with_distinct() {
        // MATCH (n:Person) WITH DISTINCT n.city AS city RETURN city
        let store = make_store();
        let planner = QueryPlanner::new(&store, "g");
        let query = Query { unions: Vec::new(),
            clauses: vec![
                make_match_clause("Person", "n"),
                Clause::With(WithClause {
                    distinct: true,
                    items: vec![ReturnItem {
                        expr: Expr::Property(
                            Box::new(Expr::Variable(Identifier::new("n"))),
                            "city".into(),
                        ),
                        alias: Some(Identifier::new("city")),
                    }],
                    where_clause: None,
                    order_by: vec![],
                    skip: None,
                    limit: None,
                }),
                Clause::Return(ReturnClause {
                    distinct: false,
                    items: vec![ReturnItem {
                        expr: Expr::Variable(Identifier::new("city")),
                        alias: None,
                    }],
                    order_by: vec![],
                    skip: None,
                    limit: None,
                }),
            ],
        };
        let plan = planner.plan(&query).unwrap();
        assert!(plan_contains_distinct(&plan));
    }

    // ── UNWIND tests ──────────────────────────────────────────────────────────

    #[test]
    fn plan_unwind_basic() {
        // UNWIND [1, 2, 3] AS x RETURN x
        let store = make_store();
        let planner = QueryPlanner::new(&store, "g");
        let query = Query { unions: Vec::new(),
            clauses: vec![
                Clause::Unwind(UnwindClause {
                    expr: Expr::List(vec![Expr::Integer(1), Expr::Integer(2), Expr::Integer(3)]),
                    alias: Identifier::new("x"),
                }),
                Clause::Return(ReturnClause {
                    distinct: false,
                    items: vec![ReturnItem {
                        expr: Expr::Variable(Identifier::new("x")),
                        alias: None,
                    }],
                    order_by: vec![],
                    skip: None,
                    limit: None,
                }),
            ],
        };
        let plan = planner.plan(&query).unwrap();
        assert!(plan_contains_unwind(&plan));
    }

    #[test]
    fn plan_unwind_after_match() {
        // MATCH (n:Person) UNWIND n.hobbies AS h RETURN n, h
        let store = make_store();
        let planner = QueryPlanner::new(&store, "g");
        let query = Query { unions: Vec::new(),
            clauses: vec![
                make_match_clause("Person", "n"),
                Clause::Unwind(UnwindClause {
                    expr: Expr::Property(
                        Box::new(Expr::Variable(Identifier::new("n"))),
                        "hobbies".into(),
                    ),
                    alias: Identifier::new("h"),
                }),
                Clause::Return(ReturnClause {
                    distinct: false,
                    items: vec![
                        ReturnItem {
                            expr: Expr::Variable(Identifier::new("n")),
                            alias: None,
                        },
                        ReturnItem {
                            expr: Expr::Variable(Identifier::new("h")),
                            alias: None,
                        },
                    ],
                    order_by: vec![],
                    skip: None,
                    limit: None,
                }),
            ],
        };
        let plan = planner.plan(&query).unwrap();
        assert!(plan_contains_unwind(&plan));
        assert!(matches!(plan, Operator::ProduceResults { .. }));
    }

    // ── SET tests ─────────────────────────────────────────────────────────────

    #[test]
    fn plan_set_property() {
        // MATCH (n:Person) SET n.age = 30
        let store = make_store();
        let planner = QueryPlanner::new(&store, "g");
        let query = Query { unions: Vec::new(),
            clauses: vec![
                make_match_clause("Person", "n"),
                Clause::Set(SetClause {
                    items: vec![SetClauseItem::SetProperty {
                        alias: Identifier::new("n"),
                        prop: "age".into(),
                        value: Expr::Integer(30),
                    }],
                }),
            ],
        };
        let plan = planner.plan(&query).unwrap();
        assert!(plan_contains_set(&plan));
    }

    #[test]
    fn plan_set_labels() {
        // MATCH (n:Person) SET n:Admin:Active
        let store = make_store();
        let planner = QueryPlanner::new(&store, "g");
        let query = Query { unions: Vec::new(),
            clauses: vec![
                make_match_clause("Person", "n"),
                Clause::Set(SetClause {
                    items: vec![SetClauseItem::SetLabels {
                        alias: Identifier::new("n"),
                        labels: vec![Label::new("Admin"), Label::new("Active")],
                    }],
                }),
            ],
        };
        let plan = planner.plan(&query).unwrap();
        assert!(plan_contains_set(&plan));
    }

    #[test]
    fn plan_set_map_merge() {
        // MATCH (n:Person) SET n += {age: 30, city: 'NY'}
        let store = make_store();
        let planner = QueryPlanner::new(&store, "g");
        let query = Query { unions: Vec::new(),
            clauses: vec![
                make_match_clause("Person", "n"),
                Clause::Set(SetClause {
                    items: vec![SetClauseItem::SetMap {
                        alias: Identifier::new("n"),
                        map: Expr::MapLiteral(vec![
                            (smol_str::SmolStr::new("age"), Expr::Integer(30)),
                            (smol_str::SmolStr::new("city"), Expr::Str("NY".into())),
                        ]),
                        merge: true,
                    }],
                }),
            ],
        };
        let plan = planner.plan(&query).unwrap();
        assert!(plan_contains_set(&plan));
    }

    #[test]
    fn plan_set_without_match_is_error() {
        let store = make_store();
        let planner = QueryPlanner::new(&store, "g");
        let query = Query { unions: Vec::new(),
            clauses: vec![Clause::Set(SetClause {
                items: vec![SetClauseItem::SetProperty {
                    alias: Identifier::new("n"),
                    prop: "age".into(),
                    value: Expr::Integer(30),
                }],
            })],
        };
        assert!(planner.plan(&query).is_err());
    }

    // ── REMOVE tests ──────────────────────────────────────────────────────────

    #[test]
    fn plan_remove_property() {
        // MATCH (n:Person) REMOVE n.age
        let store = make_store();
        let planner = QueryPlanner::new(&store, "g");
        let query = Query { unions: Vec::new(),
            clauses: vec![
                make_match_clause("Person", "n"),
                Clause::Remove(RemoveClause {
                    items: vec![RemoveClauseItem::RemoveProperty {
                        alias: Identifier::new("n"),
                        prop: "age".into(),
                    }],
                }),
            ],
        };
        let plan = planner.plan(&query).unwrap();
        assert!(plan_contains_remove(&plan));
    }

    #[test]
    fn plan_remove_labels() {
        // MATCH (n:Person) REMOVE n:Admin
        let store = make_store();
        let planner = QueryPlanner::new(&store, "g");
        let query = Query { unions: Vec::new(),
            clauses: vec![
                make_match_clause("Person", "n"),
                Clause::Remove(RemoveClause {
                    items: vec![RemoveClauseItem::RemoveLabels {
                        alias: Identifier::new("n"),
                        labels: vec![Label::new("Admin")],
                    }],
                }),
            ],
        };
        let plan = planner.plan(&query).unwrap();
        assert!(plan_contains_remove(&plan));
    }

    #[test]
    fn plan_remove_without_match_is_error() {
        let store = make_store();
        let planner = QueryPlanner::new(&store, "g");
        let query = Query { unions: Vec::new(),
            clauses: vec![Clause::Remove(RemoveClause {
                items: vec![RemoveClauseItem::RemoveProperty {
                    alias: Identifier::new("n"),
                    prop: "age".into(),
                }],
            })],
        };
        assert!(planner.plan(&query).is_err());
    }

    // ── MERGE tests ───────────────────────────────────────────────────────────

    #[test]
    fn plan_merge_basic() {
        // MERGE (n:Person)
        let store = make_store();
        let planner = QueryPlanner::new(&store, "g");
        let query = Query { unions: Vec::new(),
            clauses: vec![Clause::Merge(MergeClause {
                pattern: PathPattern {
                    variable: None,
                    start: NodePattern {
                        variable: Some(Identifier::new("n")),
                        labels: vec![Label::new("Person")],
                        properties: None,
                        where_clause: None,
                    },
                    steps: vec![],
                },
                on_create: vec![],
                on_match: vec![],
            })],
        };
        let plan = planner.plan(&query).unwrap();
        assert!(plan_contains_merge(&plan));
    }

    #[test]
    fn plan_merge_with_on_create_on_match() {
        // MERGE (n:Person)
        //   ON CREATE SET n.created = true
        //   ON MATCH SET n.accessed = true
        let store = make_store();
        let planner = QueryPlanner::new(&store, "g");
        let query = Query { unions: Vec::new(),
            clauses: vec![Clause::Merge(MergeClause {
                pattern: PathPattern {
                    variable: None,
                    start: NodePattern {
                        variable: Some(Identifier::new("n")),
                        labels: vec![Label::new("Person")],
                        properties: None,
                        where_clause: None,
                    },
                    steps: vec![],
                },
                on_create: vec![SetClauseItem::SetProperty {
                    alias: Identifier::new("n"),
                    prop: "created".into(),
                    value: Expr::Bool(true),
                }],
                on_match: vec![SetClauseItem::SetProperty {
                    alias: Identifier::new("n"),
                    prop: "accessed".into(),
                    value: Expr::Bool(true),
                }],
            })],
        };
        let plan = planner.plan(&query).unwrap();
        assert!(plan_contains_merge(&plan));
        // Verify merge has ON CREATE and ON MATCH items.
        assert!(plan_merge_has_actions(&plan));
    }

    #[test]
    fn plan_merge_after_match() {
        // MATCH (a:Person) MERGE (n:City)
        let store = make_store();
        let planner = QueryPlanner::new(&store, "g");
        let query = Query { unions: Vec::new(),
            clauses: vec![
                make_match_clause("Person", "a"),
                Clause::Merge(MergeClause {
                    pattern: PathPattern {
                        variable: None,
                        start: NodePattern {
                            variable: Some(Identifier::new("n")),
                            labels: vec![Label::new("City")],
                            properties: None,
                            where_clause: None,
                        },
                        steps: vec![],
                    },
                    on_create: vec![],
                    on_match: vec![],
                }),
            ],
        };
        let plan = planner.plan(&query).unwrap();
        assert!(plan_contains_merge(&plan));
    }

    // ── OPTIONAL MATCH tests ──────────────────────────────────────────────────

    #[test]
    fn plan_optional_match_basic() {
        // MATCH (n:Person) OPTIONAL MATCH (n)-[:KNOWS]->(m) RETURN n, m
        let store = make_store();
        let planner = QueryPlanner::new(&store, "g");
        let query = Query { unions: Vec::new(),
            clauses: vec![
                make_match_clause("Person", "n"),
                Clause::OptionalMatch(MatchClause {
                    patterns: vec![PathPattern {
                        variable: None,
                        start: NodePattern {
                            variable: Some(Identifier::new("n")),
                            labels: vec![],
                            properties: None,
                            where_clause: None,
                        },
                        steps: vec![PathStep {
                            rel: RelPattern {
                                variable: None,
                                rel_types: vec![crate::ast::RelType::new("KNOWS")],
                                direction: Direction::Outgoing,
                                range: None,
                                properties: None,
                                where_clause: None,
                            },
                            node: NodePattern {
                                variable: Some(Identifier::new("m")),
                                labels: vec![],
                                properties: None,
                                where_clause: None,
                            },
                        }],
                    }],
                    where_clause: None,
                }),
                Clause::Return(ReturnClause {
                    distinct: false,
                    items: vec![
                        ReturnItem {
                            expr: Expr::Variable(Identifier::new("n")),
                            alias: None,
                        },
                        ReturnItem {
                            expr: Expr::Variable(Identifier::new("m")),
                            alias: None,
                        },
                    ],
                    order_by: vec![],
                    skip: None,
                    limit: None,
                }),
            ],
        };
        let plan = planner.plan(&query).unwrap();
        assert!(plan_contains_left_outer_apply(&plan));
    }

    #[test]
    fn plan_optional_match_without_prior_match() {
        // OPTIONAL MATCH (n:Person) RETURN n
        let store = make_store();
        let planner = QueryPlanner::new(&store, "g");
        let query = Query { unions: Vec::new(),
            clauses: vec![
                Clause::OptionalMatch(MatchClause {
                    patterns: vec![PathPattern {
                        variable: None,
                        start: NodePattern {
                            variable: Some(Identifier::new("n")),
                            labels: vec![Label::new("Person")],
                            properties: None,
                            where_clause: None,
                        },
                        steps: vec![],
                    }],
                    where_clause: None,
                }),
                Clause::Return(ReturnClause {
                    distinct: false,
                    items: vec![ReturnItem {
                        expr: Expr::Variable(Identifier::new("n")),
                        alias: None,
                    }],
                    order_by: vec![],
                    skip: None,
                    limit: None,
                }),
            ],
        };
        let plan = planner.plan(&query).unwrap();
        // Should still produce a LeftOuterApply even without a prior MATCH.
        assert!(plan_contains_left_outer_apply(&plan));
    }

    // ── Multi-pattern MATCH tests ─────────────────────────────────────────────

    #[test]
    fn plan_multi_pattern_match() {
        // MATCH (a:Person), (b:Movie) RETURN a, b
        let store = make_store();
        let planner = QueryPlanner::new(&store, "g");
        let query = Query { unions: Vec::new(),
            clauses: vec![
                Clause::Match(MatchClause {
                    patterns: vec![
                        PathPattern {
                            variable: None,
                            start: NodePattern {
                                variable: Some(Identifier::new("a")),
                                labels: vec![Label::new("Person")],
                                properties: None,
                                where_clause: None,
                            },
                            steps: vec![],
                        },
                        PathPattern {
                            variable: None,
                            start: NodePattern {
                                variable: Some(Identifier::new("b")),
                                labels: vec![Label::new("Movie")],
                                properties: None,
                                where_clause: None,
                            },
                            steps: vec![],
                        },
                    ],
                    where_clause: None,
                }),
                Clause::Return(ReturnClause {
                    distinct: false,
                    items: vec![
                        ReturnItem {
                            expr: Expr::Variable(Identifier::new("a")),
                            alias: None,
                        },
                        ReturnItem {
                            expr: Expr::Variable(Identifier::new("b")),
                            alias: None,
                        },
                    ],
                    order_by: vec![],
                    skip: None,
                    limit: None,
                }),
            ],
        };
        let plan = planner.plan(&query).unwrap();
        // Multi-pattern MATCH produces an Apply (cross-product) in the tree.
        assert!(plan_contains_apply(&plan));
    }

    #[test]
    fn plan_multi_pattern_match_with_where() {
        // MATCH (a:Person), (b:Movie) WHERE a.name = 'Alice' RETURN a, b
        let store = make_store();
        let planner = QueryPlanner::new(&store, "g");
        let query = Query { unions: Vec::new(),
            clauses: vec![
                Clause::Match(MatchClause {
                    patterns: vec![
                        PathPattern {
                            variable: None,
                            start: NodePattern {
                                variable: Some(Identifier::new("a")),
                                labels: vec![Label::new("Person")],
                                properties: None,
                                where_clause: None,
                            },
                            steps: vec![],
                        },
                        PathPattern {
                            variable: None,
                            start: NodePattern {
                                variable: Some(Identifier::new("b")),
                                labels: vec![Label::new("Movie")],
                                properties: None,
                                where_clause: None,
                            },
                            steps: vec![],
                        },
                    ],
                    where_clause: Some(Expr::BinaryOp {
                        op: BinaryOp::Eq,
                        left: Box::new(Expr::Property(
                            Box::new(Expr::Variable(Identifier::new("a"))),
                            "name".into(),
                        )),
                        right: Box::new(Expr::Str("Alice".into())),
                    }),
                }),
                Clause::Return(ReturnClause {
                    distinct: false,
                    items: vec![
                        ReturnItem {
                            expr: Expr::Variable(Identifier::new("a")),
                            alias: None,
                        },
                        ReturnItem {
                            expr: Expr::Variable(Identifier::new("b")),
                            alias: None,
                        },
                    ],
                    order_by: vec![],
                    skip: None,
                    limit: None,
                }),
            ],
        };
        let plan = planner.plan(&query).unwrap();
        assert!(plan_contains_apply(&plan));
        assert!(plan_contains_filter(&plan));
    }

    // ── Index scan tests ──────────────────────────────────────────────────────

    #[test]
    fn plan_node_scan_upgrades_to_index_scan_when_index_exists() {
        // Given a store with an index on Person.name
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();
        store.create_index("g", "Person", "name").unwrap();

        let planner = QueryPlanner::new(&store, "g");
        // MATCH (n:Person {name: 'Alice'}) RETURN n
        let query = Query { unions: Vec::new(),
            clauses: vec![
                Clause::Match(MatchClause {
                    patterns: vec![PathPattern {
                        variable: None,
                        start: NodePattern {
                            variable: Some(Identifier::new("n")),
                            labels: vec![Label::new("Person")],
                            properties: Some(vec![(
                                smol_str::SmolStr::new("name"),
                                Expr::Str("Alice".into()),
                            )]),
                            where_clause: None,
                        },
                        steps: vec![],
                    }],
                    where_clause: None,
                }),
                Clause::Return(ReturnClause {
                    distinct: false,
                    items: vec![ReturnItem {
                        expr: Expr::Variable(Identifier::new("n")),
                        alias: None,
                    }],
                    order_by: vec![],
                    skip: None,
                    limit: None,
                }),
            ],
        };
        let plan = planner.plan(&query).unwrap();
        assert!(
            plan_contains_index_scan(&plan),
            "Expected IndexScan in plan when index exists for Person.name, got: {plan:?}"
        );
    }

    #[test]
    fn plan_node_scan_stays_node_scan_when_no_index() {
        // No index created -- must fall back to NodeScan + Filter.
        let store = make_store(); // no indexes
        let planner = QueryPlanner::new(&store, "g");
        let query = Query { unions: Vec::new(),
            clauses: vec![
                Clause::Match(MatchClause {
                    patterns: vec![PathPattern {
                        variable: None,
                        start: NodePattern {
                            variable: Some(Identifier::new("n")),
                            labels: vec![Label::new("Person")],
                            properties: Some(vec![(
                                smol_str::SmolStr::new("name"),
                                Expr::Str("Alice".into()),
                            )]),
                            where_clause: None,
                        },
                        steps: vec![],
                    }],
                    where_clause: None,
                }),
                Clause::Return(ReturnClause {
                    distinct: false,
                    items: vec![ReturnItem {
                        expr: Expr::Variable(Identifier::new("n")),
                        alias: None,
                    }],
                    order_by: vec![],
                    skip: None,
                    limit: None,
                }),
            ],
        };
        let plan = planner.plan(&query).unwrap();
        assert!(
            !plan_contains_index_scan(&plan),
            "Expected NodeScan (not IndexScan) when no index exists, got: {plan:?}"
        );
        // Should still have a Filter for the inline property predicate.
        assert!(
            plan_contains_filter(&plan),
            "Expected Filter wrapping NodeScan when no index, got: {plan:?}"
        );
    }

    // ── Operator tree search helpers ──────────────────────────────────────────

    fn plan_contains_filter(op: &Operator) -> bool {
        match op {
            Operator::Filter { .. } => true,
            _ => children_of(op).iter().any(|c| plan_contains_filter(c)),
        }
    }

    fn plan_contains_limit(op: &Operator) -> bool {
        match op {
            Operator::Limit { .. } => true,
            _ => children_of(op).iter().any(|c| plan_contains_limit(c)),
        }
    }

    fn plan_contains_skip(op: &Operator) -> bool {
        match op {
            Operator::Skip { .. } => true,
            _ => children_of(op).iter().any(|c| plan_contains_skip(c)),
        }
    }

    fn plan_contains_sort(op: &Operator) -> bool {
        match op {
            Operator::Sort { .. } => true,
            _ => children_of(op).iter().any(|c| plan_contains_sort(c)),
        }
    }

    fn plan_contains_delete(op: &Operator) -> bool {
        match op {
            Operator::Delete { .. } => true,
            _ => children_of(op).iter().any(|c| plan_contains_delete(c)),
        }
    }

    fn plan_delete_is_detach(op: &Operator) -> bool {
        match op {
            Operator::Delete { detach, .. } => *detach,
            _ => children_of(op).iter().any(|c| plan_delete_is_detach(c)),
        }
    }

    fn plan_contains_project(op: &Operator) -> bool {
        match op {
            Operator::Project { .. } => true,
            _ => children_of(op).iter().any(|c| plan_contains_project(c)),
        }
    }

    fn plan_contains_aggregate(op: &Operator) -> bool {
        match op {
            Operator::Aggregate { .. } => true,
            _ => children_of(op).iter().any(|c| plan_contains_aggregate(c)),
        }
    }

    fn plan_contains_unwind(op: &Operator) -> bool {
        match op {
            Operator::Unwind { .. } => true,
            _ => children_of(op).iter().any(|c| plan_contains_unwind(c)),
        }
    }

    fn plan_contains_set(op: &Operator) -> bool {
        match op {
            Operator::Set { .. } => true,
            _ => children_of(op).iter().any(|c| plan_contains_set(c)),
        }
    }

    fn plan_contains_remove(op: &Operator) -> bool {
        match op {
            Operator::Remove { .. } => true,
            _ => children_of(op).iter().any(|c| plan_contains_remove(c)),
        }
    }

    fn plan_contains_merge(op: &Operator) -> bool {
        match op {
            Operator::Merge { .. } => true,
            _ => children_of(op).iter().any(|c| plan_contains_merge(c)),
        }
    }

    fn plan_merge_has_actions(op: &Operator) -> bool {
        match op {
            Operator::Merge {
                on_create,
                on_match,
                ..
            } => !on_create.is_empty() && !on_match.is_empty(),
            _ => children_of(op).iter().any(|c| plan_merge_has_actions(c)),
        }
    }

    fn plan_contains_left_outer_apply(op: &Operator) -> bool {
        match op {
            Operator::LeftOuterApply { .. } => true,
            _ => children_of(op)
                .iter()
                .any(|c| plan_contains_left_outer_apply(c)),
        }
    }

    fn plan_contains_apply(op: &Operator) -> bool {
        match op {
            Operator::Apply { .. } => true,
            _ => children_of(op).iter().any(|c| plan_contains_apply(c)),
        }
    }

    fn plan_contains_distinct(op: &Operator) -> bool {
        match op {
            Operator::Distinct { .. } => true,
            _ => children_of(op).iter().any(|c| plan_contains_distinct(c)),
        }
    }

    fn plan_contains_index_scan(op: &Operator) -> bool {
        match op {
            Operator::IndexScan { .. } => true,
            _ => children_of(op).iter().any(|c| plan_contains_index_scan(c)),
        }
    }

    /// Extract all child operators from an operator for recursive tree searches.
    fn children_of(op: &Operator) -> Vec<&Operator> {
        match op {
            Operator::SingleRow
            | Operator::NodeScan { .. }
            | Operator::IndexScan { .. }
            | Operator::NodeByIdSeek { .. }
            | Operator::ShortestPath { .. } => vec![],

            Operator::Filter { child, .. }
            | Operator::Project { child, .. }
            | Operator::Aggregate { child, .. }
            | Operator::Distinct { child }
            | Operator::Sort { child, .. }
            | Operator::Limit { child, .. }
            | Operator::Skip { child, .. }
            | Operator::Unwind { child, .. }
            | Operator::Set { child, .. }
            | Operator::Remove { child, .. }
            | Operator::Delete { child, .. }
            | Operator::ProduceResults { child, .. }
            | Operator::Expand { child, .. }
            | Operator::VarLenExpand { child, .. }
            | Operator::AssemblePath { child, .. } => vec![child],

            Operator::Apply { outer, inner }
            | Operator::LeftOuterApply {
                preserved: outer,
                optional_pattern: inner,
            } => vec![outer, inner],

            Operator::Union { left, right, .. } => vec![left, right],

            Operator::Create { child, .. } => match child {
                Some(c) => vec![c],
                None => vec![],
            },

            Operator::Merge { child, .. } => match child {
                Some(c) => vec![c],
                None => vec![],
            },

            Operator::Call { child, .. } => match child {
                Some(c) => vec![c],
                None => vec![],
            },
            Operator::MergeRelationship { child, .. } => vec![child],
        }
    }
}

// ── Static defaults for backward compatibility ──────────────────────────────

static EMPTY_REGISTRY: std::sync::OnceLock<ProcedureRegistry> = std::sync::OnceLock::new();
static EMPTY_PARAMS: std::sync::OnceLock<HashMap<String, Value>> = std::sync::OnceLock::new();

fn empty_registry() -> &'static ProcedureRegistry {
    EMPTY_REGISTRY.get_or_init(ProcedureRegistry::new)
}

fn empty_params() -> &'static HashMap<String, Value> {
    EMPTY_PARAMS.get_or_init(HashMap::new)
}

// ── Helper functions ─────────────────────────────────────────────────────────

/// Convert a literal expression to a Value for type checking.
fn expr_to_literal_value(expr: &Expr) -> Option<Value> {
    match expr {
        Expr::Integer(n) => Some(Value::Int(*n)),
        Expr::Float(f) => Some(Value::Float(*f)),
        Expr::Str(s) => Some(Value::Str(s.clone())),
        Expr::Bool(b) => Some(Value::Bool(*b)),
        Expr::Null => Some(Value::Null),
        _ => None, // Non-literal expressions can't be type-checked at plan time
    }
}

/// Check if an expression contains an aggregation function.
fn contains_aggregation(expr: &Expr) -> bool {
    match expr {
        Expr::FunctionCall { name, args, .. } => {
            let agg_names = [
                "count", "sum", "avg", "min", "max", "collect", "stdev", "stdevp",
                "percentilecont", "percentiledisc",
            ];
            if agg_names.contains(&name.to_lowercase().as_str()) {
                return true;
            }
            args.iter().any(contains_aggregation)
        }
        Expr::BinaryOp { left, right, .. } => {
            contains_aggregation(left) || contains_aggregation(right)
        }
        Expr::UnaryOp { expr: inner, .. } => contains_aggregation(inner),
        _ => false,
    }
}
