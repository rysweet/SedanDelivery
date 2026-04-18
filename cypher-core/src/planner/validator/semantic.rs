//! Clause-level semantic validation for RETURN, WITH, UNION, and ORDER BY.

use std::collections::HashSet;

use crate::ast::{Clause, Expr, ReturnClause, ReturnItem, WithClause};
use crate::error::GqlError;

use super::expression::{
    collect_variable_refs, contains_aggregation, has_ambiguous_aggregation, has_nested_aggregation,
    is_aggregation_function,
};
use super::{plan_error_semantic, ValidationContext, VarKind};

impl ValidationContext {
    // ── RETURN validation ───────────────────────────────────────────────────

    pub(super) fn validate_return(&self, rc: &ReturnClause) -> Result<(), GqlError> {
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
    fn validate_order_by_expr_scope(
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

    // ── WITH validation ─────────────────────────────────────────────────────

    pub(super) fn validate_with(&self, wc: &WithClause) -> Result<(), GqlError> {
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

    pub(super) fn reset_scope_for_with(&mut self, wc: &WithClause) {
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

    // ── UNION validation ───────────────────────────────────────────────────

    pub(super) fn validate_unions(&self, query: &crate::ast::Query) -> Result<(), GqlError> {
        // Check that all UNION parts are either all UNION or all UNION ALL (no mixing)
        if query.unions.len() >= 2 {
            let first_is_all = query.unions[0].0;
            for (i, (is_all, _)) in query.unions.iter().enumerate().skip(1) {
                if *is_all != first_is_all {
                    return Err(plan_error_semantic(
                        "invalid clause composition: cannot mix UNION and UNION ALL",
                    ));
                }
                let _ = i;
            }
        }

        // Check that all UNION parts have the same number and names of RETURN columns
        let first_return = Self::find_return_columns(&query.clauses);
        for (_, part_clauses) in &query.unions {
            let part_return = Self::find_return_columns(part_clauses);
            if let (Some(ref first), Some(ref part)) = (&first_return, &part_return) {
                if first.len() != part.len() {
                    return Err(plan_error_semantic(
                        "all sub queries in a UNION must have the same column names",
                    ));
                }
                for (f, p) in first.iter().zip(part.iter()) {
                    if f != p {
                        return Err(plan_error_semantic(
                            "all sub queries in a UNION must have the same column names",
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn find_return_columns(clauses: &[Clause]) -> Option<Vec<String>> {
        for clause in clauses {
            if let Clause::Return(rc) = clause {
                let names: Vec<String> = rc
                    .items
                    .iter()
                    .map(|item| {
                        if let Some(ref alias) = item.alias {
                            alias.0.clone()
                        } else if let Expr::Variable(ref id) = item.expr {
                            id.0.clone()
                        } else {
                            // Expression without alias
                            String::new()
                        }
                    })
                    .collect();
                return Some(names);
            }
        }
        None
    }

    // ── ORDER BY validation ─────────────────────────────────────────────────

    fn validate_order_by_expr(
        &self,
        expr: &Expr,
        _return_items: &[ReturnItem],
    ) -> Result<(), GqlError> {
        // Validate type errors in ORDER BY expressions
        self.validate_expr_general(expr)
    }

    fn validate_order_by_expr_in_with(
        &self,
        expr: &Expr,
        with_names: &HashSet<String>,
    ) -> Result<(), GqlError> {
        // Variables in ORDER BY must be in the projected WITH columns or current scope
        self.check_expr_scope_in_with(expr, with_names)
    }

    fn check_expr_scope_in_with(
        &self,
        expr: &Expr,
        with_names: &HashSet<String>,
    ) -> Result<(), GqlError> {
        match expr {
            Expr::Variable(ref id)
                if !with_names.contains(&id.0) && !self.vars.contains_key(&id.0) =>
            {
                return Err(plan_error_semantic(&format!(
                    "variable '{}' not defined",
                    id.0
                )));
            }
            Expr::Property(inner, _) => {
                self.check_expr_scope_in_with(inner, with_names)?;
            }
            Expr::BinaryOp { left, right, .. } => {
                self.check_expr_scope_in_with(left, with_names)?;
                self.check_expr_scope_in_with(right, with_names)?;
            }
            Expr::UnaryOp { expr, .. } => {
                self.check_expr_scope_in_with(expr, with_names)?;
            }
            Expr::FunctionCall { args, .. } => {
                for arg in args {
                    self.check_expr_scope_in_with(arg, with_names)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    // ── Column name conflict checking ───────────────────────────────────────

    pub(super) fn check_column_name_conflicts(&self, items: &[ReturnItem]) -> Result<(), GqlError> {
        let mut names: HashSet<String> = HashSet::new();
        for item in items {
            // Only check explicit aliases and bare variable names
            let name = if let Some(ref alias) = item.alias {
                alias.0.clone()
            } else if let Expr::Variable(ref id) = item.expr {
                id.0.clone()
            } else {
                continue;
            };
            if !names.insert(name.clone()) {
                return Err(plan_error_semantic(&format!(
                    "column name '{name}' appears multiple times in RETURN"
                )));
            }
        }
        Ok(())
    }

    pub(super) fn check_with_column_conflicts(&self, items: &[ReturnItem]) -> Result<(), GqlError> {
        let mut names: HashSet<String> = HashSet::new();
        for item in items {
            let name = if let Some(ref alias) = item.alias {
                alias.0.clone()
            } else if let Expr::Variable(ref id) = item.expr {
                id.0.clone()
            } else {
                continue;
            };
            if !names.insert(name.clone()) {
                return Err(plan_error_semantic(&format!(
                    "column name '{name}' appears multiple times in WITH"
                )));
            }
        }
        Ok(())
    }

    // ── Aggregation validation ──────────────────────────────────────────────

    pub(super) fn validate_aggregation_consistency(
        &self,
        items: &[ReturnItem],
    ) -> Result<(), GqlError> {
        let has_agg = items.iter().any(|item| contains_aggregation(&item.expr));
        if !has_agg {
            return Ok(());
        }
        for item in items {
            if has_nested_aggregation(&item.expr, false) {
                return Err(plan_error_semantic("nested aggregation is not allowed"));
            }
            // Check for ambiguous aggregation: expression that contains both
            // aggregation functions AND non-simple references at the same level.
            // e.g. `me.age + you.age + count(*)` where the addition mixes
            // aggregated and non-aggregated parts.
            if has_ambiguous_aggregation(&item.expr) {
                return Err(plan_error_semantic(
                    "ambiguous aggregation expression: contains both aggregation and non-aggregation",
                ));
            }
        }
        Ok(())
    }
}
