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
//!
//! Unsupported constructs (OPTIONAL MATCH, MERGE, WITH, UNWIND, SET, REMOVE, DELETE)
//! return `GqlError::Execution(ExecutionError::UnsupportedFeature)`.

use crate::ast::{
    Clause, CreateClause, Expr, Identifier, Label, MatchClause, PathPattern, PathStep, Query,
    ReturnClause, ReturnItem,
};
use crate::error::{ExecutionError, GqlError, PlanError};
use crate::planner::{AggFunction, CreatePattern, Operator};
use crate::storage::GraphStore;

// ── Planner ───────────────────────────────────────────────────────────────────

/// Translates a [`Query`] AST into a physical [`Operator`] tree.
pub struct QueryPlanner<'s> {
    store: &'s dyn GraphStore,
    graph: &'s str,
}

impl<'s> QueryPlanner<'s> {
    /// Create a new planner backed by `store` targeting `graph`.
    pub fn new(store: &'s dyn GraphStore, graph: &'s str) -> Self {
        Self { store, graph }
    }

    /// Plan a query: produce the root [`Operator`] for execution.
    pub fn plan(&self, query: &Query) -> Result<Operator, GqlError> {
        let mut pipeline: Option<Operator> = None;
        let mut return_clause: Option<&ReturnClause> = None;

        for clause in &query.clauses {
            match clause {
                Clause::Match(mc) => {
                    let op = self.plan_match(mc)?;
                    pipeline = Some(match pipeline {
                        None => op,
                        Some(prev) => {
                            // Chain: apply operator on top of previous pipeline.
                            // For now, nested MATCH clauses are treated as independent scans
                            // joined via a cross-product (simple Apply placeholder).
                            // The executor will get UnsupportedFeature for Apply anyway,
                            // but we keep the structure valid.
                            Operator::Apply {
                                outer: Box::new(prev),
                                inner: Box::new(op),
                            }
                        }
                    });
                }

                Clause::OptionalMatch(_) => {
                    return Err(GqlError::Execution(ExecutionError::unsupported(
                        "OPTIONAL MATCH",
                    )));
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

                Clause::Delete(_) => {
                    return Err(GqlError::Execution(ExecutionError::unsupported("DELETE")));
                }

                Clause::With(_) => {
                    return Err(GqlError::Execution(ExecutionError::unsupported("WITH")));
                }

                Clause::Unwind(_) => {
                    return Err(GqlError::Execution(ExecutionError::unsupported("UNWIND")));
                }

                Clause::Set(_) => {
                    return Err(GqlError::Execution(ExecutionError::unsupported("SET")));
                }

                Clause::Remove(_) => {
                    return Err(GqlError::Execution(ExecutionError::unsupported("REMOVE")));
                }

                Clause::Merge(_) => {
                    return Err(GqlError::Execution(ExecutionError::unsupported("MERGE")));
                }
            }
        }

        // If we have a RETURN clause, build the return pipeline.
        if let Some(rc) = return_clause {
            let child = pipeline.unwrap_or_else(|| {
                // RETURN without MATCH: just return the literal expressions.
                // Use a single-row "leaf" represented by an empty NodeScan.
                Operator::NodeScan {
                    label: None,
                    alias: Identifier::new("__empty__"),
                }
            });
            return self.plan_return(rc, child);
        }

        // If we only have a CREATE (no RETURN), wrap in ProduceResults with no columns.
        if let Some(op) = pipeline {
            return Ok(Operator::ProduceResults {
                columns: vec![],
                child: Box::new(op),
            });
        }

        Err(GqlError::Plan(PlanError::ReturnStarNoVariables))
    }

    // ── MATCH planning ────────────────────────────────────────────────────────

    fn plan_match(&self, mc: &MatchClause) -> Result<Operator, GqlError> {
        if mc.patterns.is_empty() {
            return Err(GqlError::Plan(PlanError::ReturnStarNoVariables));
        }

        // Plan each pattern, then join them (multiple patterns = cross-product in
        // current Phase 2 implementation; Apply is returned for multi-pattern matches).
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

    /// Plan a single `PathPattern` into nested `NodeScan` + `Expand` operators.
    fn plan_path_pattern(&self, pattern: &PathPattern) -> Result<Operator, GqlError> {
        let start = &pattern.start;
        let start_alias = start
            .variable
            .clone()
            .unwrap_or_else(|| Identifier::new("__anon0__"));
        let start_label = start.labels.first().cloned();

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
                op = Operator::VarLenExpand {
                    direction: rel.direction,
                    rel_types: rel.rel_types.clone(),
                    min,
                    max,
                    src_alias: start_alias.clone(),
                    path_alias: rel.variable.clone(),
                    dst_alias: dst_alias.clone(),
                    child: Box::new(op),
                };
            } else {
                // Single-hop expand.
                let rel_alias = rel.variable.clone();
                op = Operator::Expand {
                    direction: rel.direction,
                    rel_types: rel.rel_types.clone(),
                    src_alias: start_alias.clone(),
                    rel_alias,
                    dst_alias: dst_alias.clone(),
                    child: Box::new(op),
                };
            }

            // If the destination node has label constraints, add a label filter.
            if let Some(dst_label) = node.labels.first() {
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

        for path_pattern in &cc.patterns {
            let mut nodes: Vec<crate::planner::CreateNodeSpec> = Vec::new();
            let mut edges: Vec<crate::planner::CreateEdgeSpec> = Vec::new();

            // Extract nodes from the pattern.
            let start = &path_pattern.start;
            let start_alias = start.variable.clone();
            let node_props = start
                .properties
                .as_ref()
                .map(|p| {
                    p.iter()
                        .map(|(k, v)| (k.to_string(), v.clone()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            nodes.push((start_alias.clone(), start.labels.clone(), node_props));

            for (step_idx, step) in path_pattern.steps.iter().enumerate() {
                let PathStep { rel, node } = step;

                // Destination node.
                let dst_alias = node.variable.clone();
                let dst_props = node
                    .properties
                    .as_ref()
                    .map(|p| {
                        p.iter()
                            .map(|(k, v)| (k.to_string(), v.clone()))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                nodes.push((dst_alias.clone(), node.labels.clone(), dst_props));

                // Edge specification.
                let rel_type = rel.rel_types.first().ok_or_else(|| {
                    GqlError::Plan(PlanError::UndefinedVariable {
                        name: format!("CREATE relationship at step {step_idx} has no type"),
                        offset: 0,
                    })
                })?;

                let src_for_edge = if step_idx == 0 {
                    start_alias
                        .clone()
                        .unwrap_or_else(|| Identifier::new("__src__"))
                } else {
                    // Previous step's destination node alias.
                    path_pattern.steps[step_idx - 1]
                        .node
                        .variable
                        .clone()
                        .unwrap_or_else(|| Identifier::new(format!("__node{}__", step_idx)))
                };

                let dst_for_edge = dst_alias
                    .unwrap_or_else(|| Identifier::new(format!("__node{}__", step_idx + 1)));

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
            }

            patterns.push(CreatePattern { nodes, edges });
        }

        Ok(Operator::Create {
            patterns,
            child: child.map(Box::new),
        })
    }

    // ── RETURN planning ───────────────────────────────────────────────────────

    fn plan_return(&self, rc: &ReturnClause, child: Operator) -> Result<Operator, GqlError> {
        // Determine whether any item uses an aggregate function.
        let has_aggregation = rc.items.iter().any(|item| has_agg_function(&item.expr));

        // For aggregation, wrap the pipeline now so ORDER BY can sort on aggregate output.
        // For plain queries, Sort/Distinct/Skip/Limit all run on the raw variable rows,
        // BEFORE ProduceResults does the final projection.  This matches Cypher semantics:
        // ORDER BY `n.name` sorts on the real value, not on the projection alias.
        let pipeline = if has_aggregation {
            self.plan_aggregation(rc, child)?
        } else {
            child
        };

        // ORDER BY — sort expressions reference raw variable names.
        let pipeline = if !rc.order_by.is_empty() {
            Operator::Sort {
                items: rc
                    .order_by
                    .iter()
                    .map(|s| (s.expr.clone(), s.ascending))
                    .collect(),
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
        let columns: Vec<(String, Expr)> = rc
            .items
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

    fn plan_aggregation(&self, rc: &ReturnClause, child: Operator) -> Result<Operator, GqlError> {
        let mut keys: Vec<Expr> = Vec::new();
        let mut aggregations: Vec<(AggFunction, Expr, bool, Identifier)> = Vec::new();

        for item in &rc.items {
            if has_agg_function(&item.expr) {
                let (func, arg, distinct) = extract_agg_function(&item.expr)?;
                let alias = item
                    .alias
                    .clone()
                    .unwrap_or_else(|| Identifier::new(return_item_name(item)));
                aggregations.push((func, arg, distinct, alias));
            } else {
                keys.push(item.expr.clone());
            }
        }

        Ok(Operator::Aggregate {
            keys,
            aggregations,
            child: Box::new(child),
        })
    }
}

// ── Free helpers ──────────────────────────────────────────────────────────────

/// Return the output column name for a RETURN item.
fn return_item_name(item: &ReturnItem) -> String {
    if let Some(alias) = &item.alias {
        return alias.0.clone();
    }
    match &item.expr {
        Expr::Variable(id) => id.0.clone(),
        Expr::Property(obj, key) => {
            format!("{}.{}", expr_name(obj), key)
        }
        Expr::FunctionCall { name, .. } => name.to_string(),
        other => format!("{:?}", other),
    }
}

fn expr_name(expr: &Expr) -> String {
    match expr {
        Expr::Variable(id) => id.0.clone(),
        Expr::Property(obj, key) => format!("{}.{}", expr_name(obj), key),
        _ => "expr".to_string(),
    }
}

/// Returns `true` if the expression is or contains an aggregate function call.
fn has_agg_function(expr: &Expr) -> bool {
    match expr {
        Expr::FunctionCall { name, .. } => is_agg_name(name.as_str()),
        Expr::BinaryOp { left, right, .. } => has_agg_function(left) || has_agg_function(right),
        Expr::UnaryOp { expr, .. } => has_agg_function(expr),
        Expr::Property(obj, _) => has_agg_function(obj),
        Expr::List(items) => items.iter().any(has_agg_function),
        Expr::ListComp {
            list, filter, map, ..
        } => {
            has_agg_function(list)
                || filter.as_deref().map(has_agg_function).unwrap_or(false)
                || has_agg_function(map)
        }
        _ => false,
    }
}

fn is_agg_name(name: &str) -> bool {
    matches!(
        name.to_uppercase().as_str(),
        "COUNT" | "SUM" | "AVG" | "MIN" | "MAX" | "COLLECT" | "STDEV" | "STDEVP"
    )
}

/// Extract the aggregate function, argument, and distinct flag from a top-level
/// `FunctionCall` expression.
fn extract_agg_function(expr: &Expr) -> Result<(AggFunction, Expr, bool), GqlError> {
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
                other => {
                    return Err(GqlError::Execution(ExecutionError::unsupported(format!(
                        "aggregate function '{other}'"
                    ))));
                }
            };
            let arg = args.first().cloned().unwrap_or(Expr::Null);
            Ok((func, arg, *distinct))
        }
        _ => Err(GqlError::Execution(ExecutionError::unsupported(
            "non-function aggregate expression",
        ))),
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{
        Direction, Expr, Identifier, Label, MatchClause, NodePattern, PathPattern, PathStep, Query,
        RelPattern, RelType, ReturnClause, ReturnItem, SortItem,
    };
    use crate::storage::InMemoryGraphStore;

    fn make_store() -> InMemoryGraphStore {
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();
        store
    }

    fn make_simple_match_query(label: &str, alias: &str) -> Query {
        Query {
            clauses: vec![
                Clause::Match(MatchClause {
                    patterns: vec![PathPattern {
                        variable: None,
                        start: NodePattern {
                            variable: Some(Identifier::new(alias)),
                            labels: vec![Label::new(label)],
                            properties: None,
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
        let query = Query {
            clauses: vec![
                Clause::Match(MatchClause {
                    patterns: vec![PathPattern {
                        variable: None,
                        start: NodePattern {
                            variable: Some(Identifier::new("n")),
                            labels: vec![Label::new("Person")],
                            properties: None,
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
        let query = Query {
            clauses: vec![
                Clause::Match(MatchClause {
                    patterns: vec![PathPattern {
                        variable: None,
                        start: NodePattern {
                            variable: Some(Identifier::new("n")),
                            labels: vec![Label::new("Person")],
                            properties: None,
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
        let query = Query {
            clauses: vec![
                Clause::Match(MatchClause {
                    patterns: vec![PathPattern {
                        variable: None,
                        start: NodePattern {
                            variable: Some(Identifier::new("n")),
                            labels: vec![Label::new("Person")],
                            properties: None,
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

    fn plan_contains_filter(op: &Operator) -> bool {
        match op {
            Operator::Filter { .. } => true,
            Operator::ProduceResults { child, .. } => plan_contains_filter(child),
            Operator::Project { child, .. } => plan_contains_filter(child),
            Operator::Sort { child, .. } => plan_contains_filter(child),
            Operator::Limit { child, .. } => plan_contains_filter(child),
            Operator::Skip { child, .. } => plan_contains_filter(child),
            Operator::Aggregate { child, .. } => plan_contains_filter(child),
            Operator::Expand { child, .. } => plan_contains_filter(child),
            _ => false,
        }
    }

    fn plan_contains_limit(op: &Operator) -> bool {
        match op {
            Operator::Limit { .. } => true,
            Operator::ProduceResults { child, .. } => plan_contains_limit(child),
            Operator::Sort { child, .. } => plan_contains_limit(child),
            Operator::Skip { child, .. } => plan_contains_limit(child),
            _ => false,
        }
    }

    fn plan_contains_skip(op: &Operator) -> bool {
        match op {
            Operator::Skip { .. } => true,
            Operator::ProduceResults { child, .. } => plan_contains_skip(child),
            Operator::Sort { child, .. } => plan_contains_skip(child),
            Operator::Limit { child, .. } => plan_contains_skip(child),
            _ => false,
        }
    }

    fn plan_contains_sort(op: &Operator) -> bool {
        match op {
            Operator::Sort { .. } => true,
            Operator::ProduceResults { child, .. } => plan_contains_sort(child),
            Operator::Limit { child, .. } => plan_contains_sort(child),
            Operator::Skip { child, .. } => plan_contains_sort(child),
            _ => false,
        }
    }

    fn plan_contains_index_scan(op: &Operator) -> bool {
        match op {
            Operator::IndexScan { .. } => true,
            Operator::ProduceResults { child, .. } => plan_contains_index_scan(child),
            Operator::Filter { child, .. } => plan_contains_index_scan(child),
            Operator::Sort { child, .. } => plan_contains_index_scan(child),
            Operator::Limit { child, .. } => plan_contains_index_scan(child),
            Operator::Skip { child, .. } => plan_contains_index_scan(child),
            Operator::Aggregate { child, .. } => plan_contains_index_scan(child),
            Operator::Expand { child, .. } => plan_contains_index_scan(child),
            _ => false,
        }
    }

    #[test]
    fn plan_node_scan_upgrades_to_index_scan_when_index_exists() {
        // Given a store with an index on Person.name
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();
        store.create_index("g", "Person", "name").unwrap();

        let planner = QueryPlanner::new(&store, "g");
        // MATCH (n:Person {name: 'Alice'}) RETURN n
        let query = Query {
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
        // No index created — must fall back to NodeScan + Filter.
        let store = make_store(); // no indexes
        let planner = QueryPlanner::new(&store, "g");
        let query = Query {
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
}
