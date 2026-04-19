//! Tests for `NodeScan`, `Filter`, and `Expand` operators.

use super::common::make_store;
use crate::ast::{Direction, Expr, Identifier, Label, RelType};
use crate::executor::budget::ExecutionBudget;
use crate::executor::engine::QueryExecutor;
use crate::planner::Operator;
use crate::storage::GraphStore;
use crate::types::Value;
use indexmap::IndexMap;

#[test]
fn node_scan_no_label() {
    let store = make_store();
    store
        .create_node("g", vec!["Person".into()], IndexMap::new())
        .unwrap();
    store
        .create_node("g", vec!["Movie".into()], IndexMap::new())
        .unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::NodeScan {
        label: None,
        alias: Identifier::new("n"),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 2);
}

#[test]
fn node_scan_with_label() {
    let store = make_store();
    store
        .create_node("g", vec!["Person".into()], IndexMap::new())
        .unwrap();
    store
        .create_node("g", vec!["Movie".into()], IndexMap::new())
        .unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::NodeScan {
        label: Some(Label::new("Person")),
        alias: Identifier::new("n"),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
}

#[test]
fn filter_operator() {
    let store = make_store();
    let mut props = IndexMap::new();
    props.insert("age".to_string(), Value::Int(30));
    store
        .create_node("g", vec!["Person".into()], props)
        .unwrap();
    let mut props2 = IndexMap::new();
    props2.insert("age".to_string(), Value::Int(20));
    store
        .create_node("g", vec!["Person".into()], props2)
        .unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Filter {
        predicate: Expr::BinaryOp {
            op: crate::ast::BinaryOp::Gte,
            left: Box::new(Expr::Property(
                Box::new(Expr::Variable(Identifier::new("n"))),
                "age".into(),
            )),
            right: Box::new(Expr::Integer(25)),
        },
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("Person")),
            alias: Identifier::new("n"),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
}

#[test]
fn expand_operator() {
    let store = make_store();
    let alice = store
        .create_node("g", vec!["Person".into()], IndexMap::new())
        .unwrap();
    let bob = store
        .create_node("g", vec!["Person".into()], IndexMap::new())
        .unwrap();
    store
        .create_edge("g", alice, bob, "KNOWS".into(), IndexMap::new())
        .unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Expand {
        direction: Direction::Outgoing,
        rel_types: vec![RelType::new("KNOWS")],
        src_alias: Identifier::new("a"),
        rel_alias: Some(Identifier::new("r")),
        dst_alias: Identifier::new("b"),
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("Person")),
            alias: Identifier::new("a"),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    assert!(result.rows[0].contains_key("b"));
}
