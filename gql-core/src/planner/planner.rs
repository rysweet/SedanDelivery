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

use crate::ast::{
    Clause, CreateClause, DeleteClause, Expr, Identifier, Label, MatchClause, MergeClause,
    PathPattern, PathStep, Query, RemoveClause, RemoveClauseItem, ReturnClause, ReturnItem,
    SetClause, SetClauseItem, UnwindClause, WithClause,
};
use crate::error::{ExecutionError, GqlError, PlanError};
use crate::planner::{AggFunction, CreatePattern, Operator, RemoveItem, SetItem};
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
                    let preserved = pipeline.unwrap_or_else(|| Operator::NodeScan {
                        label: None,
                        alias: Identifier::new("__empty__"),
                    });
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
                    // CALL clause: return an unsupported-feature error for now.
                    // This allows TCK tests to detect a runtime error rather than
                    // a parse error, which is the correct behavior.
                    return Err(GqlError::Execution(ExecutionError::unsupported(format!(
                        "CALL procedure: {}",
                        cc.procedure
                    ))));
                }
            }
        }

        // If we have a RETURN clause, build the return pipeline.
        if let Some(rc) = return_clause {
            // RETURN without MATCH: produce exactly one row with no bindings.
            let child = pipeline.unwrap_or(Operator::SingleRow);
            return self.plan_return(rc, child);
        }

        // If we only have a CREATE/DELETE/SET/etc. (no RETURN), wrap in ProduceResults with no columns.
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

        // Inline WHERE clause on the start node pattern.
        if let Some(pred) = &start.where_clause {
            op = Operator::Filter {
                predicate: pred.clone(),
                child: Box::new(op),
            };
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

            // Inline WHERE clause on the relationship pattern.
            if let Some(pred) = &rel.where_clause {
                op = Operator::Filter {
                    predicate: pred.clone(),
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

            // Inline WHERE clause on the destination node pattern.
            if let Some(pred) = &node.where_clause {
                op = Operator::Filter {
                    predicate: pred.clone(),
                    child: Box::new(op),
                };
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
        let child = child.unwrap_or_else(|| Operator::NodeScan {
            label: None,
            alias: Identifier::new("__empty__"),
        });

        let has_aggregation = wc.items.iter().any(|item| has_agg_function(&item.expr));

        // Build the projection or aggregation pipeline.
        let mut pipeline = if has_aggregation {
            self.plan_with_aggregation(&wc.items, child)?
        } else {
            // Plain projection: wrap in Project operator.
            let items: Vec<(Expr, Option<Identifier>)> = wc
                .items
                .iter()
                .map(|item| (item.expr.clone(), item.alias.clone()))
                .collect();
            Operator::Project {
                items,
                child: Box::new(child),
            }
        };

        // ORDER BY.
        if !wc.order_by.is_empty() {
            pipeline = Operator::Sort {
                items: wc
                    .order_by
                    .iter()
                    .map(|s| (s.expr.clone(), s.ascending))
                    .collect(),
                child: Box::new(pipeline),
            };
        }

        // SKIP.
        if let Some(skip_expr) = &wc.skip {
            pipeline = Operator::Skip {
                count: skip_expr.clone(),
                child: Box::new(pipeline),
            };
        }

        // LIMIT.
        if let Some(limit_expr) = &wc.limit {
            pipeline = Operator::Limit {
                count: limit_expr.clone(),
                child: Box::new(pipeline),
            };
        }

        // DISTINCT.
        if wc.distinct && !has_aggregation {
            pipeline = Operator::Distinct {
                child: Box::new(pipeline),
            };
        }

        // WHERE clause (applied after projection to filter on projected aliases).
        if let Some(pred) = &wc.where_clause {
            pipeline = Operator::Filter {
                predicate: pred.clone(),
                child: Box::new(pipeline),
            };
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
        let mut aggregations: Vec<(AggFunction, Expr, bool, Identifier)> = Vec::new();

        for item in items {
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

    // ── UNWIND planning ───────────────────────────────────────────────────────

    fn plan_unwind(
        &self,
        uc: &UnwindClause,
        child: Option<Operator>,
    ) -> Result<Operator, GqlError> {
        let child = child.unwrap_or_else(|| Operator::NodeScan {
            label: None,
            alias: Identifier::new("__empty__"),
        });

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
        let node_alias = mc.pattern.start.variable.clone();
        let labels = mc.pattern.start.labels.clone();

        let on_create: Vec<SetItem> = mc.on_create.iter().map(map_set_clause_item).collect();
        let on_match: Vec<SetItem> = mc.on_match.iter().map(map_set_clause_item).collect();

        Ok(Operator::Merge {
            node_alias,
            labels,
            on_create,
            on_match,
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

        // ORDER BY -- sort expressions reference raw variable names.
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
    match &item.expr {
        Expr::Variable(id) => id.0.clone(),
        Expr::Property(obj, key) => {
            format!("{}.{}", expr_name(obj), key)
        }
        Expr::FunctionCall { name, args, .. } => {
            let arg_strs: Vec<String> = args.iter().map(expr_name).collect();
            format!("{}({})", name, arg_strs.join(", "))
        }
        other => format!("{:?}", other),
    }
}

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
        Query {
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
        let query = Query {
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
        let query = Query {
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
        let query = Query {
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
        let query = Query {
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
        let query = Query {
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
        let query = Query {
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
        let query = Query {
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
        let query = Query {
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
        let query = Query {
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
        let query = Query {
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
        let query = Query {
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
        let query = Query {
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
        let query = Query {
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
        let query = Query {
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
        let query = Query {
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
        let query = Query {
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
        let query = Query {
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
        let query = Query {
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
        let query = Query {
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
        let query = Query {
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
        let query = Query {
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
        let query = Query {
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
        let query = Query {
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
        let query = Query {
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
        let query = Query {
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
        let query = Query {
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
        let query = Query {
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
            | Operator::VarLenExpand { child, .. } => vec![child],

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
        }
    }
}
