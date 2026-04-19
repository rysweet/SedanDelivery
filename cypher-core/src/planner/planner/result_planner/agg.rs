//! Aggregation extraction shared by RETURN/WITH planners.

use crate::ast::{Expr, Identifier, ReturnItem};
use crate::error::GqlError;
use crate::planner::{AggFunction, Operator};

use super::super::helpers::{
    expr_to_col_name, extract_agg_function, find_inner_aggregate, has_agg_function,
    return_item_name, rewrite_agg_to_var,
};
use super::super::QueryPlanner;

impl<'s> QueryPlanner<'s> {
    pub(super) fn plan_aggregation_from_items(
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
