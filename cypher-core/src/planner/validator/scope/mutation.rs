//! DELETE, UNWIND, SET, and REMOVE clause validation.

use crate::ast::{DeleteClause, Expr, RemoveClause, SetClause, UnwindClause};
use crate::error::GqlError;

use super::super::expression::is_non_variable_expr;
use super::super::{plan_error_semantic, ValidationContext, VarKind};

impl ValidationContext {
    // ── DELETE validation ───────────────────────────────────────────────────

    pub(in crate::planner::validator) fn validate_delete(
        &self,
        dc: &DeleteClause,
    ) -> Result<(), GqlError> {
        for expr in &dc.exprs {
            self.validate_delete_expr(expr)?;
        }
        Ok(())
    }

    fn validate_delete_expr(&self, expr: &Expr) -> Result<(), GqlError> {
        match expr {
            Expr::Variable(ref id) => {
                if !self.vars.contains_key(&id.0) {
                    return Err(plan_error_semantic(&format!(
                        "variable '{}' not defined",
                        id.0
                    )));
                }
                Ok(())
            }
            // DELETE n.prop, DELETE map.key
            Expr::Property(inner, _) => self.validate_delete_expr(inner),
            // DELETE list[idx]
            Expr::Subscript(inner, _) => self.validate_delete_expr(inner),
            // DELETE n:Label is invalid — label predicates are not valid in DELETE
            Expr::HasLabel(_, _) => Err(plan_error_semantic(
                "invalid use of a label predicate in DELETE; use REMOVE to remove labels",
            )),
            // Literal values in DELETE are invalid: DELETE 1+1
            Expr::Integer(_) | Expr::Float(_) | Expr::Str(_) | Expr::Bool(_) | Expr::Null => Err(
                plan_error_semantic("DELETE requires a variable reference, got a literal"),
            ),
            // Arithmetic expressions on literals: DELETE 1+1
            Expr::BinaryOp { left, right, .. } => {
                if is_non_variable_expr(left) && is_non_variable_expr(right) {
                    return Err(plan_error_semantic(
                        "DELETE requires a variable reference, got an expression",
                    ));
                }
                Ok(())
            }
            // Other expressions (function calls, etc.) — allow through for runtime checking
            _ => Ok(()),
        }
    }

    // ── UNWIND validation ───────────────────────────────────────────────────

    pub(in crate::planner::validator) fn validate_unwind(
        &mut self,
        uc: &UnwindClause,
    ) -> Result<(), GqlError> {
        self.validate_expr_general(&uc.expr)?;
        // UNWIND can produce any type (nodes, relationships, scalars),
        // so use Untyped to avoid false conflicts in subsequent MATCH patterns
        self.vars.insert(uc.alias.0.clone(), VarKind::Untyped);
        Ok(())
    }

    // ── SET / REMOVE validation ─────────────────────────────────────────────

    pub(in crate::planner::validator) fn validate_set(
        &self,
        sc: &SetClause,
    ) -> Result<(), GqlError> {
        let empty_set = std::collections::HashSet::new();
        for item in &sc.items {
            match item {
                crate::ast::SetClauseItem::SetProperty { alias, value, .. } => {
                    if !self.vars.contains_key(&alias.0) {
                        return Err(plan_error_semantic(&format!(
                            "variable '{}' not defined",
                            alias.0
                        )));
                    }
                    // Check for PatternExpr in SET values (UnexpectedSyntax)
                    self.check_no_pattern_expr(value)?;
                    self.check_vars_defined(value, &empty_set)?;
                }
                crate::ast::SetClauseItem::SetMap { alias, map, .. } => {
                    if !self.vars.contains_key(&alias.0) {
                        return Err(plan_error_semantic(&format!(
                            "variable '{}' not defined",
                            alias.0
                        )));
                    }
                    self.check_no_pattern_expr(map)?;
                    self.check_vars_defined(map, &empty_set)?;
                }
                crate::ast::SetClauseItem::SetLabels { alias, .. } => {
                    if !self.vars.contains_key(&alias.0) {
                        return Err(plan_error_semantic(&format!(
                            "variable '{}' not defined",
                            alias.0
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    pub(in crate::planner::validator) fn validate_remove(
        &self,
        _rc: &RemoveClause,
    ) -> Result<(), GqlError> {
        Ok(())
    }
}
