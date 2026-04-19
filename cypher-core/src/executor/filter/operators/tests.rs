//! Dispatch-level tests: unary operators and short-circuit AND/OR/XOR.

use super::test_utils::{bin, lit_b, lit_f, lit_i, lit_null, lit_s, run, una};
use crate::ast::{BinaryOp, UnaryOp};
use crate::types::Value;

// ── Unary ─────────────────────────────────────────────────────────
#[test]
fn neg_int() {
    assert_eq!(run(&una(UnaryOp::Neg, lit_i(5))).unwrap(), Value::Int(-5));
}
#[test]
fn neg_float() {
    assert_eq!(
        run(&una(UnaryOp::Neg, lit_f(1.5))).unwrap(),
        Value::Float(-1.5)
    );
}
#[test]
fn neg_null() {
    assert_eq!(run(&una(UnaryOp::Neg, lit_null())).unwrap(), Value::Null);
}
#[test]
fn neg_overflow() {
    assert!(run(&una(UnaryOp::Neg, lit_i(i64::MIN))).is_err());
}
#[test]
fn neg_string_errs() {
    assert!(run(&una(UnaryOp::Neg, lit_s("a"))).is_err());
}
#[test]
fn not_bool() {
    assert_eq!(
        run(&una(UnaryOp::Not, lit_b(true))).unwrap(),
        Value::Bool(false)
    );
}
#[test]
fn not_null() {
    assert_eq!(run(&una(UnaryOp::Not, lit_null())).unwrap(), Value::Null);
}
#[test]
fn not_int_errs() {
    assert!(run(&una(UnaryOp::Not, lit_i(0))).is_err());
}

// ── AND / OR / XOR ───────────────────────────────────────────────
#[test]
fn and_true_false() {
    assert_eq!(
        run(&bin(BinaryOp::And, lit_b(true), lit_b(false))).unwrap(),
        Value::Bool(false)
    );
}
#[test]
fn and_short_circuit_false() {
    assert_eq!(
        run(&bin(BinaryOp::And, lit_b(false), lit_i(99))).unwrap(),
        Value::Bool(false)
    );
}
#[test]
fn and_null_false() {
    assert_eq!(
        run(&bin(BinaryOp::And, lit_null(), lit_b(false))).unwrap(),
        Value::Bool(false)
    );
    assert_eq!(
        run(&bin(BinaryOp::And, lit_b(false), lit_null())).unwrap(),
        Value::Bool(false)
    );
}
#[test]
fn and_null_true() {
    assert_eq!(
        run(&bin(BinaryOp::And, lit_null(), lit_b(true))).unwrap(),
        Value::Null
    );
}
#[test]
fn and_type_err() {
    assert!(run(&bin(BinaryOp::And, lit_i(1), lit_b(true))).is_err());
}
#[test]
fn or_short_circuit_true() {
    assert_eq!(
        run(&bin(BinaryOp::Or, lit_b(true), lit_i(99))).unwrap(),
        Value::Bool(true)
    );
}
#[test]
fn or_null_true() {
    assert_eq!(
        run(&bin(BinaryOp::Or, lit_null(), lit_b(true))).unwrap(),
        Value::Bool(true)
    );
}
#[test]
fn or_null_null() {
    assert_eq!(
        run(&bin(BinaryOp::Or, lit_null(), lit_b(false))).unwrap(),
        Value::Null
    );
}
#[test]
fn or_type_err() {
    assert!(run(&bin(BinaryOp::Or, lit_b(false), lit_i(1))).is_err());
}
#[test]
fn xor_basic() {
    assert_eq!(
        run(&bin(BinaryOp::Xor, lit_b(true), lit_b(false))).unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        run(&bin(BinaryOp::Xor, lit_b(true), lit_b(true))).unwrap(),
        Value::Bool(false)
    );
}
#[test]
fn xor_null() {
    assert_eq!(
        run(&bin(BinaryOp::Xor, lit_null(), lit_b(true))).unwrap(),
        Value::Null
    );
}
#[test]
fn xor_type_err() {
    assert!(run(&bin(BinaryOp::Xor, lit_i(1), lit_i(2))).is_err());
}
