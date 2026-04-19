//! RETURN clause validation, including ORDER BY scope checks specific to
//! `RETURN [DISTINCT]`.

use std::collections::HashSet;

use crate::ast::{Expr, ReturnClause};
use crate::error::GqlError;
use crate::planner::validator::expression::{contains_aggregation, has_ambiguous_aggregation};
use crate::planner::validator::{plan_error_semantic, ValidationContext};

impl ValidationContext {
    // ── RETURN validation ───────────────────────────────────────────────────

    pub(in crate::planner::validator) fn validate_return(
        &self,
        rc: &ReturnClause,
    ) -> Result<(), GqlError> {
        // Check RETURN * with empty scope — `RETURN *` is represented as Variable("*")
        let is_return_star = rc.items.len() == 1
            && matches!(&rc.items[0].expr, Expr::Variable(ref id) if id.0 == "*");
        if is_return_star && self.vars.is_empty() {
            return Err(plan_error_semantic(
                "RETURN * requires at least one variable in scope",
            ));
        }
        // Check for undefined variables in RETURN (only top-level variables, not inside expressions)
        if !is_return_star && self.has_read {
            for item in &rc.items {
                if let Expr::Variable(ref id) = item.expr {
                    if id.0 != "*" && !self.vars.contains_key(&id.0) {
                        return Err(plan_error_semantic(&format!(
                            "variable '{}' not defined",
                            id.0
                        )));
                    }
                }
            }
        }
        self.check_column_name_conflicts(&rc.items)?;
        self.validate_aggregation_consistency(&rc.items)?;
        // Validate RETURN expressions
        for item in &rc.items {
            self.validate_expr_general(&item.expr)?;
            // Check for PatternExpr in RETURN projection (UnexpectedSyntax)
            self.check_no_pattern_expr(&item.expr)?;
            // Check that all variable references are defined (UndefinedVariable)
            if !is_return_star {
                self.check_vars_defined(&item.expr, &std::collections::HashSet::new())?;
            }
        }
        // Validate ORDER BY — check for aggregation in ORDER BY
        let has_agg_in_return = rc.items.iter().any(|item| contains_aggregation(&item.expr));

        // Build the set of names available after RETURN DISTINCT (projected names only)
        let return_projected_names: HashSet<String> = rc
            .items
            .iter()
            .filter_map(|item| {
                if let Some(ref alias) = item.alias {
                    Some(alias.0.clone())
                } else if let Expr::Variable(ref id) = item.expr {
                    Some(id.0.clone())
                } else {
                    None
                }
            })
            .collect();

        for sort_item in &rc.order_by {
            if !has_agg_in_return && contains_aggregation(&sort_item.expr) {
                return Err(plan_error_semantic(
                    "aggregation in ORDER BY requires aggregation in RETURN",
                ));
            }
            // After RETURN DISTINCT, only projected names are in scope for ORDER BY
            if rc.distinct && has_agg_in_return {
                // With aggregation + DISTINCT, strictly check ORDER BY scope
                self.validate_order_by_expr_scope(&sort_item.expr, &return_projected_names)?;
            } else if rc.distinct {
                // DISTINCT without aggregation: ORDER BY must use projected names only
                // (variables not projected are no longer in scope)
                self.validate_order_by_expr_scope(&sort_item.expr, &return_projected_names)?;
            }
            // Check for ambiguous aggregation in ORDER BY when RETURN has aggregation
            if has_agg_in_return && has_ambiguous_aggregation(&sort_item.expr) {
                return Err(plan_error_semantic(
                    "ambiguous aggregation in ORDER BY: expression mixes aggregation and non-aggregation",
                ));
            }
            self.validate_order_by_expr(&sort_item.expr, &rc.items)?;
        }
        if let Some(ref skip) = rc.skip {
            self.validate_skip_limit(skip, "SKIP")?;
        }
        if let Some(ref limit) = rc.limit {
            self.validate_skip_limit(limit, "LIMIT")?;
        }
        Ok(())
    }

    /// Validate that ORDER BY expression only references projected names.
    pub(super) fn validate_order_by_expr_scope(
        &self,
        expr: &Expr,
        projected_names: &HashSet<String>,
    ) -> Result<(), GqlError> {
        match expr {
            Expr::Variable(ref id)
                if id.0 != "*"
                    && !projected_names.contains(&id.0)
                    && !self.vars.contains_key(&id.0) =>
            {
                return Err(plan_error_semantic(&format!(
                    "variable '{}' not in scope after RETURN/WITH DISTINCT",
                    id.0
                )));
            }
            Expr::Property(inner, _) => {
                // For property access like `a.age`, check if `a` itself is projected
                // After DISTINCT, if `a` is not projected but only `a.name` is, then `a.age` is undefined
                if let Expr::Variable(ref id) = inner.as_ref() {
                    // Check if the variable `a` is directly projected OR if any property of `a` is projected
                    if !projected_names.contains(&id.0) && !self.vars.contains_key(&id.0) {
                        return Err(plan_error_semantic(&format!(
                            "variable '{}' not in scope after RETURN/WITH DISTINCT",
                            id.0
                        )));
                    }
                    // Check if the node variable `a` is projected (not just a.name property)
                    // When DISTINCT is used and only a.name was projected (not `a`), then a.age is invalid
                    if !projected_names.contains(&id.0) {
                        // Only the property `a.name` was projected, not `a` itself
                        // `a.age` would reference the original variable which is no longer in scope
                        return Err(plan_error_semantic(&format!(
                            "variable '{}' not projected by DISTINCT",
                            id.0
                        )));
                    }
                }
            }
            Expr::BinaryOp { left, right, .. } => {
                self.validate_order_by_expr_scope(left, projected_names)?;
                self.validate_order_by_expr_scope(right, projected_names)?;
            }
            Expr::UnaryOp { expr, .. } => {
                self.validate_order_by_expr_scope(expr, projected_names)?;
            }
            Expr::FunctionCall { args, .. } => {
                for arg in args {
                    self.validate_order_by_expr_scope(arg, projected_names)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}
