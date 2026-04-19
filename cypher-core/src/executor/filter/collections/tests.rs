use std::collections::HashMap;

use crate::ast::{BinaryOp, Expr, Identifier};
use crate::executor::filter::eval_expr;
use crate::types::Value;

type Row<'a> = HashMap<&'a str, &'a Value>;

fn empty_row() -> Row<'static> {
    HashMap::new()
}

// ── reduce tests ─────────────────────────────────────────────────────────

#[test]
fn reduce_sum_integers() {
    // reduce(total = 0, x IN [1,2,3] | total + x) → 6
    let expr = Expr::Reduce {
        accumulator: Identifier::new("total"),
        initial: Box::new(Expr::Integer(0)),
        variable: Identifier::new("x"),
        list: Box::new(Expr::List(vec![
            Expr::Integer(1),
            Expr::Integer(2),
            Expr::Integer(3),
        ])),
        expression: Box::new(Expr::BinaryOp {
            op: BinaryOp::Add,
            left: Box::new(Expr::Variable(Identifier::new("total"))),
            right: Box::new(Expr::Variable(Identifier::new("x"))),
        }),
    };
    let result = eval_expr(&expr, &empty_row()).unwrap();
    assert_eq!(result, Value::Int(6));
}

#[test]
fn reduce_concat_strings() {
    // reduce(s = '', x IN ['a','b','c'] | s + x) → 'abc'
    let expr = Expr::Reduce {
        accumulator: Identifier::new("s"),
        initial: Box::new(Expr::Str("".into())),
        variable: Identifier::new("x"),
        list: Box::new(Expr::List(vec![
            Expr::Str("a".into()),
            Expr::Str("b".into()),
            Expr::Str("c".into()),
        ])),
        expression: Box::new(Expr::BinaryOp {
            op: BinaryOp::Add,
            left: Box::new(Expr::Variable(Identifier::new("s"))),
            right: Box::new(Expr::Variable(Identifier::new("x"))),
        }),
    };
    let result = eval_expr(&expr, &empty_row()).unwrap();
    assert_eq!(result, Value::Str("abc".into()));
}

#[test]
fn reduce_empty_list_returns_initial() {
    // reduce(total = 42, x IN [] | total + x) → 42
    let expr = Expr::Reduce {
        accumulator: Identifier::new("total"),
        initial: Box::new(Expr::Integer(42)),
        variable: Identifier::new("x"),
        list: Box::new(Expr::List(vec![])),
        expression: Box::new(Expr::BinaryOp {
            op: BinaryOp::Add,
            left: Box::new(Expr::Variable(Identifier::new("total"))),
            right: Box::new(Expr::Variable(Identifier::new("x"))),
        }),
    };
    let result = eval_expr(&expr, &empty_row()).unwrap();
    assert_eq!(result, Value::Int(42));
}

#[test]
fn reduce_null_list_returns_null() {
    let expr = Expr::Reduce {
        accumulator: Identifier::new("total"),
        initial: Box::new(Expr::Integer(0)),
        variable: Identifier::new("x"),
        list: Box::new(Expr::Null),
        expression: Box::new(Expr::BinaryOp {
            op: BinaryOp::Add,
            left: Box::new(Expr::Variable(Identifier::new("total"))),
            right: Box::new(Expr::Variable(Identifier::new("x"))),
        }),
    };
    let result = eval_expr(&expr, &empty_row()).unwrap();
    assert_eq!(result, Value::Null);
}

// ── extract tests (desugared to ListComp) ────────────────────────────────

#[test]
fn extract_doubles_values() {
    // extract(x IN [1,2,3] | x * 2) → [2,4,6]
    // Desugared to: [x IN [1,2,3] | x * 2]
    let expr = Expr::ListComp {
        variable: Identifier::new("x"),
        list: Box::new(Expr::List(vec![
            Expr::Integer(1),
            Expr::Integer(2),
            Expr::Integer(3),
        ])),
        filter: None,
        map: Box::new(Expr::BinaryOp {
            op: BinaryOp::Mul,
            left: Box::new(Expr::Variable(Identifier::new("x"))),
            right: Box::new(Expr::Integer(2)),
        }),
    };
    let result = eval_expr(&expr, &empty_row()).unwrap();
    assert_eq!(
        result,
        Value::List(vec![Value::Int(2), Value::Int(4), Value::Int(6)])
    );
}

// ── filter tests (desugared to ListComp) ─────────────────────────────────

#[test]
fn filter_greater_than_three() {
    // filter(x IN [1,2,3,4,5] WHERE x > 3) → [4,5]
    // Desugared to: [x IN [1,2,3,4,5] WHERE x > 3 | x]
    let expr = Expr::ListComp {
        variable: Identifier::new("x"),
        list: Box::new(Expr::List(vec![
            Expr::Integer(1),
            Expr::Integer(2),
            Expr::Integer(3),
            Expr::Integer(4),
            Expr::Integer(5),
        ])),
        filter: Some(Box::new(Expr::BinaryOp {
            op: BinaryOp::Gt,
            left: Box::new(Expr::Variable(Identifier::new("x"))),
            right: Box::new(Expr::Integer(3)),
        })),
        map: Box::new(Expr::Variable(Identifier::new("x"))),
    };
    let result = eval_expr(&expr, &empty_row()).unwrap();
    assert_eq!(result, Value::List(vec![Value::Int(4), Value::Int(5)]));
}
