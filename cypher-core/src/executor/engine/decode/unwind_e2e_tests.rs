//! Tests for `Operator::Unwind` row decoding plus end-to-end planner-driven
//! coverage of variable-length expansion and CREATE-path semantics.

use crate::ast::{Expr, Identifier};
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
fn unwind_list() {
    let store = make_store();
    store.create_node("g", vec![], IndexMap::new()).unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Unwind {
        list_expr: Expr::List(vec![Expr::Integer(1), Expr::Integer(2), Expr::Integer(3)]),
        alias: Identifier::new("x"),
        child: Box::new(Operator::NodeScan {
            label: None,
            alias: Identifier::new("n"),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 3);
    let values: Vec<&Value> = result.rows.iter().map(|r| &r["x"]).collect();
    assert_eq!(values, vec![&Value::Int(1), &Value::Int(2), &Value::Int(3)]);
}

#[test]
fn unwind_null_produces_zero_rows() {
    let store = make_store();
    store.create_node("g", vec![], IndexMap::new()).unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Unwind {
        list_expr: Expr::Null,
        alias: Identifier::new("x"),
        child: Box::new(Operator::NodeScan {
            label: None,
            alias: Identifier::new("n"),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 0);
}

#[test]
fn unwind_empty_list_produces_zero_rows() {
    let store = make_store();
    store.create_node("g", vec![], IndexMap::new()).unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Unwind {
        list_expr: Expr::List(vec![]),
        alias: Identifier::new("x"),
        child: Box::new(Operator::NodeScan {
            label: None,
            alias: Identifier::new("n"),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 0);
}

#[test]
fn e2e_varlen_expand_through_planner() {
    use crate::parser::parse_query;
    use crate::planner::QueryPlanner;
    let store = make_store();
    let setup = "CREATE (a {name: 'A'}), (b {name: 'B'}), (c {name: 'C'}), (d {name: 'D'}) CREATE (a)-[:CONTAINS]->(b), (b)-[:CONTAINS]->(c), (c)-[:CONTAINS]->(d)";
    let ast = parse_query(setup).unwrap();
    let planner = QueryPlanner::new(&store, "g");
    let plan = planner.plan(&ast).unwrap();
    let mut budget = ExecutionBudget::unlimited();
    QueryExecutor::new(&store)
        .execute("g", &plan, &mut budget)
        .unwrap();
    let query = "MATCH (a {name: 'A'})-[*]->(x) RETURN x";
    let ast2 = parse_query(query).unwrap();
    let plan2 = planner.plan(&ast2).unwrap();
    let mut budget2 = ExecutionBudget::unlimited();
    let result = QueryExecutor::new(&store)
        .execute("g", &plan2, &mut budget2)
        .unwrap();
    assert_eq!(result.rows.len(), 3, "Expected 3 rows: B, C, D");
}

#[test]
fn e2e_create_path_variable_returns_path() {
    use crate::parser::parse_query;
    use crate::planner::QueryPlanner;
    let store = make_store();
    let planner = QueryPlanner::new(&store, "g");
    let query = "CREATE p = (a:X)-[r:Y]->(b:Z) RETURN p";
    let ast = parse_query(query).unwrap();
    let plan = planner.plan(&ast).unwrap();
    let mut budget = ExecutionBudget::unlimited();
    let result = QueryExecutor::new(&store)
        .execute("g", &plan, &mut budget)
        .unwrap();
    assert_eq!(
        result.rows.len(),
        1,
        "CREATE should produce exactly one row"
    );
    let row = &result.rows[0];
    let p = row.get("p").expect("row should contain path variable 'p'");
    match p {
        Value::Path { nodes, edges } => {
            assert_eq!(nodes.len(), 2, "path should have 2 nodes");
            assert_eq!(edges.len(), 1, "path should have 1 edge");
        }
        other => panic!("expected Value::Path, got {:?}", other),
    }
}
