//! SKIP / LIMIT expression validation: rejects negatives, floats, and
//! obviously non-integer literal forms.

use crate::ast::{Expr, UnaryOp};
use crate::error::GqlError;

use super::{plan_error_semantic, ValidationContext};

impl ValidationContext {
    pub(in crate::planner::validator) fn validate_skip_limit(
        &self,
        expr: &Expr,
        clause_name: &str,
    ) -> Result<(), GqlError> {
        match expr {
            Expr::Integer(n) if *n < 0 => {
                return Err(plan_error_semantic(&format!(
                    "{clause_name} requires a non-negative integer, got {n}"
                )));
            }
            Expr::Float(_) => {
                return Err(plan_error_semantic(&format!(
                    "{clause_name} requires an integer, got a floating point value"
                )));
            }
            Expr::UnaryOp {
                op: UnaryOp::Neg,
                expr: inner,
            } => match inner.as_ref() {
                Expr::Integer(_) => {
                    return Err(plan_error_semantic(&format!(
                        "{clause_name} requires a non-negative integer"
                    )));
                }
                Expr::Float(_) => {
                    return Err(plan_error_semantic(&format!(
                        "{clause_name} requires an integer, got a floating point value"
                    )));
                }
                _ => {}
            },
            Expr::Str(_) | Expr::Bool(_) | Expr::Null | Expr::List(_) | Expr::MapLiteral(_) => {
                return Err(plan_error_semantic(&format!(
                    "{clause_name} requires an integer expression"
                )));
            }
            _ => {}
        }
        Ok(())
    }
}
