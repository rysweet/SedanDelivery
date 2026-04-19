//! WITH clause validation, scope reset, and the strict ORDER BY scope check
//! used when WITH contains aggregation.

use std::collections::HashSet;

use crate::ast::{Expr, WithClause};
use crate::error::GqlError;
use crate::planner::validator::expression::{
    collect_variable_refs, contains_aggregation, has_ambiguous_aggregation, is_aggregation_function,
};
use crate::planner::validator::{plan_error_semantic, ValidationContext, VarKind};

impl ValidationContext {
    pub(in crate::planner::validator) fn validate_with(
        &self,
        wc: &WithClause,
    ) -> Result<(), GqlError> {
        // WITH requires all non-variable expressions to have aliases
        for item in &wc.items {
            if item.alias.is_none() && !matches!(item.expr, Expr::Variable(_)) {
                // Expression without alias in WITH — unless it's `*`
                if matches!(&item.expr, Expr::Variable(ref id) if id.0 == "*") {
                    continue;
                }
                return Err(plan_error_semantic(
                    "expression in WITH must be aliased (use AS)",
                ));
            }
        }
        self.check_with_column_conflicts(&wc.items)?;
        self.validate_aggregation_consistency(&wc.items)?;
        for item in &wc.items {
            self.validate_expr_general(&item.expr)?;
            // Check for PatternExpr in WITH projection (UnexpectedSyntax)
            self.check_no_pattern_expr(&item.expr)?;
        }
        let with_names: HashSet<String> = wc
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

        // Check for aggregation in ORDER BY when WITH projection has no aggregation
        let has_agg_in_projection = wc.items.iter().any(|item| contains_aggregation(&item.expr));

        // Collect variables used in the non-aggregated (grouping key) WITH items.
        // These variables can be referenced in ORDER BY even after aggregation.
        let mut grouping_key_vars: HashSet<String> = HashSet::new();
        if has_agg_in_projection {
            for item in &wc.items {
                if !contains_aggregation(&item.expr) {
                    collect_variable_refs(&item.expr, &mut grouping_key_vars);
                }
            }
        }

        for sort_item in &wc.order_by {
            if !has_agg_in_projection && contains_aggregation(&sort_item.expr) {
                return Err(plan_error_semantic(
                    "aggregation in ORDER BY requires aggregation in WITH projection",
                ));
            }
            // Check for ambiguous aggregation in ORDER BY when WITH has aggregation
            if has_agg_in_projection && has_ambiguous_aggregation(&sort_item.expr) {
                return Err(plan_error_semantic(
                    "ambiguous aggregation in ORDER BY: expression mixes aggregation and non-aggregation",
                ));
            }
            if has_agg_in_projection {
                // When aggregation is present, ORDER BY can only reference:
                // 1. Projected names (with_names)
                // 2. Variables used in the grouping key expressions
                // (not arbitrary variables from the previous scope)
                self.validate_order_by_expr_strict_with(
                    &sort_item.expr,
                    &with_names,
                    &grouping_key_vars,
                )?;
            } else {
                self.validate_order_by_expr_in_with(&sort_item.expr, &with_names)?;
            }
        }
        // Note: WHERE in WITH is validated loosely — the executor will catch
        // truly undefined variables. We skip strict scope checking here to avoid
        // rejecting valid queries that use complex expressions.
        if let Some(ref pred) = wc.where_clause {
            self.validate_expr_general(pred)?;
        }
        if let Some(ref skip) = wc.skip {
            self.validate_skip_limit(skip, "SKIP")?;
        }
        if let Some(ref limit) = wc.limit {
            self.validate_skip_limit(limit, "LIMIT")?;
        }
        Ok(())
    }

    /// Strict ORDER BY scope check for WITH with aggregation.
    /// Variables outside aggregation calls must be from with_names or grouping_key_vars.
    /// Variables INSIDE aggregation calls are allowed (they reference pre-aggregation data).
    fn validate_order_by_expr_strict_with(
        &self,
        expr: &Expr,
        with_names: &HashSet<String>,
        grouping_key_vars: &HashSet<String>,
    ) -> Result<(), GqlError> {
        self.validate_order_by_expr_strict_with_inner(expr, with_names, grouping_key_vars, false)
    }

    fn validate_order_by_expr_strict_with_inner(
        &self,
        expr: &Expr,
        with_names: &HashSet<String>,
        grouping_key_vars: &HashSet<String>,
        inside_aggregation: bool,
    ) -> Result<(), GqlError> {
        match expr {
            Expr::Variable(ref id)
                // Variables inside aggregation calls are allowed (pre-agg data access)
                if !inside_aggregation
                    && id.0 != "*"
                    && !with_names.contains(&id.0)
                    && !grouping_key_vars.contains(&id.0)
                => {
                    return Err(plan_error_semantic(&format!(
                        "variable '{}' not in scope after WITH projection",
                        id.0
                    )));
                }
            Expr::FunctionCall { name, args, .. } => {
                let next_inside_agg = inside_aggregation || is_aggregation_function(name.as_str());
                for arg in args {
                    self.validate_order_by_expr_strict_with_inner(
                        arg,
                        with_names,
                        grouping_key_vars,
                        next_inside_agg,
                    )?;
                }
            }
            Expr::Property(inner, _) => {
                self.validate_order_by_expr_strict_with_inner(
                    inner,
                    with_names,
                    grouping_key_vars,
                    inside_aggregation,
                )?;
            }
            Expr::BinaryOp { left, right, .. } => {
                self.validate_order_by_expr_strict_with_inner(
                    left,
                    with_names,
                    grouping_key_vars,
                    inside_aggregation,
                )?;
                self.validate_order_by_expr_strict_with_inner(
                    right,
                    with_names,
                    grouping_key_vars,
                    inside_aggregation,
                )?;
            }
            Expr::UnaryOp { expr, .. } => {
                self.validate_order_by_expr_strict_with_inner(
                    expr,
                    with_names,
                    grouping_key_vars,
                    inside_aggregation,
                )?;
            }
            _ => {}
        }
        Ok(())
    }

    pub(in crate::planner::validator) fn reset_scope_for_with(&mut self, wc: &WithClause) {
        let mut new_vars = std::collections::HashMap::new();
        for item in &wc.items {
            if let Some(ref alias) = item.alias {
                // Preserve the kind if the expression is a direct variable reference.
                // For literals (known scalars), use Value.
                // For function calls and other complex expressions use Untyped so
                // a subsequent MATCH can treat the variable as a node if needed
                // (e.g. `WITH coalesce(a, b) AS x MATCH (x)-->(d)`).
                let kind = match &item.expr {
                    Expr::Variable(ref id) => {
                        self.vars.get(&id.0).copied().unwrap_or(VarKind::Value)
                    }
                    // Known non-node scalar literals.
                    Expr::Integer(_) | Expr::Float(_) | Expr::Str(_) | Expr::Bool(_) => {
                        VarKind::Value
                    }
                    // List and map literals are also non-node values.
                    // e.g. `WITH [] AS n MATCH (n)` or `WITH {x:1} AS n MATCH (n)` must fail.
                    Expr::List(_) | Expr::MapLiteral(_) => VarKind::Value,
                    // Null and all other expressions (function calls, properties, etc.)
                    // are Untyped — their runtime type is not known at compile time.
                    // This allows e.g. `WITH null AS a OPTIONAL MATCH (a)-->()`.
                    _ => VarKind::Untyped,
                };
                new_vars.insert(alias.0.clone(), kind);
            } else if let Expr::Variable(ref id) = item.expr {
                let kind = self.vars.get(&id.0).copied().unwrap_or(VarKind::Value);
                new_vars.insert(id.0.clone(), kind);
            }
        }
        self.vars = new_vars;
        self.has_read = false;
        self.has_write = false;
    }
}
