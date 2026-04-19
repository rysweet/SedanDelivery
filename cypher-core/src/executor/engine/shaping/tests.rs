use super::super::QueryExecutor;
use crate::ast::{BinaryOp, Expr, Identifier, Label};
use crate::executor::budget::ExecutionBudget;
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
fn filter_keeps_matching() {
    let store = make_store();
    let mut p = IndexMap::new();
    p.insert("x".to_string(), Value::Int(10));
    store.create_node("g", vec!["N".to_string()], p).unwrap();
    store
        .create_node("g", vec!["N".to_string()], IndexMap::new())
        .unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Filter {
        predicate: Expr::BinaryOp {
            op: BinaryOp::Gte,
            left: Box::new(Expr::Property(
                Box::new(Expr::Variable(Identifier::new("n"))),
                "x".into(),
            )),
            right: Box::new(Expr::Integer(5)),
        },
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("n"),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
}

#[test]
fn distinct_removes_duplicates() {
    let store = make_store();
    store
        .create_node("g", vec!["N".to_string()], IndexMap::new())
        .unwrap();
    store
        .create_node("g", vec!["N".to_string()], IndexMap::new())
        .unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Distinct {
        child: Box::new(Operator::Project {
            items: vec![(Expr::Integer(1), Some(Identifier::new("x")))],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("n"),
            }),
        }),
        by_key: None,
        productive: false,
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
}

#[test]
fn sort_ascending() {
    let store = make_store();
    for name in &["C", "A", "B"] {
        let mut p = IndexMap::new();
        p.insert("name".to_string(), Value::Str((*name).into()));
        store.create_node("g", vec!["P".to_string()], p).unwrap();
    }
    let executor = QueryExecutor::new(&store);
    let plan = Operator::ProduceResults {
        columns: vec![(
            "n.name".to_string(),
            Expr::Property(
                Box::new(Expr::Variable(Identifier::new("n"))),
                "name".into(),
            ),
        )],
        child: Box::new(Operator::Sort {
            items: vec![(
                Expr::Property(
                    Box::new(Expr::Variable(Identifier::new("n"))),
                    "name".into(),
                ),
                true,
            )],
            gremlin_order: false,
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("P")),
                alias: Identifier::new("n"),
            }),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows[0]["n.name"], Value::Str("A".into()));
    assert_eq!(result.rows[2]["n.name"], Value::Str("C".into()));
}

#[test]
fn limit_and_skip() {
    let store = make_store();
    for _ in 0..5 {
        store.create_node("g", vec![], IndexMap::new()).unwrap();
    }
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Limit {
        count: Expr::Integer(2),
        child: Box::new(Operator::Skip {
            count: Expr::Integer(1),
            child: Box::new(Operator::NodeScan {
                label: None,
                alias: Identifier::new("n"),
            }),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 2);
}

/// LIMIT over a bare NodeScan triggers early termination: on 1000 nodes,
/// LIMIT 10 should NOT materialise all 1000 rows. Verify that the
/// budget's leaf_rows_produced counter stays well below the total node
/// count.
#[test]
fn limit_early_termination_avoids_full_scan() {
    let store = make_store();
    let total_nodes = 1000;
    for _ in 0..total_nodes {
        store
            .create_node("g", vec!["N".to_string()], IndexMap::new())
            .unwrap();
    }
    let executor = QueryExecutor::new(&store);
    // Plan: Limit { 10, NodeScan }
    let plan = Operator::Limit {
        count: Expr::Integer(10),
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("n"),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 10, "LIMIT should return exactly 10 rows");
    // The total rows_produced should be far less than 1000.
    // With early termination, NodeScan produces only ~10 leaf rows.
    // budget.rows_produced() includes both leaf and intermediate rows,
    // but should still be much less than 1000.
    assert!(
        budget.rows_produced() < 100,
        "Expected early termination to avoid full scan; \
         rows_produced={} but total_nodes={}",
        budget.rows_produced(),
        total_nodes,
    );
}

/// LIMIT over Project(NodeScan) also triggers early termination.
#[test]
fn limit_over_project_early_termination() {
    let store = make_store();
    let total_nodes = 500;
    for i in 0..total_nodes {
        let mut p = IndexMap::new();
        p.insert("i".to_string(), Value::Int(i));
        store.create_node("g", vec!["N".to_string()], p).unwrap();
    }
    let executor = QueryExecutor::new(&store);
    // Plan: Limit { 5, Project { items: [n], child: NodeScan } }
    let plan = Operator::Limit {
        count: Expr::Integer(5),
        child: Box::new(Operator::Project {
            items: vec![(
                Expr::Variable(Identifier::new("n")),
                Some(Identifier::new("n")),
            )],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("n"),
            }),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 5);
    assert!(
        budget.rows_produced() < 100,
        "Expected early termination through Project; rows_produced={}",
        budget.rows_produced(),
    );
}

/// LIMIT over Sort(NodeScan) does NOT early-terminate (sort needs all rows).
#[test]
fn limit_over_sort_no_early_termination() {
    let store = make_store();
    let total_nodes = 50;
    for i in 0..total_nodes {
        let mut p = IndexMap::new();
        p.insert("i".to_string(), Value::Int(i));
        store.create_node("g", vec!["N".to_string()], p).unwrap();
    }
    let executor = QueryExecutor::new(&store);
    // Plan: Limit { 5, Sort { child: NodeScan } }
    let plan = Operator::Limit {
        count: Expr::Integer(5),
        child: Box::new(Operator::Sort {
            items: vec![(
                Expr::Property(Box::new(Expr::Variable(Identifier::new("n"))), "i".into()),
                true,
            )],
            gremlin_order: false,
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("n"),
            }),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 5);
    // Sort needs all rows so early termination must NOT fire.
    // rows_produced >= total_nodes because NodeScan produces 50 rows.
    assert!(
        budget.rows_produced() >= total_nodes as usize,
        "Sort should prevent early termination; rows_produced={}",
        budget.rows_produced(),
    );
}

/// SKIP 3 LIMIT 2 with early termination: should produce exactly 2 rows
/// and the scan should materialise only skip+limit=5 rows.
#[test]
fn skip_limit_early_termination() {
    let store = make_store();
    let total_nodes = 200;
    for _ in 0..total_nodes {
        store
            .create_node("g", vec!["N".to_string()], IndexMap::new())
            .unwrap();
    }
    let executor = QueryExecutor::new(&store);
    // Plan: Limit { 2, Skip { 3, NodeScan } }
    let plan = Operator::Limit {
        count: Expr::Integer(2),
        child: Box::new(Operator::Skip {
            count: Expr::Integer(3),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("n"),
            }),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 2, "SKIP 3 LIMIT 2 should return 2 rows");
    // With early termination, NodeScan should produce only 5 leaf rows
    // (skip=3 + limit=2). Budget rows_produced includes both leaf and
    // intermediate counts, but should be far less than 200.
    assert!(
        budget.rows_produced() < 50,
        "Expected early termination for SKIP+LIMIT; rows_produced={} but total_nodes={}",
        budget.rows_produced(),
        total_nodes,
    );
}
