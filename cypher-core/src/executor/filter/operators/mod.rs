//! Binary and unary operator evaluation, comparison logic.
//!
//! The dispatch logic lives here. Each operator family lives in a submodule:
//! [`arithmetic`] (`+`, `-`, `*`, `/`, `%`, `^`), [`comparison`] (`=`, `<>`,
//! `<`, `>`, `<=`, `>=`, plus the shared `cypher_compare` helper),
//! [`string_ops`] (`STARTS WITH`, `ENDS WITH`, `CONTAINS`, `=~`), and
//! [`list_ops`] (`IN`). Logical operators (`AND`/`OR`/`XOR`) and the unary
//! operators (`-`, `NOT`) stay here because they require expression-level
//! short-circuit semantics or are trivial enough not to warrant a module.

use crate::ast::{BinaryOp, UnaryOp};
use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

use super::{eval_expr, Row};

mod arithmetic;
mod comparison;
mod list_ops;
mod string_ops;

#[cfg(test)]
mod test_utils;
#[cfg(test)]
mod tests;

pub(crate) use comparison::{cypher_compare, safe_int_float_cmp};

// ── Unary operator evaluation ─────────────────────────────────────────────────

pub(super) fn eval_unary<'r>(
    op: UnaryOp,
    expr: &crate::ast::Expr,
    row: &Row<'r>,
) -> Result<Value, GqlError> {
    let val = eval_expr(expr, row)?;
    match op {
        UnaryOp::Neg => match val {
            Value::Null => Ok(Value::Null),
            Value::Int(n) => Ok(Value::Int(n.checked_neg().ok_or_else(|| {
                GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "integer arithmetic without overflow".to_string(),
                    actual: "integer overflow".to_string(),
                })
            })?)),
            Value::Float(f) => Ok(Value::Float(-f)),
            other => Err(GqlError::Execution(ExecutionError::InvalidOperator {
                op: "-".to_string(),
                left: other.type_name().to_string(),
                right: "".to_string(),
            })),
        },
        UnaryOp::Not => match val {
            Value::Null => Ok(Value::Null),
            Value::Bool(b) => Ok(Value::Bool(!b)),
            other => Err(GqlError::Execution(ExecutionError::InvalidOperator {
                op: "NOT".to_string(),
                left: other.type_name().to_string(),
                right: "".to_string(),
            })),
        },
    }
}

// ── Binary operator evaluation ────────────────────────────────────────────────

pub(super) fn eval_binary<'r>(
    op: BinaryOp,
    left: &crate::ast::Expr,
    right: &crate::ast::Expr,
    row: &Row<'r>,
) -> Result<Value, GqlError> {
    // Short-circuit AND / OR before evaluating both sides.
    match op {
        BinaryOp::And => {
            let lv = eval_expr(left, row)?;
            if lv == Value::Bool(false) {
                return Ok(Value::Bool(false));
            }
            let rv = eval_expr(right, row)?;
            return match (lv, rv) {
                (Value::Null, Value::Bool(false)) | (Value::Bool(false), Value::Null) => {
                    Ok(Value::Bool(false))
                }
                (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
                (Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(a && b)),
                (l, r) => Err(GqlError::Execution(ExecutionError::InvalidOperator {
                    op: "AND".to_string(),
                    left: l.type_name().to_string(),
                    right: r.type_name().to_string(),
                })),
            };
        }
        BinaryOp::Or => {
            let lv = eval_expr(left, row)?;
            if lv == Value::Bool(true) {
                return Ok(Value::Bool(true));
            }
            let rv = eval_expr(right, row)?;
            return match (lv, rv) {
                (Value::Null, Value::Bool(true)) | (Value::Bool(true), Value::Null) => {
                    Ok(Value::Bool(true))
                }
                (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
                (Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(a || b)),
                (l, r) => Err(GqlError::Execution(ExecutionError::InvalidOperator {
                    op: "OR".to_string(),
                    left: l.type_name().to_string(),
                    right: r.type_name().to_string(),
                })),
            };
        }
        _ => {}
    }

    let lv = eval_expr(left, row)?;
    let rv = eval_expr(right, row)?;

    match op {
        BinaryOp::And | BinaryOp::Or => unreachable!("handled above"),

        BinaryOp::Xor => match (lv, rv) {
            (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
            (Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(a ^ b)),
            (l, r) => Err(GqlError::Execution(ExecutionError::InvalidOperator {
                op: "XOR".to_string(),
                left: l.type_name().to_string(),
                right: r.type_name().to_string(),
            })),
        },

        BinaryOp::Add | BinaryOp::Concat => arithmetic::add(lv, rv),
        BinaryOp::Sub => arithmetic::sub(lv, rv),
        BinaryOp::Mul => arithmetic::mul(lv, rv),
        BinaryOp::Div => arithmetic::div(lv, rv),
        BinaryOp::Mod => arithmetic::modulo(lv, rv),
        BinaryOp::Pow => arithmetic::pow(lv, rv),

        BinaryOp::Eq => Ok(comparison::eq(&lv, &rv)),
        BinaryOp::Neq => Ok(comparison::neq(&lv, &rv)),
        BinaryOp::Lt => comparison::compare(&lv, &rv, std::cmp::Ordering::Less, false),
        BinaryOp::Lte => comparison::compare(&lv, &rv, std::cmp::Ordering::Less, true),
        BinaryOp::Gt => comparison::compare(&lv, &rv, std::cmp::Ordering::Greater, false),
        BinaryOp::Gte => comparison::compare(&lv, &rv, std::cmp::Ordering::Greater, true),

        BinaryOp::StartsWith => Ok(string_ops::starts_with(lv, rv)),
        BinaryOp::EndsWith => Ok(string_ops::ends_with(lv, rv)),
        BinaryOp::Contains => Ok(string_ops::contains(lv, rv)),
        BinaryOp::RegexMatch => string_ops::regex_match(lv, rv),

        BinaryOp::In => list_ops::in_op(lv, rv),
    }
}
