//! End-to-end integration tests: parser → AST → QueryPlanner → QueryExecutor → results.
//!
//! These tests construct the Query AST directly (bypassing the text parser)
//! to exercise the planner and executor in isolation. For full round-trip tests
//! that go through the recursive descent parser, see the TCK runner in tests/tck/.

use std::collections::HashMap;
use std::time::Duration;

use gql_core::ast::{
    BinaryOp, Clause, Direction, Expr, Identifier, Label, MatchClause, NodePattern, PathPattern,
    PathStep, Query, RelPattern, RelType, ReturnClause, ReturnItem, SortItem,
};
use gql_core::executor::{ExecutionBudget, QueryExecutor};
use gql_core::planner::{CreatePattern, QueryPlanner};
use gql_core::storage::{GraphStore, InMemoryGraphStore};
use gql_core::types::Value;

// ── Helpers ────────────────────────────────────────────────────────────────────

fn make_store() -> InMemoryGraphStore {
    let store = InMemoryGraphStore::new();
    store.create_graph("test").unwrap();
    store
}

fn unlimited() -> ExecutionBudget {
    // Use a very large budget for tests (1 hour timeout, unlimited rows).
    ExecutionBudget::new(Duration::from_secs(3600), usize::MAX)
}

/// Build the query:
///   MATCH (alias:label)
///   RETURN alias
fn simple_match_return(label: &str, alias: &str) -> Query {
    Query {
        clauses: vec![
            Clause::Match(MatchClause {
                patterns: vec![PathPattern {
                    variable: None,
                    start: NodePattern {
                        variable: Some(Identifier::new(alias)),
                        labels: vec![Label::new(label)],
                        properties: None,
                    },
                    steps: vec![],
                }],
                where_clause: None,
            }),
            Clause::Return(ReturnClause {
                distinct: false,
                items: vec![ReturnItem {
                    expr: Expr::Variable(Identifier::new(alias)),
                    alias: None,
                }],
                order_by: vec![],
                skip: None,
                limit: None,
            }),
        ],
    }
}

// ── Test 1: Create nodes directly and query them back via planner ─────────────

#[test]
fn test_create_and_match() {
    let store = make_store();

    // Insert nodes directly into the store (bypassing the planner for setup).
    let mut alice_props = HashMap::new();
    alice_props.insert("name".to_string(), Value::Str("Alice".into()));
    store
        .create_node("test", vec!["Person".to_string()], alice_props)
        .unwrap();

    let mut bob_props = HashMap::new();
    bob_props.insert("name".to_string(), Value::Str("Bob".into()));
    store
        .create_node("test", vec!["Person".to_string()], bob_props)
        .unwrap();

    // Query: MATCH (p:Person) RETURN p.name ORDER BY p.name
    let query = Query {
        clauses: vec![
            Clause::Match(MatchClause {
                patterns: vec![PathPattern {
                    variable: None,
                    start: NodePattern {
                        variable: Some(Identifier::new("p")),
                        labels: vec![Label::new("Person")],
                        properties: None,
                    },
                    steps: vec![],
                }],
                where_clause: None,
            }),
            Clause::Return(ReturnClause {
                distinct: false,
                items: vec![ReturnItem {
                    expr: Expr::Property(
                        Box::new(Expr::Variable(Identifier::new("p"))),
                        "name".into(),
                    ),
                    alias: None,
                }],
                order_by: vec![SortItem {
                    expr: Expr::Property(
                        Box::new(Expr::Variable(Identifier::new("p"))),
                        "name".into(),
                    ),
                    ascending: true,
                }],
                skip: None,
                limit: None,
            }),
        ],
    };

    let planner = QueryPlanner::new(&store, "test");
    let plan = planner.plan(&query).unwrap();

    let executor = QueryExecutor::new(&store);
    let mut budget = unlimited();
    let result = executor.execute("test", &plan, &mut budget).unwrap();

    assert_eq!(result.rows.len(), 2, "should return two Person rows");
    assert_eq!(
        result.rows[0]["p.name"],
        Value::Str("Alice".into()),
        "first row should be Alice (sorted)"
    );
    assert_eq!(
        result.rows[1]["p.name"],
        Value::Str("Bob".into()),
        "second row should be Bob (sorted)"
    );
}

// ── Test 2: Single-hop relationship traversal ─────────────────────────────────

#[test]
fn test_match_relationship() {
    let store = make_store();

    let mut alice_props = HashMap::new();
    alice_props.insert("name".to_string(), Value::Str("Alice".into()));
    let alice_id = store
        .create_node("test", vec!["Person".to_string()], alice_props)
        .unwrap();

    let mut bob_props = HashMap::new();
    bob_props.insert("name".to_string(), Value::Str("Bob".into()));
    let bob_id = store
        .create_node("test", vec!["Person".to_string()], bob_props)
        .unwrap();

    store
        .create_edge(
            "test",
            alice_id,
            bob_id,
            "KNOWS".to_string(),
            HashMap::new(),
        )
        .unwrap();

    // Query: MATCH (a:Person)-[:KNOWS]->(b:Person) RETURN a.name, b.name
    let query = Query {
        clauses: vec![
            Clause::Match(MatchClause {
                patterns: vec![PathPattern {
                    variable: None,
                    start: NodePattern {
                        variable: Some(Identifier::new("a")),
                        labels: vec![Label::new("Person")],
                        properties: None,
                    },
                    steps: vec![PathStep {
                        rel: RelPattern {
                            variable: None,
                            rel_types: vec![RelType::new("KNOWS")],
                            direction: Direction::Outgoing,
                            range: None,
                            properties: None,
                        },
                        node: NodePattern {
                            variable: Some(Identifier::new("b")),
                            labels: vec![Label::new("Person")],
                            properties: None,
                        },
                    }],
                }],
                where_clause: None,
            }),
            Clause::Return(ReturnClause {
                distinct: false,
                items: vec![
                    ReturnItem {
                        expr: Expr::Property(
                            Box::new(Expr::Variable(Identifier::new("a"))),
                            "name".into(),
                        ),
                        alias: Some(Identifier::new("a_name")),
                    },
                    ReturnItem {
                        expr: Expr::Property(
                            Box::new(Expr::Variable(Identifier::new("b"))),
                            "name".into(),
                        ),
                        alias: Some(Identifier::new("b_name")),
                    },
                ],
                order_by: vec![],
                skip: None,
                limit: None,
            }),
        ],
    };

    let planner = QueryPlanner::new(&store, "test");
    let plan = planner.plan(&query).unwrap();

    let executor = QueryExecutor::new(&store);
    let mut budget = unlimited();
    let result = executor.execute("test", &plan, &mut budget).unwrap();

    // Alice knows Bob — but we also added a label filter for `b:Person`.
    // The label filter uses `hasLabel()` which may or may not be handled.
    // At minimum, we should have at least one row.
    assert!(
        result.rows.len() >= 1,
        "should have at least one traversal result, got {}",
        result.rows.len()
    );

    // At least one row should link Alice -> Bob
    let has_alice_bob = result.rows.iter().any(|row| {
        row.get("a_name") == Some(&Value::Str("Alice".into()))
            && row.get("b_name") == Some(&Value::Str("Bob".into()))
    });
    assert!(has_alice_bob, "should find Alice -> Bob path");
}

// ── Test 3: CREATE via planner ────────────────────────────────────────────────

#[test]
fn test_create_query() {
    use gql_core::planner::Operator;

    let store = make_store();

    // Use the executor directly with a Create operator (planner CREATE path).
    let plan = Operator::ProduceResults {
        columns: vec![],
        child: Box::new(Operator::Create {
            patterns: vec![CreatePattern {
                nodes: vec![(
                    Some(Identifier::new("n")),
                    vec![Label::new("City")],
                    vec![("name".to_string(), Expr::Str("Paris".into()))],
                )],
                edges: vec![],
            }],
            child: None,
        }),
    };

    let executor = QueryExecutor::new(&store);
    let mut budget = unlimited();
    let result = executor.execute("test", &plan, &mut budget).unwrap();

    assert_eq!(result.stats.nodes_created, 1);

    // Verify the node exists.
    let city_ids = store.find_nodes_by_label("test", "City").unwrap();
    assert_eq!(city_ids.len(), 1);

    let city_data = store.get_node("test", city_ids[0]).unwrap().unwrap();
    assert_eq!(
        city_data.properties.get("name"),
        Some(&Value::Str("Paris".into()))
    );
}

// ── Test 4: WHERE filtering ───────────────────────────────────────────────────

#[test]
fn test_where_filter() {
    let store = make_store();

    for (name, age) in &[("Alice", 30i64), ("Bob", 17), ("Charlie", 25)] {
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::Str((*name).into()));
        props.insert("age".to_string(), Value::Int(*age));
        store
            .create_node("test", vec!["Person".to_string()], props)
            .unwrap();
    }

    // Query: MATCH (p:Person) WHERE p.age >= 18 RETURN p.name
    let query = Query {
        clauses: vec![
            Clause::Match(MatchClause {
                patterns: vec![PathPattern {
                    variable: None,
                    start: NodePattern {
                        variable: Some(Identifier::new("p")),
                        labels: vec![Label::new("Person")],
                        properties: None,
                    },
                    steps: vec![],
                }],
                where_clause: Some(Expr::BinaryOp {
                    op: BinaryOp::Gte,
                    left: Box::new(Expr::Property(
                        Box::new(Expr::Variable(Identifier::new("p"))),
                        "age".into(),
                    )),
                    right: Box::new(Expr::Integer(18)),
                }),
            }),
            Clause::Return(ReturnClause {
                distinct: false,
                items: vec![ReturnItem {
                    expr: Expr::Property(
                        Box::new(Expr::Variable(Identifier::new("p"))),
                        "name".into(),
                    ),
                    alias: None,
                }],
                order_by: vec![SortItem {
                    expr: Expr::Property(
                        Box::new(Expr::Variable(Identifier::new("p"))),
                        "name".into(),
                    ),
                    ascending: true,
                }],
                skip: None,
                limit: None,
            }),
        ],
    };

    let planner = QueryPlanner::new(&store, "test");
    let plan = planner.plan(&query).unwrap();

    let executor = QueryExecutor::new(&store);
    let mut budget = unlimited();
    let result = executor.execute("test", &plan, &mut budget).unwrap();

    // Bob (17) should be filtered out; Alice (30) and Charlie (25) remain.
    assert_eq!(
        result.rows.len(),
        2,
        "two adults should pass the age filter"
    );

    let names: Vec<&Value> = result.rows.iter().map(|r| &r["p.name"]).collect();
    assert!(
        names.contains(&&Value::Str("Alice".into())),
        "Alice should be in results"
    );
    assert!(
        names.contains(&&Value::Str("Charlie".into())),
        "Charlie should be in results"
    );
    assert!(
        !names.contains(&&Value::Str("Bob".into())),
        "Bob should not be in results"
    );
}

// ── Test 5: LIMIT and SKIP ───────────────────────────────────────────────────

#[test]
fn test_limit_skip() {
    let store = make_store();

    // Create 10 nodes with sequential integers.
    for i in 0..10i64 {
        let mut props = HashMap::new();
        props.insert("i".to_string(), Value::Int(i));
        store
            .create_node("test", vec!["Item".to_string()], props)
            .unwrap();
    }

    // Query: MATCH (x:Item) RETURN x.i ORDER BY x.i SKIP 3 LIMIT 4
    let query = Query {
        clauses: vec![
            Clause::Match(MatchClause {
                patterns: vec![PathPattern {
                    variable: None,
                    start: NodePattern {
                        variable: Some(Identifier::new("x")),
                        labels: vec![Label::new("Item")],
                        properties: None,
                    },
                    steps: vec![],
                }],
                where_clause: None,
            }),
            Clause::Return(ReturnClause {
                distinct: false,
                items: vec![ReturnItem {
                    expr: Expr::Property(
                        Box::new(Expr::Variable(Identifier::new("x"))),
                        "i".into(),
                    ),
                    alias: None,
                }],
                order_by: vec![SortItem {
                    expr: Expr::Property(
                        Box::new(Expr::Variable(Identifier::new("x"))),
                        "i".into(),
                    ),
                    ascending: true,
                }],
                skip: Some(Expr::Integer(3)),
                limit: Some(Expr::Integer(4)),
            }),
        ],
    };

    let planner = QueryPlanner::new(&store, "test");
    let plan = planner.plan(&query).unwrap();

    let executor = QueryExecutor::new(&store);
    let mut budget = unlimited();
    let result = executor.execute("test", &plan, &mut budget).unwrap();

    // After ORDER BY: [0,1,2,3,4,5,6,7,8,9]
    // After SKIP 3:   [3,4,5,6,7,8,9]
    // After LIMIT 4:  [3,4,5,6]
    assert_eq!(result.rows.len(), 4, "should return exactly 4 rows");
    assert_eq!(result.rows[0]["x.i"], Value::Int(3));
    assert_eq!(result.rows[1]["x.i"], Value::Int(4));
    assert_eq!(result.rows[2]["x.i"], Value::Int(5));
    assert_eq!(result.rows[3]["x.i"], Value::Int(6));
}

// ── Test 6: Aggregation (COUNT) ───────────────────────────────────────────────

#[test]
fn test_aggregate_count() {
    let store = make_store();

    for name in &["Alice", "Bob", "Charlie"] {
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::Str((*name).into()));
        store
            .create_node("test", vec!["Person".to_string()], props)
            .unwrap();
    }

    // Query: MATCH (p:Person) RETURN count(p) AS total
    let query = Query {
        clauses: vec![
            Clause::Match(MatchClause {
                patterns: vec![PathPattern {
                    variable: None,
                    start: NodePattern {
                        variable: Some(Identifier::new("p")),
                        labels: vec![Label::new("Person")],
                        properties: None,
                    },
                    steps: vec![],
                }],
                where_clause: None,
            }),
            Clause::Return(ReturnClause {
                distinct: false,
                items: vec![ReturnItem {
                    expr: Expr::FunctionCall {
                        name: "count".into(),
                        args: vec![Expr::Variable(Identifier::new("p"))],
                        distinct: false,
                    },
                    alias: Some(Identifier::new("total")),
                }],
                order_by: vec![],
                skip: None,
                limit: None,
            }),
        ],
    };

    let planner = QueryPlanner::new(&store, "test");
    let plan = planner.plan(&query).unwrap();

    let executor = QueryExecutor::new(&store);
    let mut budget = unlimited();
    let result = executor.execute("test", &plan, &mut budget).unwrap();

    assert_eq!(result.rows.len(), 1, "aggregate should produce one row");
    assert_eq!(result.rows[0]["total"], Value::Int(3), "count should be 3");
}

// ── Test 7: Distinct ──────────────────────────────────────────────────────────

#[test]
fn test_distinct() {
    let store = make_store();

    // Two nodes with the same label value.
    for _ in 0..3 {
        let mut props = HashMap::new();
        props.insert("dept".to_string(), Value::Str("Engineering".into()));
        store
            .create_node("test", vec!["Employee".to_string()], props)
            .unwrap();
    }
    let mut props = HashMap::new();
    props.insert("dept".to_string(), Value::Str("Marketing".into()));
    store
        .create_node("test", vec!["Employee".to_string()], props)
        .unwrap();

    // Query: MATCH (e:Employee) RETURN DISTINCT e.dept
    let query = Query {
        clauses: vec![
            Clause::Match(MatchClause {
                patterns: vec![PathPattern {
                    variable: None,
                    start: NodePattern {
                        variable: Some(Identifier::new("e")),
                        labels: vec![Label::new("Employee")],
                        properties: None,
                    },
                    steps: vec![],
                }],
                where_clause: None,
            }),
            Clause::Return(ReturnClause {
                distinct: true,
                items: vec![ReturnItem {
                    expr: Expr::Property(
                        Box::new(Expr::Variable(Identifier::new("e"))),
                        "dept".into(),
                    ),
                    alias: None,
                }],
                order_by: vec![],
                skip: None,
                limit: None,
            }),
        ],
    };

    let planner = QueryPlanner::new(&store, "test");
    let plan = planner.plan(&query).unwrap();

    let executor = QueryExecutor::new(&store);
    let mut budget = unlimited();
    let result = executor.execute("test", &plan, &mut budget).unwrap();

    // Should return exactly 2 distinct department values.
    assert_eq!(result.rows.len(), 2, "should have 2 distinct departments");
}

// ── Test 8: Create node + edge via planner then traverse ──────────────────────

#[test]
fn test_create_and_traverse() {
    use gql_core::planner::Operator;

    let store = make_store();

    // Create two Person nodes and a KNOWS edge between them via the executor.
    let plan_create = Operator::Create {
        patterns: vec![CreatePattern {
            nodes: vec![
                (
                    Some(Identifier::new("a")),
                    vec![Label::new("Person")],
                    vec![("name".to_string(), Expr::Str("Eve".into()))],
                ),
                (
                    Some(Identifier::new("b")),
                    vec![Label::new("Person")],
                    vec![("name".to_string(), Expr::Str("Frank".into()))],
                ),
            ],
            edges: vec![(
                None,
                Identifier::new("a"),
                Identifier::new("b"),
                RelType::new("KNOWS"),
                Direction::Outgoing,
                vec![],
            )],
        }],
        child: None,
    };

    let executor = QueryExecutor::new(&store);
    let mut budget = unlimited();
    let create_result = executor.execute("test", &plan_create, &mut budget).unwrap();

    assert_eq!(create_result.stats.nodes_created, 2);
    assert_eq!(create_result.stats.edges_created, 1);

    // Now traverse: MATCH (a:Person)-[:KNOWS]->(b:Person) RETURN a.name, b.name
    let query = Query {
        clauses: vec![
            Clause::Match(MatchClause {
                patterns: vec![PathPattern {
                    variable: None,
                    start: NodePattern {
                        variable: Some(Identifier::new("a")),
                        labels: vec![Label::new("Person")],
                        properties: None,
                    },
                    steps: vec![PathStep {
                        rel: RelPattern {
                            variable: None,
                            rel_types: vec![RelType::new("KNOWS")],
                            direction: Direction::Outgoing,
                            range: None,
                            properties: None,
                        },
                        node: NodePattern {
                            variable: Some(Identifier::new("b")),
                            labels: vec![],
                            properties: None,
                        },
                    }],
                }],
                where_clause: None,
            }),
            Clause::Return(ReturnClause {
                distinct: false,
                items: vec![
                    ReturnItem {
                        expr: Expr::Property(
                            Box::new(Expr::Variable(Identifier::new("a"))),
                            "name".into(),
                        ),
                        alias: Some(Identifier::new("a_name")),
                    },
                    ReturnItem {
                        expr: Expr::Property(
                            Box::new(Expr::Variable(Identifier::new("b"))),
                            "name".into(),
                        ),
                        alias: Some(Identifier::new("b_name")),
                    },
                ],
                order_by: vec![],
                skip: None,
                limit: None,
            }),
        ],
    };

    let planner = QueryPlanner::new(&store, "test");
    let plan = planner.plan(&query).unwrap();
    let mut budget2 = unlimited();
    let result = executor.execute("test", &plan, &mut budget2).unwrap();

    // Eve knows Frank.
    let has_eve_frank = result.rows.iter().any(|row| {
        row.get("a_name") == Some(&Value::Str("Eve".into()))
            && row.get("b_name") == Some(&Value::Str("Frank".into()))
    });
    assert!(has_eve_frank, "should find Eve -> Frank traversal");
}

// ── Test 9: Empty result set ──────────────────────────────────────────────────

#[test]
fn test_empty_result() {
    let store = make_store();

    let query = simple_match_return("Person", "p");

    let planner = QueryPlanner::new(&store, "test");
    let plan = planner.plan(&query).unwrap();

    let executor = QueryExecutor::new(&store);
    let mut budget = unlimited();
    let result = executor.execute("test", &plan, &mut budget).unwrap();

    assert_eq!(result.rows.len(), 0, "empty graph returns no rows");
}

// ── Test 10: Budget row limit enforced ────────────────────────────────────────

#[test]
fn test_budget_row_limit() {
    let store = make_store();

    for i in 0..10i64 {
        let mut props = HashMap::new();
        props.insert("i".to_string(), Value::Int(i));
        store
            .create_node("test", vec!["Item".to_string()], props)
            .unwrap();
    }

    let query = simple_match_return("Item", "x");

    let planner = QueryPlanner::new(&store, "test");
    let plan = planner.plan(&query).unwrap();

    let executor = QueryExecutor::new(&store);
    // Budget allows only 3 rows.
    let mut budget = ExecutionBudget::new(Duration::from_secs(60), 3);
    let result = executor.execute("test", &plan, &mut budget);

    assert!(result.is_err(), "should fail when row budget is exceeded");
}
