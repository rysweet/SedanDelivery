//! Result planning — RETURN, WITH, UNWIND, CALL, aggregation extraction, ORDER BY,
//! DISTINCT/SKIP/LIMIT.

use crate::ast::{
    CallClause, Clause, Expr, Identifier, Query, ReturnClause, ReturnItem, UnwindClause, WithClause,
};
use crate::error::{ExecutionError, GqlError, PlanError};
use crate::planner::{AggFunction, Operator};

use super::helpers::{
    contains_aggregation, expand_star_items, expr_to_col_name, expr_to_literal_value,
    extract_agg_function, find_inner_aggregate, has_agg_function, return_item_name,
    rewrite_agg_order_expr, rewrite_agg_to_var, rewrite_with_order_expr,
};
use super::QueryPlanner;

impl<'s> QueryPlanner<'s> {
    // ── CALL planning ─────────────────────────────────────────────────────────

    pub(crate) fn plan_call(
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

        // Check if this is a built-in procedure handled dynamically by the executor.
        let is_builtin = crate::procedure::is_builtin_procedure(&cc.procedure);

        // Look up the procedure (skip registry lookup for built-ins).
        let proc_def;
        let builtin_def;
        let proc_ref: Option<&crate::procedure::ProcedureDefinition> = if is_builtin {
            // Built-in procedures have no input params; create a temporary definition
            // for validation purposes only. The executor handles them dynamically.
            let output_columns =
                crate::procedure::builtin_output_columns(&cc.procedure).unwrap_or_default();
            builtin_def = crate::procedure::ProcedureDefinition {
                name: cc.procedure.clone(),
                input_params: vec![],
                output_columns,
                data: vec![],
            };
            Some(&builtin_def)
        } else {
            match self.registry.get(&cc.procedure) {
                Some(p) => {
                    proc_def = p;
                    Some(proc_def)
                }
                None => {
                    return Err(GqlError::Execution(ExecutionError::unsupported(format!(
                        "ProcedureNotFound: {}",
                        cc.procedure
                    ))));
                }
            }
        };
        let proc_ref = proc_ref.unwrap();

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
            if cc.args.len() != proc_ref.input_params.len() {
                return Err(GqlError::Execution(ExecutionError::unsupported(format!(
                    "InvalidNumberOfArguments: expected {}, got {}",
                    proc_ref.input_params.len(),
                    cc.args.len()
                ))));
            }
            // Basic type checking for literal arguments.
            for (i, arg) in cc.args.iter().enumerate() {
                if let Some(val) = expr_to_literal_value(arg) {
                    if !proc_ref.check_arg_type(i, &val) {
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
            for param in &proc_ref.input_params {
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
        // Check if subsequent RETURN references procedure output columns -> UndefinedVariable.
        if is_in_query && cc.yield_items.is_empty() && !cc.yield_star && cc.explicit_args {
            let output_names: std::collections::HashSet<&str> = proc_ref
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
    pub(crate) fn derive_standalone_call_columns(&self, op: &Operator) -> Vec<(String, Expr)> {
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
                    // Standalone or YIELD * -> output all columns from registry.
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
                } else if let Some(cols) = crate::procedure::builtin_output_columns(procedure) {
                    // Built-in procedure: derive columns from static metadata.
                    cols.iter()
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

    // ── WITH planning ─────────────────────────────────────────────────────────

    /// Plan a WITH clause. WITH is semantically identical to RETURN but feeds
    /// the next clause instead of producing final output.
    ///
    /// WITH a, b WHERE a.age > 25  ->  Project then Filter
    /// WITH a, count(b) AS cnt     ->  Aggregate
    /// WITH a ORDER BY a.name SKIP 1 LIMIT 5  ->  Project + Sort + Skip + Limit
    pub(crate) fn plan_with(
        &self,
        wc: &WithClause,
        child: Option<Operator>,
    ) -> Result<Operator, GqlError> {
        let child = child.unwrap_or(Operator::SingleRow);

        // Detect WITH * -- if so, skip the projection entirely (passthrough).
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
            // WITH * is a passthrough -- no projection needed.
            child
        } else if has_aggregation {
            self.plan_with_aggregation(&wc.items, child)?
        } else {
            // For plain projection, Sort/Skip/Limit run on raw rows first.
            // However, if ORDER BY references projected aliases, we need to augment
            // the rows with those aliases before sorting.
            let mut raw = child;

            // Determine if ORDER BY references any projected aliases (at any expression depth).
            fn with_expr_refs_alias(expr: &Expr, alias: &str) -> bool {
                match expr {
                    Expr::Variable(v) => v.0.as_str() == alias,
                    Expr::Property(obj, _) => with_expr_refs_alias(obj, alias),
                    Expr::BinaryOp { left, right, .. } => {
                        with_expr_refs_alias(left, alias) || with_expr_refs_alias(right, alias)
                    }
                    Expr::UnaryOp { expr: inner, .. } => with_expr_refs_alias(inner, alias),
                    Expr::FunctionCall { args, .. } => {
                        args.iter().any(|a| with_expr_refs_alias(a, alias))
                    }
                    _ => false,
                }
            }
            let needs_augment_for_sort = projected_items.iter().any(|(_, alias)| {
                if let Some(a) = alias {
                    wc.order_by
                        .iter()
                        .any(|s| with_expr_refs_alias(&s.expr, a.0.as_str()))
                } else {
                    false
                }
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
                    gremlin_order: false,
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
            if let Some(predicate) = wc.where_clause.clone() {
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

                // Filter (WHERE) -- can see both raw vars and projected aliases.
                raw = Operator::Filter {
                    predicate,
                    child: Box::new(raw),
                };

                // Final project: restrict to only projected aliases.
                Operator::Project {
                    items: projected_items.clone(),
                    child: Box::new(raw),
                }
            } else {
                // No WHERE -- just project after Sort/Skip/Limit.
                Operator::Project {
                    items: projected_items.clone(),
                    child: Box::new(raw),
                }
            }
        };

        // ORDER BY (for aggregation/star -- after projection).
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
                gremlin_order: false,
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
                by_key: None,
                productive: false,
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
                        items_out.push((Expr::Variable(agg_alias.clone()), item.alias.clone()));
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

    pub(crate) fn plan_unwind(
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

    // ── RETURN planning ───────────────────────────────────────────────────────

    pub(crate) fn plan_return(
        &self,
        rc: &ReturnClause,
        child: Operator,
    ) -> Result<Operator, GqlError> {
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
                // Collect all RETURN aliases referenced in ORDER BY (at any depth in expression).
                // This handles cases like `ORDER BY n + 2` where `n` is a RETURN alias.
                fn expr_references_alias(expr: &Expr, alias: &str) -> bool {
                    match expr {
                        Expr::Variable(v) => v.0.as_str() == alias,
                        Expr::Property(obj, _) => expr_references_alias(obj, alias),
                        Expr::BinaryOp { left, right, .. } => {
                            expr_references_alias(left, alias)
                                || expr_references_alias(right, alias)
                        }
                        Expr::UnaryOp { expr: inner, .. } => expr_references_alias(inner, alias),
                        Expr::FunctionCall { args, .. } => {
                            args.iter().any(|a| expr_references_alias(a, alias))
                        }
                        _ => false,
                    }
                }
                let needs_pre_project = rc.order_by.iter().any(|s| {
                    items.iter().any(|item| {
                        if let Some(alias) = &item.alias {
                            let alias_str = alias.0.as_str();
                            let orig_name = expr_to_col_name(&item.expr);
                            alias_str != orig_name.as_str()
                                && expr_references_alias(&s.expr, alias_str)
                        } else {
                            false
                        }
                    })
                });
                if needs_pre_project {
                    // Add a Project that exposes RETURN aliases for ORDER BY use.
                    // When an alias shadows an existing variable (e.g. `n.num AS n`),
                    // use a temp name to avoid overwriting the raw variable, then
                    // rewrite ORDER BY expressions to reference the temp name.
                    let child_aliases = crate::planner::collect_visible_aliases(&pipeline);
                    let mut aug_items: Vec<(Expr, Option<Identifier>)> = child_aliases
                        .iter()
                        .map(|a| (Expr::Variable(Identifier::new(a.clone())), None))
                        .collect();
                    // Map from alias name -> temp name (when shadowing occurs).
                    let mut alias_remap: std::collections::HashMap<String, String> =
                        std::collections::HashMap::new();
                    for item in &items {
                        if let Some(alias) = &item.alias {
                            if alias.0 != expr_to_col_name(&item.expr) {
                                let use_alias =
                                    if child_aliases.iter().any(|a| a.as_str() == alias.0.as_str())
                                    {
                                        // Alias shadows an existing variable: use a temp name.
                                        let temp = format!("__presort_{}__", alias.0);
                                        alias_remap.insert(alias.0.to_string(), temp.clone());
                                        Identifier::new(temp)
                                    } else {
                                        alias.clone()
                                    };
                                aug_items.push((item.expr.clone(), Some(use_alias)));
                            }
                        }
                    }
                    (
                        Operator::Project {
                            items: aug_items,
                            child: Box::new(pipeline),
                        },
                        alias_remap,
                    )
                } else {
                    (pipeline, std::collections::HashMap::new())
                }
            } else {
                (pipeline, std::collections::HashMap::new())
            };
            // Rewrite ORDER BY expressions to use temp names when aliases were remapped.
            fn rewrite_alias_refs(
                expr: Expr,
                remap: &std::collections::HashMap<String, String>,
            ) -> Expr {
                if remap.is_empty() {
                    return expr;
                }
                match expr {
                    Expr::Variable(v) => {
                        if let Some(temp) = remap.get(v.0.as_str()) {
                            Expr::Variable(Identifier::new(temp.clone()))
                        } else {
                            Expr::Variable(v)
                        }
                    }
                    Expr::BinaryOp { op, left, right } => Expr::BinaryOp {
                        op,
                        left: Box::new(rewrite_alias_refs(*left, remap)),
                        right: Box::new(rewrite_alias_refs(*right, remap)),
                    },
                    Expr::UnaryOp { op, expr: inner } => Expr::UnaryOp {
                        op,
                        expr: Box::new(rewrite_alias_refs(*inner, remap)),
                    },
                    Expr::FunctionCall {
                        name,
                        args,
                        distinct,
                    } => Expr::FunctionCall {
                        name,
                        args: args
                            .into_iter()
                            .map(|a| rewrite_alias_refs(a, remap))
                            .collect(),
                        distinct,
                    },
                    other => other,
                }
            }
            let (pipeline, alias_remap_final) = pipeline;
            let sort_items: Vec<(Expr, bool)> = rc
                .order_by
                .iter()
                .map(|s| {
                    if has_aggregation {
                        (rewrite_agg_order_expr(&s.expr, &items), s.ascending)
                    } else {
                        (
                            rewrite_alias_refs(s.expr.clone(), &alias_remap_final),
                            s.ascending,
                        )
                    }
                })
                .collect();
            Operator::Sort {
                items: sort_items,
                gremlin_order: false,
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
                by_key: None,
                productive: false,
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
                            aggregations.push((
                                func,
                                arg,
                                second_arg,
                                distinct,
                                temp_alias.clone(),
                            ));
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
                    post_projections.push((Expr::Variable(Identifier::new(gen_name)), Some(alias)));
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
