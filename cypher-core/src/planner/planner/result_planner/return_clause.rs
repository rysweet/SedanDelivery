//! RETURN clause planning.

use crate::ast::{Expr, Identifier, ReturnClause};
use crate::error::GqlError;
use crate::planner::Operator;

use super::super::helpers::{
    expand_star_items, expr_to_col_name, has_agg_function, return_item_name, rewrite_agg_order_expr,
};
use super::super::QueryPlanner;

impl<'s> QueryPlanner<'s> {
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
}
