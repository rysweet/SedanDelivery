//! Unit tests for produce/output operators.

use crate::ast::{Expr, Identifier, Label};
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
fn produce_results_with_property() {
    let store = make_store();
    let mut props = IndexMap::new();
    props.insert("name".to_string(), Value::Str("Alice".into()));
    store
        .create_node("g", vec!["Person".to_string()], props)
        .unwrap();

    let executor = QueryExecutor::new(&store);
    let plan = Operator::ProduceResults {
        columns: vec![(
            "p.name".to_string(),
            Expr::Property(
                Box::new(Expr::Variable(Identifier::new("p"))),
                "name".into(),
            ),
        )],
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("Person")),
            alias: Identifier::new("p"),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0]["p.name"], Value::Str("Alice".into()));
}

#[test]
fn produce_results_empty_columns() {
    let store = make_store();
    store.create_node("g", vec![], IndexMap::new()).unwrap();

    let executor = QueryExecutor::new(&store);
    let plan = Operator::ProduceResults {
        columns: vec![],
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
fn produce_results_star() {
    let store = make_store();
    let mut props = IndexMap::new();
    props.insert("x".to_string(), Value::Int(1));
    store
        .create_node("g", vec!["N".to_string()], props)
        .unwrap();

    let executor = QueryExecutor::new(&store);
    let plan = Operator::ProduceResults {
        columns: vec![("*".to_string(), Expr::Variable(Identifier::new("*")))],
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("n"),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    assert!(result.rows[0].contains_key("n"));
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
fn unwind_empty_list() {
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
fn unwind_non_list_wraps_scalar() {
    let store = make_store();
    store.create_node("g", vec![], IndexMap::new()).unwrap();

    let executor = QueryExecutor::new(&store);
    let plan = Operator::Unwind {
        list_expr: Expr::Integer(42),
        alias: Identifier::new("x"),
        child: Box::new(Operator::NodeScan {
            label: None,
            alias: Identifier::new("n"),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget);
    // Non-list scalars are now wrapped as single-element results (Gremlin unfold semantics)
    assert!(result.is_ok());
    let qr = result.unwrap();
    assert_eq!(qr.rows.len(), 1);
    assert_eq!(qr.rows[0].get("x"), Some(&Value::Int(42)));
}

#[test]
fn union_all_concatenates() {
    let store = make_store();
    store
        .create_node("g", vec!["Person".to_string()], IndexMap::new())
        .unwrap();
    store
        .create_node("g", vec!["Movie".to_string()], IndexMap::new())
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
        .create_node("g", vec!["Both".to_string()], IndexMap::new())
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
fn assemble_path_simple() {
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

    assert_eq!(result.rows.len(), 1);
    let p = result.rows[0].get("p").expect("should have path p");
    match p {
        Value::Path { nodes, edges } => {
            assert_eq!(nodes.len(), 2);
            assert_eq!(edges.len(), 1);
        }
        other => panic!("expected Value::Path, got {:?}", other),
    }
}
