//! WITH clause planning (incl. aggregation extraction).

use crate::ast::{Expr, Identifier, ReturnItem, WithClause};
use crate::error::GqlError;
use crate::planner::{AggFunction, Operator};

use super::super::helpers::{
    expr_to_col_name, extract_agg_function, find_inner_aggregate, has_agg_function,
    return_item_name, rewrite_agg_to_var, rewrite_with_order_expr,
};
use super::super::QueryPlanner;

impl<'s> QueryPlanner<'s> {
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
}
