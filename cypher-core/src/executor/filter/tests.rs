//! Tests extracted from the original `filter` module (#[cfg(test)] mod tests).
//!
//! Behavior preserved verbatim.

#![allow(clippy::approx_constant)]
use std::collections::HashMap;

use smol_str::SmolStr;

use super::*;
use crate::ast::{BinaryOp as BOp, Expr, Identifier, UnaryOp as UOp};
use crate::types::Value;

fn empty_row() -> Row<'static> {
    HashMap::new()
}

fn row_with<'a>(vars: &[(&'a str, &'a Value)]) -> Row<'a> {
    vars.iter().copied().collect()
}

// ── Literals ──────────────────────────────────────────────────────────────

#[test]
fn eval_integer_literal() {
    assert_eq!(
        eval_expr(&Expr::Integer(42), &empty_row()).unwrap(),
        Value::Int(42)
    );
}

#[test]
fn eval_float_literal() {
    assert_eq!(
        eval_expr(&Expr::Float(1.23456), &empty_row()).unwrap(),
        Value::Float(1.23456)
    );
}

#[test]
fn eval_string_literal() {
    assert_eq!(
        eval_expr(&Expr::str_lit("hello"), &empty_row()).unwrap(),
        Value::Str("hello".into())
    );
}

#[test]
fn eval_bool_literal() {
    assert_eq!(
        eval_expr(&Expr::Bool(true), &empty_row()).unwrap(),
        Value::Bool(true)
    );
}

#[test]
fn eval_null_literal() {
    assert_eq!(eval_expr(&Expr::Null, &empty_row()).unwrap(), Value::Null);
}

// ── Variable lookup ───────────────────────────────────────────────────────

#[test]
fn eval_variable_found() {
    let val = Value::Int(99);
    let row = row_with(&[("x", &val)]);
    assert_eq!(
        eval_expr(&Expr::Variable(Identifier::new("x")), &row).unwrap(),
        Value::Int(99)
    );
}

#[test]
fn eval_variable_not_found_errors() {
    let result = eval_expr(&Expr::Variable(Identifier::new("missing")), &empty_row());
    assert!(result.is_err());
}

// ── Property access ───────────────────────────────────────────────────────

#[test]
fn eval_property_on_map() {
    use indexmap::IndexMap;
    let mut m = IndexMap::new();
    m.insert(Box::from("name"), Value::Str("Alice".into()));
    let map_val = Value::Map(m);
    let row = row_with(&[("n", &map_val)]);
    let expr = Expr::Property(
        Box::new(Expr::Variable(Identifier::new("n"))),
        SmolStr::new("name"),
    );
    assert_eq!(eval_expr(&expr, &row).unwrap(), Value::Str("Alice".into()));
}

#[test]
fn eval_property_missing_returns_null() {
    use indexmap::IndexMap;
    let map_val = Value::Map(IndexMap::new());
    let row = row_with(&[("n", &map_val)]);
    let expr = Expr::Property(
        Box::new(Expr::Variable(Identifier::new("n"))),
        SmolStr::new("age"),
    );
    assert_eq!(eval_expr(&expr, &row).unwrap(), Value::Null);
}

#[test]
fn eval_property_on_null_returns_null() {
    let null_val = Value::Null;
    let row = row_with(&[("n", &null_val)]);
    let expr = Expr::Property(
        Box::new(Expr::Variable(Identifier::new("n"))),
        SmolStr::new("x"),
    );
    assert_eq!(eval_expr(&expr, &row).unwrap(), Value::Null);
}

// ── Null checks ───────────────────────────────────────────────────────────

#[test]
fn is_null_on_null() {
    assert_eq!(
        eval_expr(&Expr::IsNull(Box::new(Expr::Null)), &empty_row()).unwrap(),
        Value::Bool(true)
    );
}

#[test]
fn is_null_on_value() {
    assert_eq!(
        eval_expr(&Expr::IsNull(Box::new(Expr::Integer(1))), &empty_row()).unwrap(),
        Value::Bool(false)
    );
}

#[test]
fn is_not_null_on_value() {
    assert_eq!(
        eval_expr(&Expr::IsNotNull(Box::new(Expr::Integer(1))), &empty_row()).unwrap(),
        Value::Bool(true)
    );
}

// ── Arithmetic ────────────────────────────────────────────────────────────

#[test]
fn add_integers() {
    let expr = Expr::BinaryOp {
        op: BOp::Add,
        left: Box::new(Expr::Integer(3)),
        right: Box::new(Expr::Integer(4)),
    };
    assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Int(7));
}

#[test]
fn add_float_and_int() {
    let expr = Expr::BinaryOp {
        op: BOp::Add,
        left: Box::new(Expr::Float(1.5)),
        right: Box::new(Expr::Integer(2)),
    };
    assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Float(3.5));
}

#[test]
fn string_concatenation() {
    let expr = Expr::BinaryOp {
        op: BOp::Add,
        left: Box::new(Expr::str_lit("hello ")),
        right: Box::new(Expr::str_lit("world")),
    };
    assert_eq!(
        eval_expr(&expr, &empty_row()).unwrap(),
        Value::Str("hello world".into())
    );
}

#[test]
fn div_by_zero_errors() {
    let expr = Expr::BinaryOp {
        op: BOp::Div,
        left: Box::new(Expr::Integer(10)),
        right: Box::new(Expr::Integer(0)),
    };
    assert!(matches!(
        eval_expr(&expr, &empty_row()),
        Err(GqlError::Execution(ExecutionError::DivisionByZero))
    ));
}

#[test]
fn null_propagation_in_arithmetic() {
    let expr = Expr::BinaryOp {
        op: BOp::Add,
        left: Box::new(Expr::Null),
        right: Box::new(Expr::Integer(5)),
    };
    assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Null);
}

// ── Comparison ────────────────────────────────────────────────────────────

#[test]
fn eq_integers() {
    let expr = Expr::BinaryOp {
        op: BOp::Eq,
        left: Box::new(Expr::Integer(5)),
        right: Box::new(Expr::Integer(5)),
    };
    assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Bool(true));
}

#[test]
fn lt_comparison() {
    let expr = Expr::BinaryOp {
        op: BOp::Lt,
        left: Box::new(Expr::Integer(3)),
        right: Box::new(Expr::Integer(5)),
    };
    assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Bool(true));
}

#[test]
fn null_comparison_returns_null() {
    let expr = Expr::BinaryOp {
        op: BOp::Eq,
        left: Box::new(Expr::Null),
        right: Box::new(Expr::Integer(1)),
    };
    assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Null);
}

// ── Logical ───────────────────────────────────────────────────────────────

#[test]
fn and_short_circuits_on_false() {
    // false AND <error> should return false without evaluating right side
    let expr = Expr::BinaryOp {
        op: BOp::And,
        left: Box::new(Expr::Bool(false)),
        right: Box::new(Expr::Variable(Identifier::new("UNDEF"))),
    };
    assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Bool(false));
}

#[test]
fn or_short_circuits_on_true() {
    let expr = Expr::BinaryOp {
        op: BOp::Or,
        left: Box::new(Expr::Bool(true)),
        right: Box::new(Expr::Variable(Identifier::new("UNDEF"))),
    };
    assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Bool(true));
}

#[test]
fn not_true_is_false() {
    let expr = Expr::UnaryOp {
        op: UOp::Not,
        expr: Box::new(Expr::Bool(true)),
    };
    assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Bool(false));
}

#[test]
fn unary_neg() {
    let expr = Expr::UnaryOp {
        op: UOp::Neg,
        expr: Box::new(Expr::Integer(5)),
    };
    assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Int(-5));
}

// ── IN operator ───────────────────────────────────────────────────────────

#[test]
fn in_list_true() {
    let expr = Expr::BinaryOp {
        op: BOp::In,
        left: Box::new(Expr::Integer(2)),
        right: Box::new(Expr::List(vec![
            Expr::Integer(1),
            Expr::Integer(2),
            Expr::Integer(3),
        ])),
    };
    assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Bool(true));
}

#[test]
fn in_list_false() {
    let expr = Expr::BinaryOp {
        op: BOp::In,
        left: Box::new(Expr::Integer(9)),
        right: Box::new(Expr::List(vec![Expr::Integer(1), Expr::Integer(2)])),
    };
    assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Bool(false));
}

// ── String predicates ─────────────────────────────────────────────────────

#[test]
fn starts_with_true() {
    let expr = Expr::BinaryOp {
        op: BOp::StartsWith,
        left: Box::new(Expr::str_lit("Alice")),
        right: Box::new(Expr::str_lit("Al")),
    };
    assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Bool(true));
}

#[test]
fn contains_true() {
    let expr = Expr::BinaryOp {
        op: BOp::Contains,
        left: Box::new(Expr::str_lit("hello world")),
        right: Box::new(Expr::str_lit("lo wo")),
    };
    assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Bool(true));
}

// ── Function calls ────────────────────────────────────────────────────────

#[test]
fn abs_function() {
    let expr = Expr::FunctionCall {
        name: SmolStr::new("abs"),
        args: vec![Expr::Integer(-5)],
        distinct: false,
    };
    assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Int(5));
}

#[test]
fn size_of_list() {
    let expr = Expr::FunctionCall {
        name: SmolStr::new("size"),
        args: vec![Expr::List(vec![Expr::Integer(1), Expr::Integer(2)])],
        distinct: false,
    };
    assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Int(2));
}

#[test]
fn coalesce_returns_first_non_null() {
    let expr = Expr::FunctionCall {
        name: SmolStr::new("coalesce"),
        args: vec![Expr::Null, Expr::Null, Expr::Integer(42)],
        distinct: false,
    };
    assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Int(42));
}

#[test]
fn coalesce_all_null_returns_null() {
    let expr = Expr::FunctionCall {
        name: SmolStr::new("coalesce"),
        args: vec![Expr::Null, Expr::Null],
        distinct: false,
    };
    assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Null);
}

// ── CASE expression ───────────────────────────────────────────────────────

#[test]
fn case_generic_first_match() {
    let val = Value::Int(5);
    let row = row_with(&[("x", &val)]);
    let expr = Expr::Case {
        subject: None,
        alternatives: vec![
            (
                Expr::BinaryOp {
                    op: BOp::Gt,
                    left: Box::new(Expr::Variable(Identifier::new("x"))),
                    right: Box::new(Expr::Integer(10)),
                },
                Expr::str_lit("big"),
            ),
            (
                Expr::BinaryOp {
                    op: BOp::Gt,
                    left: Box::new(Expr::Variable(Identifier::new("x"))),
                    right: Box::new(Expr::Integer(3)),
                },
                Expr::str_lit("medium"),
            ),
        ],
        default: Some(Box::new(Expr::str_lit("small"))),
    };
    assert_eq!(eval_expr(&expr, &row).unwrap(), Value::Str("medium".into()));
}

#[test]
fn case_no_match_returns_null_without_default() {
    let val = Value::Int(1);
    let row = row_with(&[("x", &val)]);
    let expr = Expr::Case {
        subject: None,
        alternatives: vec![(
            Expr::BinaryOp {
                op: BOp::Gt,
                left: Box::new(Expr::Variable(Identifier::new("x"))),
                right: Box::new(Expr::Integer(100)),
            },
            Expr::str_lit("big"),
        )],
        default: None,
    };
    assert_eq!(eval_expr(&expr, &row).unwrap(), Value::Null);
}

// ── List comprehension ────────────────────────────────────────────────────

#[test]
fn list_comp_identity_no_filter_no_map() {
    // [x IN [1, 2, 3]] — no filter, map is identity (variable itself)
    let expr = Expr::ListComp {
        variable: Identifier::new("x"),
        list: Box::new(Expr::List(vec![
            Expr::Integer(1),
            Expr::Integer(2),
            Expr::Integer(3),
        ])),
        filter: None,
        map: Box::new(Expr::Variable(Identifier::new("x"))),
    };
    assert_eq!(
        eval_expr(&expr, &empty_row()).unwrap(),
        Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)])
    );
}

#[test]
fn list_comp_with_filter() {
    // [x IN [1, 2, 3, 4, 5] WHERE x > 2] → [3, 4, 5]
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
            op: BOp::Gt,
            left: Box::new(Expr::Variable(Identifier::new("x"))),
            right: Box::new(Expr::Integer(2)),
        })),
        map: Box::new(Expr::Variable(Identifier::new("x"))),
    };
    assert_eq!(
        eval_expr(&expr, &empty_row()).unwrap(),
        Value::List(vec![Value::Int(3), Value::Int(4), Value::Int(5)])
    );
}

#[test]
fn list_comp_with_map() {
    // [x IN [1, 2, 3] | x * 2] → [2, 4, 6]
    let expr = Expr::ListComp {
        variable: Identifier::new("x"),
        list: Box::new(Expr::List(vec![
            Expr::Integer(1),
            Expr::Integer(2),
            Expr::Integer(3),
        ])),
        filter: None,
        map: Box::new(Expr::BinaryOp {
            op: BOp::Mul,
            left: Box::new(Expr::Variable(Identifier::new("x"))),
            right: Box::new(Expr::Integer(2)),
        }),
    };
    assert_eq!(
        eval_expr(&expr, &empty_row()).unwrap(),
        Value::List(vec![Value::Int(2), Value::Int(4), Value::Int(6)])
    );
}

#[test]
fn list_comp_with_filter_and_map() {
    // [x IN [1, 2, 3, 4] WHERE x % 2 = 0 | x * 10] → [20, 40]
    let expr = Expr::ListComp {
        variable: Identifier::new("x"),
        list: Box::new(Expr::List(vec![
            Expr::Integer(1),
            Expr::Integer(2),
            Expr::Integer(3),
            Expr::Integer(4),
        ])),
        filter: Some(Box::new(Expr::BinaryOp {
            op: BOp::Eq,
            left: Box::new(Expr::BinaryOp {
                op: BOp::Mod,
                left: Box::new(Expr::Variable(Identifier::new("x"))),
                right: Box::new(Expr::Integer(2)),
            }),
            right: Box::new(Expr::Integer(0)),
        })),
        map: Box::new(Expr::BinaryOp {
            op: BOp::Mul,
            left: Box::new(Expr::Variable(Identifier::new("x"))),
            right: Box::new(Expr::Integer(10)),
        }),
    };
    assert_eq!(
        eval_expr(&expr, &empty_row()).unwrap(),
        Value::List(vec![Value::Int(20), Value::Int(40)])
    );
}

#[test]
fn list_comp_on_null_returns_null() {
    let expr = Expr::ListComp {
        variable: Identifier::new("x"),
        list: Box::new(Expr::Null),
        filter: None,
        map: Box::new(Expr::Variable(Identifier::new("x"))),
    };
    assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Null);
}

#[test]
fn list_comp_empty_list_returns_empty() {
    let expr = Expr::ListComp {
        variable: Identifier::new("x"),
        list: Box::new(Expr::List(vec![])),
        filter: None,
        map: Box::new(Expr::Variable(Identifier::new("x"))),
    };
    assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::List(vec![]));
}

// ── Built-in scalar functions ─────────────────────────────────────────────

#[test]
fn builtin_ceil_float() {
    assert_eq!(
        eval_function_with_vals("ceil", vec![Value::Float(2.3)]).unwrap(),
        Value::Float(3.0)
    );
}

#[test]
fn builtin_ceil_int_unchanged() {
    assert_eq!(
        eval_function_with_vals("ceil", vec![Value::Int(5)]).unwrap(),
        Value::Int(5)
    );
}

#[test]
fn builtin_floor_float() {
    assert_eq!(
        eval_function_with_vals("floor", vec![Value::Float(3.9)]).unwrap(),
        Value::Float(3.0)
    );
}

#[test]
fn builtin_round_float() {
    assert_eq!(
        eval_function_with_vals("round", vec![Value::Float(2.5)]).unwrap(),
        Value::Float(3.0)
    );
}

#[test]
fn builtin_sqrt_int() {
    assert_eq!(
        eval_function_with_vals("sqrt", vec![Value::Int(9)]).unwrap(),
        Value::Float(3.0)
    );
}

#[test]
fn builtin_sign_positive() {
    assert_eq!(
        eval_function_with_vals("sign", vec![Value::Int(5)]).unwrap(),
        Value::Int(1)
    );
}

#[test]
fn builtin_sign_negative() {
    assert_eq!(
        eval_function_with_vals("sign", vec![Value::Int(-3)]).unwrap(),
        Value::Int(-1)
    );
}

#[test]
fn builtin_tostring_int() {
    assert_eq!(
        eval_function_with_vals("toString", vec![Value::Int(42)]).unwrap(),
        Value::Str("42".into())
    );
}

#[test]
fn builtin_tostring_bool() {
    assert_eq!(
        eval_function_with_vals("toString", vec![Value::Bool(true)]).unwrap(),
        Value::Str("true".into())
    );
}

#[test]
fn builtin_tointeger_from_string() {
    assert_eq!(
        eval_function_with_vals("toInteger", vec![Value::Str("42".into())]).unwrap(),
        Value::Int(42)
    );
}

#[test]
fn builtin_tointeger_from_float() {
    assert_eq!(
        eval_function_with_vals("toInteger", vec![Value::Float(3.9)]).unwrap(),
        Value::Int(3)
    );
}

#[test]
fn builtin_tofloat_from_int() {
    assert_eq!(
        eval_function_with_vals("toFloat", vec![Value::Int(5)]).unwrap(),
        Value::Float(5.0)
    );
}

#[test]
fn builtin_tofloat_invalid_string_returns_null() {
    assert_eq!(
        eval_function_with_vals("toFloat", vec![Value::Str("abc".into())]).unwrap(),
        Value::Null
    );
}

#[test]
fn builtin_toboolean_true_string() {
    assert_eq!(
        eval_function_with_vals("toBoolean", vec![Value::Str("true".into())]).unwrap(),
        Value::Bool(true)
    );
}

#[test]
fn builtin_toboolean_invalid_returns_null() {
    assert_eq!(
        eval_function_with_vals("toBoolean", vec![Value::Str("yes".into())]).unwrap(),
        Value::Null
    );
}

#[test]
fn builtin_tolower() {
    assert_eq!(
        eval_function_with_vals("toLower", vec![Value::Str("HELLO".into())]).unwrap(),
        Value::Str("hello".into())
    );
}

#[test]
fn builtin_toupper() {
    assert_eq!(
        eval_function_with_vals("toUpper", vec![Value::Str("hello".into())]).unwrap(),
        Value::Str("HELLO".into())
    );
}

#[test]
fn builtin_trim() {
    assert_eq!(
        eval_function_with_vals("trim", vec![Value::Str("  hello  ".into())]).unwrap(),
        Value::Str("hello".into())
    );
}

#[test]
fn builtin_ltrim() {
    assert_eq!(
        eval_function_with_vals("ltrim", vec![Value::Str("  hello".into())]).unwrap(),
        Value::Str("hello".into())
    );
}

#[test]
fn builtin_rtrim() {
    assert_eq!(
        eval_function_with_vals("rtrim", vec![Value::Str("hello  ".into())]).unwrap(),
        Value::Str("hello".into())
    );
}

#[test]
fn builtin_left() {
    assert_eq!(
        eval_function_with_vals("left", vec![Value::Str("hello".into()), Value::Int(3)]).unwrap(),
        Value::Str("hel".into())
    );
}

#[test]
fn builtin_right() {
    assert_eq!(
        eval_function_with_vals("right", vec![Value::Str("hello".into()), Value::Int(3)]).unwrap(),
        Value::Str("llo".into())
    );
}

#[test]
fn builtin_substring_with_start_and_length() {
    assert_eq!(
        eval_function_with_vals(
            "substring",
            vec![Value::Str("hello".into()), Value::Int(1), Value::Int(3)]
        )
        .unwrap(),
        Value::Str("ell".into())
    );
}

#[test]
fn builtin_replace() {
    assert_eq!(
        eval_function_with_vals(
            "replace",
            vec![
                Value::Str("hello".into()),
                Value::Str("l".into()),
                Value::Str("r".into()),
            ]
        )
        .unwrap(),
        Value::Str("herro".into())
    );
}

#[test]
fn builtin_split() {
    assert_eq!(
        eval_function_with_vals(
            "split",
            vec![Value::Str("a,b,c".into()), Value::Str(",".into())]
        )
        .unwrap(),
        Value::List(vec![
            Value::Str("a".into()),
            Value::Str("b".into()),
            Value::Str("c".into()),
        ])
    );
}

#[test]
fn builtin_head() {
    assert_eq!(
        eval_function_with_vals(
            "head",
            vec![Value::List(vec![
                Value::Int(1),
                Value::Int(2),
                Value::Int(3)
            ])]
        )
        .unwrap(),
        Value::Int(1)
    );
}

#[test]
fn builtin_last() {
    assert_eq!(
        eval_function_with_vals(
            "last",
            vec![Value::List(vec![
                Value::Int(1),
                Value::Int(2),
                Value::Int(3)
            ])]
        )
        .unwrap(),
        Value::Int(3)
    );
}

#[test]
fn builtin_tail() {
    assert_eq!(
        eval_function_with_vals(
            "tail",
            vec![Value::List(vec![
                Value::Int(1),
                Value::Int(2),
                Value::Int(3)
            ])]
        )
        .unwrap(),
        Value::List(vec![Value::Int(2), Value::Int(3)])
    );
}

#[test]
fn builtin_reverse_list() {
    assert_eq!(
        eval_function_with_vals(
            "reverse",
            vec![Value::List(vec![
                Value::Int(1),
                Value::Int(2),
                Value::Int(3)
            ])]
        )
        .unwrap(),
        Value::List(vec![Value::Int(3), Value::Int(2), Value::Int(1)])
    );
}

#[test]
fn builtin_range_basic() {
    assert_eq!(
        eval_function_with_vals("range", vec![Value::Int(1), Value::Int(4)]).unwrap(),
        Value::List(vec![
            Value::Int(1),
            Value::Int(2),
            Value::Int(3),
            Value::Int(4),
        ])
    );
}

#[test]
fn builtin_keys_returns_non_null_keys() {
    use indexmap::IndexMap;
    let mut m: IndexMap<Box<str>, Value> = IndexMap::new();
    m.insert(Box::from("a"), Value::Int(1));
    m.insert(Box::from("b"), Value::Int(2));
    let result = eval_function_with_vals("keys", vec![Value::Map(m)]).unwrap();
    assert_eq!(
        result,
        Value::List(vec![Value::Str("a".into()), Value::Str("b".into())])
    );
}

#[test]
fn builtin_properties_returns_map_clone() {
    use indexmap::IndexMap;
    let mut m: IndexMap<Box<str>, Value> = IndexMap::new();
    m.insert(Box::from("x"), Value::Int(42));
    let val = Value::Map(m.clone());
    let result = eval_function_with_vals("properties", vec![val]).unwrap();
    assert_eq!(result, Value::Map(m));
}

// ── Null three-valued logic ───────────────────────────────────────────────

#[test]
fn null_and_true_is_null() {
    let expr = Expr::BinaryOp {
        op: BOp::And,
        left: Box::new(Expr::Null),
        right: Box::new(Expr::Bool(true)),
    };
    assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Null);
}

#[test]
fn null_and_false_is_false() {
    let expr = Expr::BinaryOp {
        op: BOp::And,
        left: Box::new(Expr::Null),
        right: Box::new(Expr::Bool(false)),
    };
    assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Bool(false));
}

#[test]
fn null_or_true_is_true() {
    let expr = Expr::BinaryOp {
        op: BOp::Or,
        left: Box::new(Expr::Null),
        right: Box::new(Expr::Bool(true)),
    };
    assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Bool(true));
}

#[test]
fn null_or_false_is_null() {
    let expr = Expr::BinaryOp {
        op: BOp::Or,
        left: Box::new(Expr::Null),
        right: Box::new(Expr::Bool(false)),
    };
    assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Null);
}

#[test]
fn not_null_is_null() {
    let expr = Expr::UnaryOp {
        op: UOp::Not,
        expr: Box::new(Expr::Null),
    };
    assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Null);
}

#[test]
fn in_list_with_null_element_not_found_is_null() {
    // 4 IN [1, null, 3] → null (element not present, but null exists)
    let expr = Expr::BinaryOp {
        op: BOp::In,
        left: Box::new(Expr::Integer(4)),
        right: Box::new(Expr::List(vec![
            Expr::Integer(1),
            Expr::Null,
            Expr::Integer(3),
        ])),
    };
    assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Null);
}

#[test]
fn in_list_with_null_element_found_is_true() {
    // 3 IN [1, null, 3] → true (exact match found before null check matters)
    let expr = Expr::BinaryOp {
        op: BOp::In,
        left: Box::new(Expr::Integer(3)),
        right: Box::new(Expr::List(vec![
            Expr::Integer(1),
            Expr::Null,
            Expr::Integer(3),
        ])),
    };
    assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Bool(true));
}

#[test]
fn null_in_empty_list_is_false() {
    // null IN [] → false
    let expr = Expr::BinaryOp {
        op: BOp::In,
        left: Box::new(Expr::Null),
        right: Box::new(Expr::List(vec![])),
    };
    assert_eq!(eval_expr(&expr, &empty_row()).unwrap(), Value::Bool(false));
}

// ── Integer boundaries ────────────────────────────────────────────────────

#[test]
fn i64_max_plus_one_returns_overflow_error() {
    // Checked arithmetic: i64::MAX + 1 returns an overflow error.
    let expr = Expr::BinaryOp {
        op: BOp::Add,
        left: Box::new(Expr::Integer(i64::MAX)),
        right: Box::new(Expr::Integer(1)),
    };
    assert!(eval_expr(&expr, &empty_row()).is_err());
}

#[test]
fn abs_i64_min_returns_overflow_error() {
    // Checked abs(): i64::MIN returns an overflow error instead of panicking.
    assert!(eval_function_with_vals("abs", vec![Value::Int(i64::MIN)]).is_err());
}

#[test]
fn neg_i64_min_returns_overflow_error() {
    // Checked negation: i64::MIN returns an overflow error instead of panicking.
    let expr = Expr::UnaryOp {
        op: UOp::Neg,
        expr: Box::new(Expr::Integer(i64::MIN)),
    };
    assert!(eval_expr(&expr, &empty_row()).is_err());
}

#[test]
#[should_panic]
fn i64_min_div_neg_one_overflows() {
    // i64::MIN / -1 overflows (result doesn't fit in i64) in debug mode.
    let expr = Expr::BinaryOp {
        op: BOp::Div,
        left: Box::new(Expr::Integer(i64::MIN)),
        right: Box::new(Expr::Integer(-1)),
    };
    let _ = eval_expr(&expr, &empty_row());
}

// -- Regex cache eviction tests ------------------------------------------

/// Helper: evaluate `string =~ pattern` and return the boolean result.
fn eval_regex_match(string: &str, pattern: &str) -> Result<Value, GqlError> {
    let expr = Expr::BinaryOp {
        op: BOp::RegexMatch,
        left: Box::new(Expr::str_lit(string)),
        right: Box::new(Expr::str_lit(pattern)),
    };
    eval_expr(&expr, &empty_row())
}

#[test]
fn regex_match_requires_full_string() {
    // Substring match must return false — Cypher =~ is implicitly anchored.
    assert_eq!(
        eval_regex_match("hello world", "hello").unwrap(),
        Value::Bool(false),
        "substring pattern should NOT match"
    );
}

#[test]
fn regex_match_exact_string() {
    assert_eq!(
        eval_regex_match("hello", "hello").unwrap(),
        Value::Bool(true),
        "exact pattern should match the full string"
    );
}

#[test]
fn regex_match_full_with_wildcard() {
    assert_eq!(
        eval_regex_match("hello", "hel.*").unwrap(),
        Value::Bool(true),
        "regex that covers the full string should match"
    );
}

#[test]
fn regex_match_already_anchored() {
    assert_eq!(
        eval_regex_match("hello", "^hello$").unwrap(),
        Value::Bool(true),
        "already-anchored pattern should still work"
    );
}

#[test]
fn regex_cache_basic_hit() {
    // First call compiles; second call should hit cache. Both return true.
    assert_eq!(eval_regex_match("abc", "^abc$").unwrap(), Value::Bool(true));
    assert_eq!(eval_regex_match("abc", "^abc$").unwrap(), Value::Bool(true));
}

#[test]
fn regex_cache_eviction_preserves_recent_entries() {
    for i in 0..513 {
        let pattern = format!("^pattern_{i}$");
        let input = format!("pattern_{i}");
        assert_eq!(
            eval_regex_match(&input, &pattern).unwrap(),
            Value::Bool(true),
            "pattern_{i} should match"
        );
    }

    REGEX_CACHE.with(|cache| {
        let map = cache.borrow();
        assert!(
            map.len() <= 512,
            "cache should not exceed 512 entries, got {}",
            map.len()
        );
        assert!(
            map.len() > 1,
            "cache should retain entries after eviction, got {}",
            map.len()
        );
        // Cache keys are anchored: ^(?:<original>)$
        assert!(
            map.contains_key("^(?:^pattern_512$)$"),
            "most recent pattern should survive eviction"
        );
        assert!(
            !map.contains_key("^(?:^pattern_0$)$"),
            "oldest pattern should have been evicted"
        );
    });

    REGEX_CACHE.with(|cache| cache.borrow_mut().clear());
}

#[test]
fn regex_cache_eviction_keeps_half() {
    for i in 0..512 {
        let pattern = format!("^evict_{i}$");
        let input = format!("evict_{i}");
        assert_eq!(
            eval_regex_match(&input, &pattern).unwrap(),
            Value::Bool(true)
        );
    }

    REGEX_CACHE.with(|cache| {
        assert_eq!(
            cache.borrow().len(),
            512,
            "cache should have 512 entries before eviction"
        );
    });

    assert_eq!(
        eval_regex_match("evict_512", "^evict_512$").unwrap(),
        Value::Bool(true)
    );

    REGEX_CACHE.with(|cache| {
        let map = cache.borrow();
        assert_eq!(
            map.len(),
            257,
            "after eviction cache should have 257 entries (256 survivors + 1 new)"
        );
        // Cache keys are anchored: ^(?:<original>)$
        assert!(!map.contains_key("^(?:^evict_0$)$"));
        assert!(!map.contains_key("^(?:^evict_255$)$"));
        assert!(map.contains_key("^(?:^evict_256$)$"));
        assert!(map.contains_key("^(?:^evict_511$)$"));
        assert!(map.contains_key("^(?:^evict_512$)$"));
    });

    REGEX_CACHE.with(|cache| cache.borrow_mut().clear());
}
