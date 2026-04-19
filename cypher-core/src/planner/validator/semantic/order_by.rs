//! Generic ORDER BY validation: type checks and scope checks for ORDER BY in
//! WITH (without aggregation).

use std::collections::HashSet;

use crate::ast::{Expr, ReturnItem};
use crate::error::GqlError;
use crate::planner::validator::{plan_error_semantic, ValidationContext};

impl ValidationContext {
    pub(super) fn validate_order_by_expr(
        &self,
        expr: &Expr,
        _return_items: &[ReturnItem],
    ) -> Result<(), GqlError> {
        // Validate type errors in ORDER BY expressions
        self.validate_expr_general(expr)
    }

    pub(super) fn validate_order_by_expr_in_with(
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
}
