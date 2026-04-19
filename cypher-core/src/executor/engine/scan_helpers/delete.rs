//! Tests for the `Delete` operator (detach, null, and edge deletion).

use super::common::make_store;
use crate::ast::{Direction, Expr, Identifier, Label, RelType};
use crate::executor::budget::ExecutionBudget;
use crate::executor::engine::QueryExecutor;
use crate::planner::Operator;
use crate::storage::GraphStore;
use indexmap::IndexMap;

#[test]
fn delete_node_detach() {
    let store = make_store();
    let a = store
        .create_node("g", vec!["Person".into()], IndexMap::new())
        .unwrap();
    let b = store
        .create_node("g", vec!["Person".into()], IndexMap::new())
        .unwrap();
    store
        .create_edge("g", a, b, "KNOWS".into(), IndexMap::new())
        .unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Delete {
        exprs: vec![Expr::Variable(Identifier::new("n"))],
        detach: true,
        child: Box::new(Operator::Filter {
            predicate: Expr::Bool(true),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("Person")),
                alias: Identifier::new("n"),
            }),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.stats.nodes_deleted, 2);
}

#[test]
fn delete_null_is_noop() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Delete {
        exprs: vec![Expr::Null],
        detach: false,
        child: Box::new(Operator::NodeScan {
            label: None,
            alias: Identifier::new("n"),
        }),
    };
    store.create_node("g", vec![], IndexMap::new()).unwrap();
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.stats.nodes_deleted, 0);
}

#[test]
fn delete_edge() {
    let store = make_store();
    let a = store
        .create_node("g", vec!["N".into()], IndexMap::new())
        .unwrap();
    let b = store
        .create_node("g", vec!["N".into()], IndexMap::new())
        .unwrap();
    let eid = store
        .create_edge("g", a, b, "R".into(), IndexMap::new())
        .unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Delete {
        exprs: vec![Expr::Variable(Identifier::new("r"))],
        detach: false,
        child: Box::new(Operator::Expand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("R")],
            src_alias: Identifier::new("a"),
            rel_alias: Some(Identifier::new("r")),
            dst_alias: Identifier::new("b"),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("a"),
            }),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.stats.edges_deleted, 1);
    assert!(store.get_edge("g", eid).unwrap().is_none());
}
