//! Undefined-variable checking for arbitrary expressions.
//!
//! Walks an `Expr` and ensures every variable reference is either bound in
//! the current `ValidationContext` scope or provided via `extra_vars`
//! (e.g. list comprehension / reduce loop variables).

use std::collections::HashSet;

use crate::ast::Expr;
use crate::error::GqlError;

use super::{plan_error_semantic, ValidationContext};

impl ValidationContext {
    /// Check that all variable references in an expression are defined in scope.
    /// `extra_vars` provides additional in-scope variables (e.g. list comprehension loop vars).
    pub(in crate::planner::validator) fn check_vars_defined(
        &self,
        expr: &Expr,
        extra_vars: &HashSet<String>,
    ) -> Result<(), GqlError> {
        match expr {
            Expr::Variable(ref id) => {
                if id.0 != "*" && !self.vars.contains_key(&id.0) && !extra_vars.contains(&id.0) {
                    return Err(plan_error_semantic(&format!(
                        "variable '{}' not defined",
                        id.0
                    )));
                }
                Ok(())
            }
            Expr::Property(inner, _) => self.check_vars_defined(inner, extra_vars),
            Expr::BinaryOp { left, right, .. } => {
                self.check_vars_defined(left, extra_vars)?;
                self.check_vars_defined(right, extra_vars)
            }
            Expr::UnaryOp { expr, .. } => self.check_vars_defined(expr, extra_vars),
            Expr::FunctionCall { name, args, .. } => {
                // Pattern comprehensions introduce their own variable scope — skip var checking
                if name.as_str() == "__patternComp" {
                    return Ok(());
                }
                for arg in args {
                    self.check_vars_defined(arg, extra_vars)?;
                }
                Ok(())
            }
            Expr::IsNull(inner) | Expr::IsNotNull(inner) | Expr::HasLabel(inner, _) => {
                self.check_vars_defined(inner, extra_vars)
            }
            Expr::ListComp {
                variable,
                list,
                filter,
                map,
                ..
            } => {
                self.check_vars_defined(list, extra_vars)?;
                // Add the loop variable to scope for filter and map
                let mut inner_extra = extra_vars.clone();
                inner_extra.insert(variable.0.clone());
                if let Some(f) = filter {
                    self.check_vars_defined(f, &inner_extra)?;
                }
                self.check_vars_defined(map, &inner_extra)
            }
            Expr::Reduce {
                accumulator,
                initial,
                variable,
                list,
                expression,
            } => {
                self.check_vars_defined(initial, extra_vars)?;
                self.check_vars_defined(list, extra_vars)?;
                // Both accumulator and variable are in scope for the expression
                let mut inner_extra = extra_vars.clone();
                inner_extra.insert(accumulator.0.clone());
                inner_extra.insert(variable.0.clone());
                self.check_vars_defined(expression, &inner_extra)
            }
            Expr::List(items) => {
                for item in items {
                    self.check_vars_defined(item, extra_vars)?;
                }
                Ok(())
            }
            Expr::MapLiteral(pairs) => {
                for (_, v) in pairs {
                    self.check_vars_defined(v, extra_vars)?;
                }
                Ok(())
            }
            Expr::Case {
                subject,
                alternatives,
                default,
            } => {
                if let Some(s) = subject {
                    self.check_vars_defined(s, extra_vars)?;
                }
                for (w, t) in alternatives {
                    self.check_vars_defined(w, extra_vars)?;
                    self.check_vars_defined(t, extra_vars)?;
                }
                if let Some(d) = default {
                    self.check_vars_defined(d, extra_vars)?;
                }
                Ok(())
            }
            Expr::Subscript(a, b) => {
                self.check_vars_defined(a, extra_vars)?;
                self.check_vars_defined(b, extra_vars)
            }
            Expr::Slice(a, b, c) => {
                self.check_vars_defined(a, extra_vars)?;
                if let Some(b) = b {
                    self.check_vars_defined(b, extra_vars)?;
                }
                if let Some(c) = c {
                    self.check_vars_defined(c, extra_vars)?;
                }
                Ok(())
            }
            // PatternExpr and ExistsSubquery introduce their own scope — skip
            Expr::PatternExpr(_) | Expr::ExistsSubquery { .. } => Ok(()),
            _ => Ok(()),
        }
    }
}
