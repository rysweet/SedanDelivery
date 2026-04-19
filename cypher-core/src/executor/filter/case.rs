//! CASE expression evaluator.
//!
//! Behavior preserved verbatim from the original `filter` module.

use crate::ast::Expr;
use crate::error::GqlError;
use crate::types::Value;

use super::{eval_expr, Row};

// ── CASE evaluation ───────────────────────────────────────────────────────────

pub(crate) fn eval_case<'r>(
    subject: Option<&Expr>,
    alternatives: &[(Expr, Expr)],
    default: Option<&Expr>,
    row: &Row<'r>,
) -> Result<Value, GqlError> {
    let subject_val = match subject {
        Some(e) => Some(eval_expr(e, row)?),
        None => None,
    };

    for (condition, result) in alternatives {
        let matches = match &subject_val {
            None => {
                // Generic CASE: WHEN is a full boolean predicate.
                let cond_val = eval_expr(condition, row)?;
                cond_val.is_truthy()
            }
            Some(sv) => {
                // Value CASE: WHEN is a value to compare against subject.
                let when_val = eval_expr(condition, row)?;
                sv.cypher_eq(&when_val).unwrap_or(false)
            }
        };
        if matches {
            return eval_expr(result, row);
        }
    }

    match default {
        Some(e) => eval_expr(e, row),
        None => Ok(Value::Null),
    }
}
