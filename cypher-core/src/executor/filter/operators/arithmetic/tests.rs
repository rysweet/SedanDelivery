//! Unit tests for arithmetic operators (split out of `arithmetic/mod.rs`).

use super::super::test_utils::{bin, lit_f, lit_i, lit_null, lit_s, run};
use crate::ast::{BinaryOp, Expr};
use crate::types::Value;

#[test]
fn add_ints() {
    assert_eq!(
        run(&bin(BinaryOp::Add, lit_i(2), lit_i(3))).unwrap(),
        Value::Int(5)
    );
}
#[test]
fn add_int_overflow() {
    assert!(run(&bin(BinaryOp::Add, lit_i(i64::MAX), lit_i(1))).is_err());
}
#[test]
fn add_floats_mixed() {
    assert_eq!(
        run(&bin(BinaryOp::Add, lit_f(1.5), lit_f(2.5))).unwrap(),
        Value::Float(4.0)
    );
    assert_eq!(
        run(&bin(BinaryOp::Add, lit_i(2), lit_f(0.5))).unwrap(),
        Value::Float(2.5)
    );
    assert_eq!(
        run(&bin(BinaryOp::Add, lit_f(0.5), lit_i(2))).unwrap(),
        Value::Float(2.5)
    );
}
#[test]
fn add_strings() {
    assert_eq!(
        run(&bin(BinaryOp::Add, lit_s("a"), lit_s("b"))).unwrap(),
        Value::Str("ab".into())
    );
}
#[test]
fn add_null() {
    assert_eq!(
        run(&bin(BinaryOp::Add, lit_null(), lit_i(1))).unwrap(),
        Value::Null
    );
}
#[test]
fn add_invalid_err() {
    assert!(run(&bin(BinaryOp::Add, Expr::Bool(true), Expr::Bool(false))).is_err());
}
#[test]
fn add_list_list() {
    let l = Expr::List(vec![lit_i(1)]);
    let r = Expr::List(vec![lit_i(2)]);
    assert_eq!(
        run(&bin(BinaryOp::Add, l, r)).unwrap(),
        Value::List(vec![Value::Int(1), Value::Int(2)])
    );
}
#[test]
fn add_list_element() {
    let l = Expr::List(vec![lit_i(1)]);
    assert_eq!(
        run(&bin(BinaryOp::Add, l, lit_i(2))).unwrap(),
        Value::List(vec![Value::Int(1), Value::Int(2)])
    );
}
#[test]
fn add_element_list() {
    let r = Expr::List(vec![lit_i(2)]);
    assert_eq!(
        run(&bin(BinaryOp::Add, lit_i(1), r)).unwrap(),
        Value::List(vec![Value::Int(1), Value::Int(2)])
    );
}
#[test]
fn sub_ints() {
    assert_eq!(
        run(&bin(BinaryOp::Sub, lit_i(5), lit_i(3))).unwrap(),
        Value::Int(2)
    );
}
#[test]
fn sub_overflow() {
    assert!(run(&bin(BinaryOp::Sub, lit_i(i64::MIN), lit_i(1))).is_err());
}
#[test]
fn sub_mixed_float() {
    assert_eq!(
        run(&bin(BinaryOp::Sub, lit_f(2.5), lit_i(1))).unwrap(),
        Value::Float(1.5)
    );
    assert_eq!(
        run(&bin(BinaryOp::Sub, lit_i(2), lit_f(0.5))).unwrap(),
        Value::Float(1.5)
    );
    assert_eq!(
        run(&bin(BinaryOp::Sub, lit_f(2.5), lit_f(0.5))).unwrap(),
        Value::Float(2.0)
    );
}
#[test]
fn sub_null() {
    assert_eq!(
        run(&bin(BinaryOp::Sub, lit_null(), lit_i(1))).unwrap(),
        Value::Null
    );
}
#[test]
fn sub_invalid() {
    assert!(run(&bin(BinaryOp::Sub, lit_s("a"), lit_s("b"))).is_err());
}
#[test]
fn mul_ints() {
    assert_eq!(
        run(&bin(BinaryOp::Mul, lit_i(2), lit_i(3))).unwrap(),
        Value::Int(6)
    );
}
#[test]
fn mul_overflow() {
    assert!(run(&bin(BinaryOp::Mul, lit_i(i64::MAX), lit_i(2))).is_err());
}
#[test]
fn mul_floats_mixed() {
    assert_eq!(
        run(&bin(BinaryOp::Mul, lit_f(1.5), lit_f(2.0))).unwrap(),
        Value::Float(3.0)
    );
    assert_eq!(
        run(&bin(BinaryOp::Mul, lit_i(2), lit_f(0.5))).unwrap(),
        Value::Float(1.0)
    );
    assert_eq!(
        run(&bin(BinaryOp::Mul, lit_f(0.5), lit_i(2))).unwrap(),
        Value::Float(1.0)
    );
}
#[test]
fn mul_null() {
    assert_eq!(
        run(&bin(BinaryOp::Mul, lit_null(), lit_i(1))).unwrap(),
        Value::Null
    );
}
#[test]
fn mul_invalid() {
    assert!(run(&bin(BinaryOp::Mul, lit_s("a"), lit_s("b"))).is_err());
}
#[test]
fn div_ints() {
    assert_eq!(
        run(&bin(BinaryOp::Div, lit_i(10), lit_i(3))).unwrap(),
        Value::Int(3)
    );
}
#[test]
fn div_int_by_zero() {
    assert!(run(&bin(BinaryOp::Div, lit_i(1), lit_i(0))).is_err());
}
#[test]
fn div_floats_mixed() {
    assert_eq!(
        run(&bin(BinaryOp::Div, lit_f(1.0), lit_f(2.0))).unwrap(),
        Value::Float(0.5)
    );
    assert_eq!(
        run(&bin(BinaryOp::Div, lit_i(1), lit_f(2.0))).unwrap(),
        Value::Float(0.5)
    );
    assert_eq!(
        run(&bin(BinaryOp::Div, lit_f(1.0), lit_i(2))).unwrap(),
        Value::Float(0.5)
    );
}
#[test]
fn div_null() {
    assert_eq!(
        run(&bin(BinaryOp::Div, lit_null(), lit_i(1))).unwrap(),
        Value::Null
    );
}
#[test]
fn div_invalid() {
    assert!(run(&bin(BinaryOp::Div, lit_s("a"), lit_s("b"))).is_err());
}
#[test]
fn mod_ints() {
    assert_eq!(
        run(&bin(BinaryOp::Mod, lit_i(10), lit_i(3))).unwrap(),
        Value::Int(1)
    );
}
#[test]
fn mod_int_by_zero() {
    assert!(run(&bin(BinaryOp::Mod, lit_i(1), lit_i(0))).is_err());
}
#[test]
fn mod_floats_mixed() {
    assert_eq!(
        run(&bin(BinaryOp::Mod, lit_f(5.0), lit_f(3.0))).unwrap(),
        Value::Float(2.0)
    );
    assert_eq!(
        run(&bin(BinaryOp::Mod, lit_i(5), lit_f(3.0))).unwrap(),
        Value::Float(2.0)
    );
    assert_eq!(
        run(&bin(BinaryOp::Mod, lit_f(5.0), lit_i(3))).unwrap(),
        Value::Float(2.0)
    );
}
#[test]
fn mod_null() {
    assert_eq!(
        run(&bin(BinaryOp::Mod, lit_null(), lit_i(1))).unwrap(),
        Value::Null
    );
}
#[test]
fn mod_invalid() {
    assert!(run(&bin(BinaryOp::Mod, lit_s("a"), lit_s("b"))).is_err());
}
#[test]
fn pow_variants() {
    assert_eq!(
        run(&bin(BinaryOp::Pow, lit_i(2), lit_i(3))).unwrap(),
        Value::Float(8.0)
    );
    assert_eq!(
        run(&bin(BinaryOp::Pow, lit_f(2.0), lit_f(3.0))).unwrap(),
        Value::Float(8.0)
    );
    assert_eq!(
        run(&bin(BinaryOp::Pow, lit_i(2), lit_f(3.0))).unwrap(),
        Value::Float(8.0)
    );
    assert_eq!(
        run(&bin(BinaryOp::Pow, lit_f(2.0), lit_i(3))).unwrap(),
        Value::Float(8.0)
    );
}
#[test]
fn pow_null() {
    assert_eq!(
        run(&bin(BinaryOp::Pow, lit_null(), lit_i(1))).unwrap(),
        Value::Null
    );
}
#[test]
fn pow_invalid() {
    assert!(run(&bin(BinaryOp::Pow, lit_s("a"), lit_s("b"))).is_err());
}
