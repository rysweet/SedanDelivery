//! Shared expression-construction helpers for operator submodule tests.

use std::collections::HashMap;

use crate::ast::{BinaryOp, Expr, UnaryOp};
use crate::error::GqlError;
use crate::executor::filter::{eval_expr, Row};
use crate::types::Value;

pub(super) fn empty_row<'a>() -> Row<'a> {
    HashMap::new()
}
pub(super) fn lit_i(n: i64) -> Expr {
    Expr::Integer(n)
}
pub(super) fn lit_f(n: f64) -> Expr {
    Expr::Float(n)
}
pub(super) fn lit_s(s: &str) -> Expr {
    Expr::Str(s.into())
}
pub(super) fn lit_b(b: bool) -> Expr {
    Expr::Bool(b)
}
pub(super) fn lit_null() -> Expr {
    Expr::Null
}
pub(super) fn bin(op: BinaryOp, l: Expr, r: Expr) -> Expr {
    Expr::BinaryOp {
        op,
        left: Box::new(l),
        right: Box::new(r),
    }
}
pub(super) fn una(op: UnaryOp, e: Expr) -> Expr {
    Expr::UnaryOp {
        op,
        expr: Box::new(e),
    }
}
pub(super) fn run(e: &Expr) -> Result<Value, GqlError> {
    eval_expr(e, &empty_row())
}
