//! WHERE expression validation: scope checking and pattern variable resolution.

use crate::ast::{Expr, PathPattern};
use crate::error::GqlError;

use super::super::expression::contains_aggregation;
use super::super::{kind_name, plan_error_semantic, ValidationContext, VarKind};

impl ValidationContext {
    pub(in crate::planner::validator) fn validate_where_expr(
        &self,
        expr: &Expr,
    ) -> Result<(), GqlError> {
        // Aggregation functions are not allowed in WHERE clauses (InvalidAggregation)
        if contains_aggregation(expr) {
            return Err(plan_error_semantic(
                "aggregation functions are not allowed in WHERE clauses",
            ));
        }
        // A standalone node/relationship variable used as a boolean predicate is
        // InvalidArgumentType — e.g. `WHERE n` where n is a node
        if let Expr::Variable(ref id) = expr {
            if let Some(&kind) = self.vars.get(&id.0) {
                if kind == VarKind::Node || kind == VarKind::Relationship {
                    return Err(plan_error_semantic(&format!(
                        "variable '{}' is a {} and cannot be used as a boolean predicate",
                        id.0,
                        kind_name(kind)
                    )));
                }
            }
        }
        // Check PatternExpr in WHERE: named variables must be in scope
        self.check_where_pattern_vars(expr)?;
        // Check that all variable references are defined
        self.check_vars_defined_in_where(expr)?;
        self.validate_expr_general(expr)
    }

    /// Check that all variable references in a WHERE expression are defined in scope.
    /// PatternExpr variables are checked separately by check_where_pattern_vars.
    pub(in crate::planner::validator) fn check_vars_defined_in_where(
        &self,
        expr: &Expr,
    ) -> Result<(), GqlError> {
        match expr {
            // PatternExpr in WHERE — variable scope is checked by check_where_pattern_vars
            Expr::PatternExpr(_) | Expr::ExistsSubquery { .. } => Ok(()),
            Expr::Variable(ref id) => {
                if id.0 != "*" && !self.vars.contains_key(&id.0) {
                    return Err(plan_error_semantic(&format!(
                        "variable '{}' not defined",
                        id.0
                    )));
                }
                Ok(())
            }
            Expr::Property(inner, _) => {
                // Property access on a Path variable is invalid (InvalidArgumentType)
                if let Expr::Variable(ref id) = inner.as_ref() {
                    if let Some(&kind) = self.vars.get(&id.0) {
                        if kind == VarKind::Path {
                            return Err(plan_error_semantic(&format!(
                                "property access on path variable '{}' is not allowed",
                                id.0
                            )));
                        }
                    }
                }
                self.check_vars_defined_in_where(inner)
            }
            Expr::BinaryOp { left, right, .. } => {
                self.check_vars_defined_in_where(left)?;
                self.check_vars_defined_in_where(right)
            }
            Expr::UnaryOp { expr, .. } => self.check_vars_defined_in_where(expr),
            Expr::FunctionCall { name, args, .. } => {
                // Pattern comprehensions introduce their own variable scope — skip var checking
                if name.as_str() == "__patternComp" {
                    return Ok(());
                }
                for arg in args {
                    self.check_vars_defined_in_where(arg)?;
                }
                Ok(())
            }
            Expr::IsNull(inner) | Expr::IsNotNull(inner) | Expr::HasLabel(inner, _) => {
                self.check_vars_defined_in_where(inner)
            }
            Expr::ListComp {
                variable,
                list,
                filter,
                map,
                ..
            } => {
                self.check_vars_defined_in_where(list)?;
                // loop variable is valid inside filter and map
                let mut inner_vars = self.vars.clone();
                inner_vars.insert(variable.0.clone(), VarKind::Value);
                let inner_ctx = ValidationContext {
                    vars: inner_vars,
                    has_read: self.has_read,
                    has_write: self.has_write,
                    return_count: self.return_count,
                };
                if let Some(f) = filter {
                    inner_ctx.check_vars_defined_in_where(f)?;
                }
                inner_ctx.check_vars_defined_in_where(map)
            }
            Expr::Reduce {
                accumulator,
                initial,
                variable,
                list,
                expression,
            } => {
                self.check_vars_defined_in_where(initial)?;
                self.check_vars_defined_in_where(list)?;
                // Both accumulator and variable are in scope for the expression
                let mut inner_vars = self.vars.clone();
                inner_vars.insert(accumulator.0.clone(), VarKind::Value);
                inner_vars.insert(variable.0.clone(), VarKind::Value);
                let inner_ctx = ValidationContext {
                    vars: inner_vars,
                    has_read: self.has_read,
                    has_write: self.has_write,
                    return_count: self.return_count,
                };
                inner_ctx.check_vars_defined_in_where(expression)
            }
            Expr::List(items) => {
                for item in items {
                    self.check_vars_defined_in_where(item)?;
                }
                Ok(())
            }
            Expr::MapLiteral(pairs) => {
                for (_, v) in pairs {
                    self.check_vars_defined_in_where(v)?;
                }
                Ok(())
            }
            Expr::Case {
                subject,
                alternatives,
                default,
            } => {
                if let Some(s) = subject {
                    self.check_vars_defined_in_where(s)?;
                }
                for (w, t) in alternatives {
                    self.check_vars_defined_in_where(w)?;
                    self.check_vars_defined_in_where(t)?;
                }
                if let Some(d) = default {
                    self.check_vars_defined_in_where(d)?;
                }
                Ok(())
            }
            Expr::Subscript(a, b) => {
                self.check_vars_defined_in_where(a)?;
                self.check_vars_defined_in_where(b)
            }
            Expr::Slice(a, b, c) => {
                self.check_vars_defined_in_where(a)?;
                if let Some(b) = b {
                    self.check_vars_defined_in_where(b)?;
                }
                if let Some(c) = c {
                    self.check_vars_defined_in_where(c)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Validate PatternExpr in WHERE: all named variables in the pattern must be in scope.
    /// Also detects standalone node patterns (no relationships) as InvalidArgumentType.
    fn check_where_pattern_vars(&self, expr: &Expr) -> Result<(), GqlError> {
        match expr {
            Expr::PatternExpr(patterns) => {
                for pattern in patterns {
                    // Standalone node-only pattern (no relationships) is InvalidArgumentType
                    if pattern.steps.is_empty() {
                        if let Some(ref var) = pattern.start.variable {
                            // (n) with no relationships — checks if node exists but doesn't test a predicate
                            if self.vars.contains_key(&var.0) {
                                return Err(plan_error_semantic(
                                    "standalone node pattern in WHERE is not a valid predicate",
                                ));
                            }
                        }
                    }
                    // All named variables in the pattern must be in scope
                    self.check_pattern_vars_in_scope(pattern)?;
                }
                Ok(())
            }
            Expr::BinaryOp { left, right, .. } => {
                self.check_where_pattern_vars(left)?;
                self.check_where_pattern_vars(right)
            }
            Expr::UnaryOp { expr, .. } => self.check_where_pattern_vars(expr),
            Expr::IsNull(inner) | Expr::IsNotNull(inner) | Expr::HasLabel(inner, _) => {
                self.check_where_pattern_vars(inner)
            }
            _ => Ok(()),
        }
    }

    /// Check that all named variables in a path pattern are already in scope.
    fn check_pattern_vars_in_scope(&self, pattern: &PathPattern) -> Result<(), GqlError> {
        if let Some(ref var) = pattern.start.variable {
            if !var.0.is_empty() && !self.vars.contains_key(&var.0) {
                return Err(plan_error_semantic(&format!(
                    "variable '{}' not defined",
                    var.0
                )));
            }
        }
        for step in &pattern.steps {
            if let Some(ref var) = step.rel.variable {
                if !var.0.is_empty() && !self.vars.contains_key(&var.0) {
                    return Err(plan_error_semantic(&format!(
                        "variable '{}' not defined",
                        var.0
                    )));
                }
            }
            if let Some(ref var) = step.node.variable {
                if !var.0.is_empty() && !self.vars.contains_key(&var.0) {
                    return Err(plan_error_semantic(&format!(
                        "variable '{}' not defined",
                        var.0
                    )));
                }
            }
        }
        Ok(())
    }
}
