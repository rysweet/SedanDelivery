//! Tests for `Union`, `Apply`, and `LeftOuterApply` operators.

use super::common::make_store;
use crate::ast::{Direction, Identifier, Label, RelType};
use crate::executor::budget::ExecutionBudget;
use crate::executor::engine::QueryExecutor;
use crate::planner::Operator;
use crate::storage::GraphStore;
use crate::types::Value;
use indexmap::IndexMap;

#[test]
fn union_all_concatenates() {
    let store = make_store();
    store
        .create_node("g", vec!["Person".into()], IndexMap::new())
        .unwrap();
    store
        .create_node("g", vec!["Movie".into()], IndexMap::new())
        .unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Union {
        left: Box::new(Operator::NodeScan {
            label: Some(Label::new("Person")),
            alias: Identifier::new("n"),
        }),
        right: Box::new(Operator::NodeScan {
            label: Some(Label::new("Movie")),
            alias: Identifier::new("n"),
        }),
        all: true,
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 2);
}

#[test]
fn union_deduplicates() {
    let store = make_store();
    store
        .create_node("g", vec!["Both".into()], IndexMap::new())
        .unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Union {
        left: Box::new(Operator::NodeScan {
            label: Some(Label::new("Both")),
            alias: Identifier::new("n"),
        }),
        right: Box::new(Operator::NodeScan {
            label: Some(Label::new("Both")),
            alias: Identifier::new("n"),
        }),
        all: false,
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
}

#[test]
fn left_outer_apply_with_match() {
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
    let plan = Operator::LeftOuterApply {
        preserved: Box::new(Operator::NodeScan {
            label: Some(Label::new("Person")),
            alias: Identifier::new("a"),
        }),
        optional_pattern: Box::new(Operator::Expand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("KNOWS")],
            src_alias: Identifier::new("a"),
            rel_alias: Some(Identifier::new("r")),
            dst_alias: Identifier::new("friend"),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("Person")),
                alias: Identifier::new("a"),
            }),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 2);
}

#[test]
fn left_outer_apply_no_match_produces_nulls() {
    let store = make_store();
    store
        .create_node("g", vec!["Person".into()], IndexMap::new())
        .unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::LeftOuterApply {
        preserved: Box::new(Operator::NodeScan {
            label: Some(Label::new("Person")),
            alias: Identifier::new("a"),
        }),
        optional_pattern: Box::new(Operator::Expand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("KNOWS")],
            src_alias: Identifier::new("a"),
            rel_alias: None,
            dst_alias: Identifier::new("b"),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("Person")),
                alias: Identifier::new("a"),
            }),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0].get("b"), Some(&Value::Null));
}

#[test]
fn apply_combines_outer_inner() {
    let store = make_store();
    store
        .create_node("g", vec!["A".into()], IndexMap::new())
        .unwrap();
    store
        .create_node("g", vec!["B".into()], IndexMap::new())
        .unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Apply {
        outer: Box::new(Operator::NodeScan {
            label: Some(Label::new("A")),
            alias: Identifier::new("a"),
        }),
        inner: Box::new(Operator::NodeScan {
            label: Some(Label::new("B")),
            alias: Identifier::new("b"),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    assert!(result.rows[0].contains_key("a"));
    assert!(result.rows[0].contains_key("b"));
}

#[test]
fn apply_empty_inner_passes_outer() {
    let store = make_store();
    store
        .create_node("g", vec!["A".into()], IndexMap::new())
        .unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Apply {
        outer: Box::new(Operator::NodeScan {
            label: Some(Label::new("A")),
            alias: Identifier::new("a"),
        }),
        inner: Box::new(Operator::NodeScan {
            label: Some(Label::new("NonExistent")),
            alias: Identifier::new("b"),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 0);
}
