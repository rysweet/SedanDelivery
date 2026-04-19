//! `UnexpectedSyntax` placement check for `PatternExpr` inside RETURN/WITH
//! projections — only the source argument of `__patternComp` is allowed.

use crate::ast::Expr;
use crate::error::GqlError;

use super::{plan_error_semantic, ValidationContext};

impl ValidationContext {
    /// Check that no PatternExpr appears in a non-WHERE context (UnexpectedSyntax).
    /// PatternExpr is allowed as the source of a `__patterncomp` (pattern comprehension).
    pub(in crate::planner::validator) fn check_no_pattern_expr(
        &self,
        expr: &Expr,
    ) -> Result<(), GqlError> {
        match expr {
            Expr::PatternExpr(_) => Err(plan_error_semantic(
                "pattern expressions are not allowed in RETURN or WITH projections",
            )),
            Expr::Property(inner, _) => self.check_no_pattern_expr(inner),
            Expr::BinaryOp { left, right, .. } => {
                self.check_no_pattern_expr(left)?;
                self.check_no_pattern_expr(right)
            }
            Expr::UnaryOp { expr, .. } => self.check_no_pattern_expr(expr),
            Expr::FunctionCall { name, args, .. } => {
                // __patternComp is a pattern comprehension — its first arg is a PatternExpr
                // and that's valid; only check the remaining args (the projection expression)
                if name.as_str() == "__patternComp" {
                    // Skip the first arg (PatternExpr source), check the rest
                    for arg in args.iter().skip(1) {
                        self.check_no_pattern_expr(arg)?;
                    }
                } else {
                    for arg in args {
                        self.check_no_pattern_expr(arg)?;
                    }
                }
                Ok(())
            }
            Expr::IsNull(inner) | Expr::IsNotNull(inner) | Expr::HasLabel(inner, _) => {
                self.check_no_pattern_expr(inner)
            }
            Expr::List(items) => {
                for item in items {
                    self.check_no_pattern_expr(item)?;
                }
                Ok(())
            }
            Expr::MapLiteral(pairs) => {
                for (_, v) in pairs {
                    self.check_no_pattern_expr(v)?;
                }
                Ok(())
            }
            Expr::ListComp {
                list, filter, map, ..
            } => {
                self.check_no_pattern_expr(list)?;
                if let Some(f) = filter {
                    self.check_no_pattern_expr(f)?;
                }
                self.check_no_pattern_expr(map)
            }
            _ => Ok(()),
        }
    }
}
