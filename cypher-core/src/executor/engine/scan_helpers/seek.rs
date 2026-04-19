//! Tests for `IndexScan` and `NodeByIdSeek` operators.

use super::common::make_store;
use crate::ast::{Expr, Identifier, Label};
use crate::executor::budget::ExecutionBudget;
use crate::executor::engine::QueryExecutor;
use crate::planner::Operator;
use crate::storage::GraphStore;
use crate::types::Value;
use indexmap::IndexMap;

#[test]
fn index_scan_finds_nodes() {
    let store = make_store();
    let mut p1 = IndexMap::new();
    p1.insert("age".to_string(), Value::Int(30));
    store.create_node("g", vec!["Person".into()], p1).unwrap();
    let mut p2 = IndexMap::new();
    p2.insert("age".to_string(), Value::Int(25));
    store.create_node("g", vec!["Person".into()], p2).unwrap();
    store
        .create_node("g", vec!["Person".into()], IndexMap::new())
        .unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::IndexScan {
        label: Label::new("Person"),
        property: "age".to_string(),
        value: None,
        alias: Identifier::new("n"),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 2);
}

#[test]
fn index_scan_empty_result() {
    let store = make_store();
    store
        .create_node("g", vec!["Person".into()], IndexMap::new())
        .unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::IndexScan {
        label: Label::new("Person"),
        property: "nonexistent".to_string(),
        value: None,
        alias: Identifier::new("n"),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 0);
}

#[test]
fn node_by_id_seek_found() {
    let store = make_store();
    let node_id = store
        .create_node("g", vec!["Person".into()], IndexMap::new())
        .unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::NodeByIdSeek {
        id_expr: Expr::Integer(node_id.0 as i64),
        alias: Identifier::new("n"),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0]["n"], Value::Node(node_id));
}

#[test]
fn node_by_id_seek_not_found() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::NodeByIdSeek {
        id_expr: Expr::Integer(99999),
        alias: Identifier::new("n"),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 0);
}

#[test]
fn node_by_id_seek_negative_id_is_error() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::NodeByIdSeek {
        id_expr: Expr::Integer(-1),
        alias: Identifier::new("n"),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget);
    assert!(result.is_err());
}
