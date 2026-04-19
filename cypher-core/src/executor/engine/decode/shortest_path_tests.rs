//! Tests for `Operator::ShortestPath` exercising the BFS arena's
//! path-reconstruction logic for both single- and all-path modes.

use crate::ast::{Direction, Identifier};
use crate::executor::budget::ExecutionBudget;
use crate::executor::engine::{ExecContext, MutationStats, QueryExecutor, Row};
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
fn shortest_path_finds_path() {
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
    store
        .create_edge("g", a, c, "R".into(), IndexMap::new())
        .unwrap();
    let mut start_row = Row::new();
    start_row.insert("src".to_string(), Value::Node(a));
    start_row.insert("dst".to_string(), Value::Node(c));
    let executor = QueryExecutor::new(&store);
    let plan = Operator::ShortestPath {
        src_alias: Identifier::new("src"),
        dst_alias: Identifier::new("dst"),
        rel_types: vec![],
        direction: Direction::Outgoing,
        min: 1,
        max: 10,
        path_alias: Identifier::new("p"),
        all_paths: false,
    };
    let mut budget = ExecutionBudget::unlimited();
    let mut stats = MutationStats::default();
    let mut cx = ExecContext {
        graph: "g",
        budget: &mut budget,
        outer_rows: &[start_row],
        stats: &mut stats,
    };
    let rows = executor.exec_operator(&mut cx, &plan).unwrap();
    assert_eq!(rows.len(), 1);
    match &rows[0]["p"] {
        Value::Path { nodes, edges } => {
            assert_eq!(edges.len(), 1);
            assert_eq!(nodes[0], a);
            assert_eq!(nodes[1], c);
        }
        other => panic!("expected Path, got {:?}", other),
    }
}

#[test]
fn shortest_path_all_paths() {
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
    let d = store
        .create_node("g", vec!["N".into()], IndexMap::new())
        .unwrap();
    store
        .create_edge("g", a, b, "R".into(), IndexMap::new())
        .unwrap();
    store
        .create_edge("g", b, c, "R".into(), IndexMap::new())
        .unwrap();
    store
        .create_edge("g", a, d, "R".into(), IndexMap::new())
        .unwrap();
    store
        .create_edge("g", d, c, "R".into(), IndexMap::new())
        .unwrap();
    let mut start_row = Row::new();
    start_row.insert("src".to_string(), Value::Node(a));
    start_row.insert("dst".to_string(), Value::Node(c));
    let executor = QueryExecutor::new(&store);
    let plan = Operator::ShortestPath {
        src_alias: Identifier::new("src"),
        dst_alias: Identifier::new("dst"),
        rel_types: vec![],
        direction: Direction::Outgoing,
        min: 1,
        max: 10,
        path_alias: Identifier::new("p"),
        all_paths: true,
    };
    let mut budget = ExecutionBudget::unlimited();
    let mut stats = MutationStats::default();
    let mut cx = ExecContext {
        graph: "g",
        budget: &mut budget,
        outer_rows: &[start_row],
        stats: &mut stats,
    };
    let rows = executor.exec_operator(&mut cx, &plan).unwrap();
    assert_eq!(rows.len(), 2);
    for row in &rows {
        match &row["p"] {
            Value::Path { edges, .. } => assert_eq!(edges.len(), 2),
            other => panic!("expected Path, got {:?}", other),
        }
    }
}

#[test]
fn shortest_path_no_path_exists() {
    let store = make_store();
    let a = store
        .create_node("g", vec!["N".into()], IndexMap::new())
        .unwrap();
    let b = store
        .create_node("g", vec!["N".into()], IndexMap::new())
        .unwrap();
    let mut start_row = Row::new();
    start_row.insert("src".to_string(), Value::Node(a));
    start_row.insert("dst".to_string(), Value::Node(b));
    let executor = QueryExecutor::new(&store);
    let plan = Operator::ShortestPath {
        src_alias: Identifier::new("src"),
        dst_alias: Identifier::new("dst"),
        rel_types: vec![],
        direction: Direction::Outgoing,
        min: 1,
        max: 5,
        path_alias: Identifier::new("p"),
        all_paths: false,
    };
    let mut budget = ExecutionBudget::unlimited();
    let mut stats = MutationStats::default();
    let mut cx = ExecContext {
        graph: "g",
        budget: &mut budget,
        outer_rows: &[start_row],
        stats: &mut stats,
    };
    let rows = executor.exec_operator(&mut cx, &plan).unwrap();
    assert_eq!(rows.len(), 0);
}
