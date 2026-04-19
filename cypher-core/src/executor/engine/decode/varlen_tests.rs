//! Tests for variable-length expansion (`Operator::VarLenExpand`) using the
//! BFS arena defined in the parent module.

use crate::ast::{Direction, Identifier, Label, RelType};
use crate::executor::budget::ExecutionBudget;
use crate::executor::engine::QueryExecutor;
use crate::planner::Operator;
use crate::storage::{GraphStore, InMemoryGraphStore};
use crate::types::Value;
use indexmap::IndexMap;

fn make_store() -> InMemoryGraphStore {
    let store = InMemoryGraphStore::new();
    store.create_graph("g").unwrap();
    store
}

#[test]
fn varlen_expand_linear_chain() {
    let store = make_store();
    let a = store
        .create_node("g", vec!["N".into()], IndexMap::new())
        .unwrap();
    let b = store
        .create_node("g", vec!["N".into()], IndexMap::new())
        .unwrap();
    let c = store
        .create_node("g", vec!["N".into()], IndexMap::new())
        .unwrap();
    store
        .create_edge("g", a, b, "R".into(), IndexMap::new())
        .unwrap();
    store
        .create_edge("g", b, c, "R".into(), IndexMap::new())
        .unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::VarLenExpand {
        direction: Direction::Outgoing,
        rel_types: vec![RelType::new("R")],
        min: 1,
        max: 2,
        src_alias: Identifier::new("a"),
        path_alias: Some(Identifier::new("p")),
        path_mode: true,
        dst_alias: Identifier::new("b"),
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("a"),
        }),
        rel_props: None,
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 3);
    let paths: Vec<_> = result
        .rows
        .iter()
        .filter(|r| matches!(r.get("p"), Some(Value::Path { .. })))
        .collect();
    assert_eq!(paths.len(), 3);
}

#[test]
fn varlen_expand_min_zero_includes_self() {
    let store = make_store();
    let a = store
        .create_node("g", vec!["N".into()], IndexMap::new())
        .unwrap();
    let b = store
        .create_node("g", vec!["N".into()], IndexMap::new())
        .unwrap();
    store
        .create_edge("g", a, b, "R".into(), IndexMap::new())
        .unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::VarLenExpand {
        direction: Direction::Outgoing,
        rel_types: vec![],
        min: 0,
        max: 1,
        src_alias: Identifier::new("src"),
        path_alias: None,
        path_mode: false,
        dst_alias: Identifier::new("dst"),
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("src"),
        }),
        rel_props: None,
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert!(result.rows.len() >= 2);
}

#[test]
fn varlen_expand_avoids_cycles() {
    let store = make_store();
    let a = store
        .create_node("g", vec!["N".into()], IndexMap::new())
        .unwrap();
    let b = store
        .create_node("g", vec!["N".into()], IndexMap::new())
        .unwrap();
    store
        .create_edge("g", a, b, "R".into(), IndexMap::new())
        .unwrap();
    store
        .create_edge("g", b, a, "R".into(), IndexMap::new())
        .unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::VarLenExpand {
        direction: Direction::Outgoing,
        rel_types: vec![],
        min: 1,
        max: 5,
        src_alias: Identifier::new("src"),
        path_alias: None,
        path_mode: false,
        dst_alias: Identifier::new("dst"),
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("src"),
        }),
        rel_props: None,
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert!(!result.rows.is_empty());
    assert!(result.rows.len() < 100);
}
