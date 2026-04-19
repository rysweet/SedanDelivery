//! Tests for `MATCH` planning: simple, multi-pattern, index-scan upgrades.

use super::*;
use crate::ast::{
    BinaryOp, Clause, Expr, Identifier, Label, MatchClause, NodePattern, PathPattern, Properties,
    Query, ReturnClause, ReturnItem,
};
use crate::executor::budget::ExecutionBudget;
use crate::executor::engine::QueryExecutor;
use crate::storage::InMemoryGraphStore;
use crate::types::Value;
use indexmap::IndexMap;

#[test]
fn plan_simple_match_return() {
    let store = make_store();
    let planner = QueryPlanner::new(&store, "g");
    let query = make_simple_match_query("Person", "p");
    let plan = planner.plan(&query).unwrap();
    // Root must be ProduceResults.
    assert!(matches!(plan, Operator::ProduceResults { .. }));
}

#[test]
fn plan_match_with_where() {
    let store = make_store();
    let planner = QueryPlanner::new(&store, "g");
    let query = Query {
        unions: Vec::new(),
        clauses: vec![
            Clause::Match(MatchClause {
                patterns: vec![PathPattern {
                    variable: None,
                    start: NodePattern {
                        variable: Some(Identifier::new("n")),
                        labels: vec![Label::new("Person")],
                        properties: None,
                        where_clause: None,
                    },
                    steps: vec![],
                    shortest_path: None,
                }],
                where_clause: Some(Expr::BinaryOp {
                    op: crate::ast::BinaryOp::Gt,
                    left: Box::new(Expr::Property(
                        Box::new(Expr::Variable(Identifier::new("n"))),
                        "age".into(),
                    )),
                    right: Box::new(Expr::Integer(18)),
                }),
            }),
            Clause::Return(ReturnClause {
                distinct: false,
                items: vec![ReturnItem {
                    expr: Expr::Variable(Identifier::new("n")),
                    alias: None,
                }],
                order_by: vec![],
                skip: None,
                limit: None,
            }),
        ],
    };
    let plan = planner.plan(&query).unwrap();
    // Should have a Filter in the tree.
    assert!(plan_contains_filter(&plan));
}

// ── Multi-pattern MATCH tests ─────────────────────────────────────────────

#[test]
fn plan_multi_pattern_match() {
    // MATCH (a:Person), (b:Movie) RETURN a, b
    let store = make_store();
    let planner = QueryPlanner::new(&store, "g");
    let query = Query {
        unions: Vec::new(),
        clauses: vec![
            Clause::Match(MatchClause {
                patterns: vec![
                    PathPattern {
                        variable: None,
                        start: NodePattern {
                            variable: Some(Identifier::new("a")),
                            labels: vec![Label::new("Person")],
                            properties: None,
                            where_clause: None,
                        },
                        steps: vec![],
                        shortest_path: None,
                    },
                    PathPattern {
                        variable: None,
                        start: NodePattern {
                            variable: Some(Identifier::new("b")),
                            labels: vec![Label::new("Movie")],
                            properties: None,
                            where_clause: None,
                        },
                        steps: vec![],
                        shortest_path: None,
                    },
                ],
                where_clause: None,
            }),
            Clause::Return(ReturnClause {
                distinct: false,
                items: vec![
                    ReturnItem {
                        expr: Expr::Variable(Identifier::new("a")),
                        alias: None,
                    },
                    ReturnItem {
                        expr: Expr::Variable(Identifier::new("b")),
                        alias: None,
                    },
                ],
                order_by: vec![],
                skip: None,
                limit: None,
            }),
        ],
    };
    let plan = planner.plan(&query).unwrap();
    // Multi-pattern MATCH produces an Apply (cross-product) in the tree.
    assert!(plan_contains_apply(&plan));
}

#[test]
fn plan_multi_pattern_match_with_where() {
    // MATCH (a:Person), (b:Movie) WHERE a.name = 'Alice' RETURN a, b
    let store = make_store();
    let planner = QueryPlanner::new(&store, "g");
    let query = Query {
        unions: Vec::new(),
        clauses: vec![
            Clause::Match(MatchClause {
                patterns: vec![
                    PathPattern {
                        variable: None,
                        start: NodePattern {
                            variable: Some(Identifier::new("a")),
                            labels: vec![Label::new("Person")],
                            properties: None,
                            where_clause: None,
                        },
                        steps: vec![],
                        shortest_path: None,
                    },
                    PathPattern {
                        variable: None,
                        start: NodePattern {
                            variable: Some(Identifier::new("b")),
                            labels: vec![Label::new("Movie")],
                            properties: None,
                            where_clause: None,
                        },
                        steps: vec![],
                        shortest_path: None,
                    },
                ],
                where_clause: Some(Expr::BinaryOp {
                    op: BinaryOp::Eq,
                    left: Box::new(Expr::Property(
                        Box::new(Expr::Variable(Identifier::new("a"))),
                        "name".into(),
                    )),
                    right: Box::new(Expr::Str("Alice".into())),
                }),
            }),
            Clause::Return(ReturnClause {
                distinct: false,
                items: vec![
                    ReturnItem {
                        expr: Expr::Variable(Identifier::new("a")),
                        alias: None,
                    },
                    ReturnItem {
                        expr: Expr::Variable(Identifier::new("b")),
                        alias: None,
                    },
                ],
                order_by: vec![],
                skip: None,
                limit: None,
            }),
        ],
    };
    let plan = planner.plan(&query).unwrap();
    assert!(plan_contains_apply(&plan));
    assert!(plan_contains_filter(&plan));
}

// ── Index scan tests ──────────────────────────────────────────────────────

#[test]
fn plan_node_scan_upgrades_to_index_scan_when_index_exists() {
    // Given a store with an index on Person.name
    let store = InMemoryGraphStore::new();
    store.create_graph("g").unwrap();
    store.create_index("g", "Person", "name").unwrap();

    let planner = QueryPlanner::new(&store, "g");
    // MATCH (n:Person {name: 'Alice'}) RETURN n
    let query = Query {
        unions: Vec::new(),
        clauses: vec![
            Clause::Match(MatchClause {
                patterns: vec![PathPattern {
                    variable: None,
                    start: NodePattern {
                        variable: Some(Identifier::new("n")),
                        labels: vec![Label::new("Person")],
                        properties: Some(Properties::Map(vec![(
                            smol_str::SmolStr::new("name"),
                            Expr::Str("Alice".into()),
                        )])),
                        where_clause: None,
                    },
                    steps: vec![],
                    shortest_path: None,
                }],
                where_clause: None,
            }),
            Clause::Return(ReturnClause {
                distinct: false,
                items: vec![ReturnItem {
                    expr: Expr::Variable(Identifier::new("n")),
                    alias: None,
                }],
                order_by: vec![],
                skip: None,
                limit: None,
            }),
        ],
    };
    let plan = planner.plan(&query).unwrap();
    assert!(
        plan_contains_index_scan(&plan),
        "Expected IndexScan in plan when index exists for Person.name, got: {plan:?}"
    );
}

#[test]
fn plan_no_declared_index_still_uses_index_scan_with_inline_props() {
    // Even without a declared index the planner should emit IndexScan
    // when inline properties are present — find_nodes_by_label_and_property
    // always filters selectively. Using NodeScan here causes Cartesian
    // product explosions in multi-pattern MATCH (issue #1090).
    let store = make_store(); // no indexes
    let planner = QueryPlanner::new(&store, "g");
    let query = Query {
        unions: Vec::new(),
        clauses: vec![
            Clause::Match(MatchClause {
                patterns: vec![PathPattern {
                    variable: None,
                    start: NodePattern {
                        variable: Some(Identifier::new("n")),
                        labels: vec![Label::new("Person")],
                        properties: Some(Properties::Map(vec![(
                            smol_str::SmolStr::new("name"),
                            Expr::Str("Alice".into()),
                        )])),
                        where_clause: None,
                    },
                    steps: vec![],
                    shortest_path: None,
                }],
                where_clause: None,
            }),
            Clause::Return(ReturnClause {
                distinct: false,
                items: vec![ReturnItem {
                    expr: Expr::Variable(Identifier::new("n")),
                    alias: None,
                }],
                order_by: vec![],
                skip: None,
                limit: None,
            }),
        ],
    };
    let plan = planner.plan(&query).unwrap();
    assert!(
        plan_contains_index_scan(&plan),
        "With inline properties, planner should use IndexScan even without declared index, got: {plan:?}"
    );
}

#[test]
fn index_scan_end_to_end_plan_and_execute() {
    // Set up store with an index and data.
    let store = InMemoryGraphStore::new();
    store.create_graph("g").unwrap();
    store.create_index("g", "Person", "name").unwrap();

    let p1 = IndexMap::from([("name".to_string(), Value::Str("Alice".into()))]);
    let alice = store
        .create_node("g", vec!["Person".to_string()], p1)
        .unwrap();
    let p2 = IndexMap::from([("name".to_string(), Value::Str("Bob".into()))]);
    let _bob = store
        .create_node("g", vec!["Person".to_string()], p2)
        .unwrap();

    // Plan: MATCH (n:Person {name: 'Alice'}) RETURN n
    let planner = QueryPlanner::new(&store, "g");
    let query = Query {
        unions: Vec::new(),
        clauses: vec![
            Clause::Match(MatchClause {
                patterns: vec![PathPattern {
                    variable: None,
                    start: NodePattern {
                        variable: Some(Identifier::new("n")),
                        labels: vec![Label::new("Person")],
                        properties: Some(Properties::Map(vec![(
                            smol_str::SmolStr::new("name"),
                            Expr::Str("Alice".into()),
                        )])),
                        where_clause: None,
                    },
                    steps: vec![],
                    shortest_path: None,
                }],
                where_clause: None,
            }),
            Clause::Return(ReturnClause {
                distinct: false,
                items: vec![ReturnItem {
                    expr: Expr::Variable(Identifier::new("n")),
                    alias: None,
                }],
                order_by: vec![],
                skip: None,
                limit: None,
            }),
        ],
    };
    let plan = planner.plan(&query).unwrap();

    // Verify planner chose IndexScan.
    assert!(
        plan_contains_index_scan(&plan),
        "Planner should choose IndexScan when index exists, got: {plan:?}"
    );

    // Execute the plan and verify correct results.
    let executor = QueryExecutor::new(&store);
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1, "Should find exactly one Alice");
    assert_eq!(
        result.rows[0].get("n"),
        Some(&Value::Node(alice)),
        "Should return Alice's node"
    );
}
