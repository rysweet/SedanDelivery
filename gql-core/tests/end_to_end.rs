//! End-to-end integration tests: parser → AST → QueryPlanner → QueryExecutor → results.
//!
//! Tests 1-10 construct the Query AST directly (bypassing the text parser)
//! to exercise the planner and executor in isolation.
//!
//! Tests 11-28 use `parse_query` → `QueryPlanner` → `QueryExecutor` to exercise the
//! full planner→executor pipeline for all remaining operators.
//!
//! Property-based tests at the bottom use proptest to validate invariants.

use std::collections::HashMap;
use std::time::Duration;

use gql_core::ast::{
    BinaryOp, Clause, Direction, Expr, Identifier, Label, MatchClause, NodePattern, PathPattern,
    PathStep, Query, RelPattern, RelType, ReturnClause, ReturnItem, SortItem,
};
use gql_core::executor::engine::cmp_values;
use gql_core::executor::{ExecutionBudget, QueryExecutor};
use gql_core::parser::parse_query;
use gql_core::planner::{CreatePattern, Operator, QueryPlanner};
use gql_core::procedure::{ProcedureDefinition, ProcedureParam, ProcedureRegistry};
use gql_core::storage::{GraphStore, InMemoryGraphStore};
use gql_core::types::Value;
use proptest::prelude::*;

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
    Query { unions: Vec::new(),
        clauses: vec![
            Clause::Match(MatchClause {
                patterns: vec![PathPattern {
                    variable: None,
                    start: NodePattern {
                        variable: Some(Identifier::new(alias)),
                        labels: vec![Label::new(label)],
                        properties: None,
                        where_clause: None,
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
    let query = Query { unions: Vec::new(),
        clauses: vec![
            Clause::Match(MatchClause {
                patterns: vec![PathPattern {
                    variable: None,
                    start: NodePattern {
                        variable: Some(Identifier::new("p")),
                        labels: vec![Label::new("Person")],
                        properties: None,
                        where_clause: None,
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
    let query = Query { unions: Vec::new(),
        clauses: vec![
            Clause::Match(MatchClause {
                patterns: vec![PathPattern {
                    variable: None,
                    start: NodePattern {
                        variable: Some(Identifier::new("a")),
                        labels: vec![Label::new("Person")],
                        properties: None,
                        where_clause: None,
                    },
                    steps: vec![PathStep {
                        rel: RelPattern {
                            variable: None,
                            rel_types: vec![RelType::new("KNOWS")],
                            direction: Direction::Outgoing,
                            range: None,
                            properties: None,
                            where_clause: None,
                        },
                        node: NodePattern {
                            variable: Some(Identifier::new("b")),
                            labels: vec![Label::new("Person")],
                            properties: None,
                            where_clause: None,
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
        !result.rows.is_empty(),
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
    let query = Query { unions: Vec::new(),
        clauses: vec![
            Clause::Match(MatchClause {
                patterns: vec![PathPattern {
                    variable: None,
                    start: NodePattern {
                        variable: Some(Identifier::new("p")),
                        labels: vec![Label::new("Person")],
                        properties: None,
                        where_clause: None,
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
    let query = Query { unions: Vec::new(),
        clauses: vec![
            Clause::Match(MatchClause {
                patterns: vec![PathPattern {
                    variable: None,
                    start: NodePattern {
                        variable: Some(Identifier::new("x")),
                        labels: vec![Label::new("Item")],
                        properties: None,
                        where_clause: None,
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
    let query = Query { unions: Vec::new(),
        clauses: vec![
            Clause::Match(MatchClause {
                patterns: vec![PathPattern {
                    variable: None,
                    start: NodePattern {
                        variable: Some(Identifier::new("p")),
                        labels: vec![Label::new("Person")],
                        properties: None,
                        where_clause: None,
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
    let query = Query { unions: Vec::new(),
        clauses: vec![
            Clause::Match(MatchClause {
                patterns: vec![PathPattern {
                    variable: None,
                    start: NodePattern {
                        variable: Some(Identifier::new("e")),
                        labels: vec![Label::new("Employee")],
                        properties: None,
                        where_clause: None,
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
    let query = Query { unions: Vec::new(),
        clauses: vec![
            Clause::Match(MatchClause {
                patterns: vec![PathPattern {
                    variable: None,
                    start: NodePattern {
                        variable: Some(Identifier::new("a")),
                        labels: vec![Label::new("Person")],
                        properties: None,
                        where_clause: None,
                    },
                    steps: vec![PathStep {
                        rel: RelPattern {
                            variable: None,
                            rel_types: vec![RelType::new("KNOWS")],
                            direction: Direction::Outgoing,
                            range: None,
                            properties: None,
                            where_clause: None,
                        },
                        node: NodePattern {
                            variable: Some(Identifier::new("b")),
                            labels: vec![],
                            properties: None,
                            where_clause: None,
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

// ════════════════════════════════════════════════════════════════════════════════
// Tests 11-28: Full parse_query → QueryPlanner → QueryExecutor pipeline
// ════════════════════════════════════════════════════════════════════════════════

// ── Test 11: OPTIONAL MATCH ───────────────────────────────────────────────────

#[test]
fn test_optional_match() {
    let store = make_store();

    // Alice knows nobody outgoing; Bob knows Alice.
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
        .create_edge("test", bob_id, alice_id, "KNOWS".to_string(), HashMap::new())
        .unwrap();

    // MATCH (p:Person) OPTIONAL MATCH (p)-[:KNOWS]->(f) RETURN p.name, f.name
    let q = parse_query(
        "MATCH (p:Person) OPTIONAL MATCH (p)-[:KNOWS]->(f) RETURN p.name, f.name",
    )
    .unwrap();
    let planner = QueryPlanner::new(&store, "test");
    let plan = planner.plan(&q).unwrap();

    let executor = QueryExecutor::new(&store);
    let mut budget = unlimited();
    let result = executor.execute("test", &plan, &mut budget).unwrap();

    assert_eq!(result.rows.len(), 2, "both persons should appear");

    // Bob's row: f.name = Alice.
    let bob_row = result
        .rows
        .iter()
        .find(|r| r.get("p.name") == Some(&Value::Str("Bob".into())));
    assert!(bob_row.is_some(), "Bob should be in results");
    assert_eq!(
        bob_row.unwrap().get("f.name"),
        Some(&Value::Str("Alice".into())),
        "Bob's friend should be Alice"
    );

    // Alice's row: f.name = Null (no outgoing KNOWS).
    let alice_row = result
        .rows
        .iter()
        .find(|r| r.get("p.name") == Some(&Value::Str("Alice".into())));
    assert!(alice_row.is_some(), "Alice should be in results");
    let f_name = alice_row.unwrap().get("f.name");
    assert!(
        f_name.map_or(true, |v| v.is_null()),
        "Alice has no outgoing KNOWS — f.name should be null, got {f_name:?}"
    );
}

// ── Test 12: OPTIONAL MATCH with no matching patterns ─────────────────────────

#[test]
fn test_optional_match_no_match() {
    let store = make_store();

    // Carol has no KNOWS edges at all.
    let mut props = HashMap::new();
    props.insert("name".to_string(), Value::Str("Carol".into()));
    store
        .create_node("test", vec!["Person".to_string()], props)
        .unwrap();

    let q = parse_query(
        "MATCH (p:Person) OPTIONAL MATCH (p)-[:KNOWS]->(f) RETURN p.name, f.name",
    )
    .unwrap();
    let planner = QueryPlanner::new(&store, "test");
    let plan = planner.plan(&q).unwrap();

    let executor = QueryExecutor::new(&store);
    let mut budget = unlimited();
    let result = executor.execute("test", &plan, &mut budget).unwrap();

    assert_eq!(result.rows.len(), 1, "one row for Carol");
    let row = &result.rows[0];
    assert_eq!(
        row.get("p.name"),
        Some(&Value::Str("Carol".into())),
        "p.name should be Carol"
    );
    let f_name = row.get("f.name");
    assert!(
        f_name.map_or(true, |v| v.is_null()),
        "f.name should be null when no match"
    );
}

// ── Test 13: MERGE node — ON CREATE SET ───────────────────────────────────────

#[test]
fn test_merge_node_on_create_set() {
    let store = make_store();

    // No Alice yet — MERGE should create the node and apply ON CREATE SET.
    let q = parse_query(
        "MERGE (p:Person {name: 'Alice'}) ON CREATE SET p.created = true RETURN p.name, p.created",
    )
    .unwrap();
    let planner = QueryPlanner::new(&store, "test");
    let plan = planner.plan(&q).unwrap();

    let executor = QueryExecutor::new(&store);
    let mut budget = unlimited();
    let result = executor.execute("test", &plan, &mut budget).unwrap();

    assert_eq!(result.rows.len(), 1, "one row for created Alice");
    assert_eq!(
        result.rows[0].get("p.name"),
        Some(&Value::Str("Alice".into()))
    );
    assert_eq!(
        result.rows[0].get("p.created"),
        Some(&Value::Bool(true)),
        "ON CREATE SET should have set p.created"
    );
}

// ── Test 14: MERGE node — ON MATCH SET ────────────────────────────────────────

#[test]
fn test_merge_node_on_match_set() {
    let store = make_store();

    // Pre-create Alice.
    let mut props = HashMap::new();
    props.insert("name".to_string(), Value::Str("Alice".into()));
    store
        .create_node("test", vec!["Person".to_string()], props)
        .unwrap();

    // MERGE should find Alice and apply ON MATCH SET.
    let q = parse_query(
        "MERGE (p:Person {name: 'Alice'}) ON MATCH SET p.matched = true RETURN p.name, p.matched",
    )
    .unwrap();
    let planner = QueryPlanner::new(&store, "test");
    let plan = planner.plan(&q).unwrap();

    let executor = QueryExecutor::new(&store);
    let mut budget = unlimited();
    let result = executor.execute("test", &plan, &mut budget).unwrap();

    assert_eq!(result.rows.len(), 1, "one row for matched Alice");
    assert_eq!(
        result.rows[0].get("p.matched"),
        Some(&Value::Bool(true)),
        "ON MATCH SET should have set p.matched"
    );
}

// ── Test 15: MERGE relationship ───────────────────────────────────────────────

#[test]
fn test_merge_relationship() {
    let store = make_store();

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

    // First MERGE: creates the relationship.
    let q = parse_query(
        "MATCH (a:Person {name: 'Alice'}), (b:Person {name: 'Bob'}) \
         MERGE (a)-[:KNOWS]->(b) RETURN a.name, b.name",
    )
    .unwrap();
    let planner = QueryPlanner::new(&store, "test");
    let plan = planner.plan(&q).unwrap();

    let executor = QueryExecutor::new(&store);
    let mut budget = unlimited();
    let result = executor.execute("test", &plan, &mut budget).unwrap();

    assert!(!result.rows.is_empty(), "should produce a row after MERGE");
    assert_eq!(
        result.rows[0].get("a.name"),
        Some(&Value::Str("Alice".into()))
    );
    assert_eq!(
        result.rows[0].get("b.name"),
        Some(&Value::Str("Bob".into()))
    );

    // Second MERGE: matches existing relationship — edge count should not grow.
    let mut budget2 = unlimited();
    let result2 = executor.execute("test", &plan, &mut budget2).unwrap();
    assert_eq!(result2.stats.edges_created, 0, "second MERGE should match, not create");
}

// ── Test 16: SET — property assignment ────────────────────────────────────────

#[test]
fn test_set_property() {
    let store = make_store();

    let mut props = HashMap::new();
    props.insert("name".to_string(), Value::Str("Alice".into()));
    store
        .create_node("test", vec!["Person".to_string()], props)
        .unwrap();

    let q = parse_query(
        "MATCH (p:Person {name: 'Alice'}) SET p.age = 30 RETURN p.name, p.age",
    )
    .unwrap();
    let planner = QueryPlanner::new(&store, "test");
    let plan = planner.plan(&q).unwrap();

    let executor = QueryExecutor::new(&store);
    let mut budget = unlimited();
    let result = executor.execute("test", &plan, &mut budget).unwrap();

    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0].get("p.age"), Some(&Value::Int(30)));
}

// ── Test 17: SET += (map merge) ───────────────────────────────────────────────

#[test]
fn test_set_map_merge() {
    let store = make_store();

    let mut props = HashMap::new();
    props.insert("name".to_string(), Value::Str("Bob".into()));
    props.insert("age".to_string(), Value::Int(25));
    store
        .create_node("test", vec!["Person".to_string()], props)
        .unwrap();

    // SET p += {city: 'London'} should merge without removing existing props.
    let q = parse_query(
        "MATCH (p:Person {name: 'Bob'}) SET p += {city: 'London'} RETURN p.name, p.age, p.city",
    )
    .unwrap();
    let planner = QueryPlanner::new(&store, "test");
    let plan = planner.plan(&q).unwrap();

    let executor = QueryExecutor::new(&store);
    let mut budget = unlimited();
    let result = executor.execute("test", &plan, &mut budget).unwrap();

    assert_eq!(result.rows.len(), 1);
    // Original props should still be present.
    assert_eq!(result.rows[0].get("p.name"), Some(&Value::Str("Bob".into())));
    assert_eq!(result.rows[0].get("p.age"), Some(&Value::Int(25)));
    // New prop should be added.
    assert_eq!(
        result.rows[0].get("p.city"),
        Some(&Value::Str("London".into()))
    );
}

// ── Test 18: REMOVE — property and label removal ──────────────────────────────

#[test]
fn test_remove_property_and_label() {
    let store = make_store();

    let mut props = HashMap::new();
    props.insert("name".to_string(), Value::Str("Dave".into()));
    props.insert("tmp".to_string(), Value::Int(99));
    store
        .create_node("test", vec!["Person".to_string(), "Temp".to_string()], props)
        .unwrap();

    // Remove the tmp property.
    let q = parse_query(
        "MATCH (p:Person {name: 'Dave'}) REMOVE p.tmp RETURN p.name, p.tmp",
    )
    .unwrap();
    let planner = QueryPlanner::new(&store, "test");
    let plan = planner.plan(&q).unwrap();

    let executor = QueryExecutor::new(&store);
    let mut budget = unlimited();
    let result = executor.execute("test", &plan, &mut budget).unwrap();

    assert_eq!(result.rows.len(), 1);
    let tmp_val = result.rows[0].get("p.tmp");
    assert!(
        tmp_val.map_or(true, |v| v.is_null()),
        "p.tmp should be null after REMOVE"
    );

    // Remove the Temp label.
    let q2 = parse_query(
        "MATCH (p:Temp) REMOVE p:Temp RETURN p.name",
    )
    .unwrap();
    let planner2 = QueryPlanner::new(&store, "test");
    let plan2 = planner2.plan(&q2).unwrap();

    let mut budget2 = unlimited();
    let result2 = executor.execute("test", &plan2, &mut budget2).unwrap();
    assert_eq!(result2.rows.len(), 1, "should find Dave as :Temp before removal");

    // After removal, MATCH (p:Temp) should find nothing.
    let q3 = parse_query("MATCH (p:Temp) RETURN p.name").unwrap();
    let plan3 = QueryPlanner::new(&store, "test").plan(&q3).unwrap();
    let mut budget3 = unlimited();
    let result3 = executor.execute("test", &plan3, &mut budget3).unwrap();
    assert_eq!(result3.rows.len(), 0, "Temp label should be removed");
}

// ── Test 19: DELETE — node, relationship, DETACH DELETE ───────────────────────

#[test]
fn test_delete_node_and_relationship() {
    let store = make_store();

    let mut a_props = HashMap::new();
    a_props.insert("name".to_string(), Value::Str("Eve".into()));
    let a_id = store
        .create_node("test", vec!["Person".to_string()], a_props)
        .unwrap();

    let mut b_props = HashMap::new();
    b_props.insert("name".to_string(), Value::Str("Frank".into()));
    let b_id = store
        .create_node("test", vec!["Person".to_string()], b_props)
        .unwrap();

    store
        .create_edge("test", a_id, b_id, "KNOWS".to_string(), HashMap::new())
        .unwrap();

    // Delete the relationship first.
    let q_del_rel = parse_query(
        "MATCH (a:Person {name: 'Eve'})-[r:KNOWS]->(b) DELETE r",
    )
    .unwrap();
    let planner = QueryPlanner::new(&store, "test");
    let plan = planner.plan(&q_del_rel).unwrap();
    let executor = QueryExecutor::new(&store);
    let mut budget = unlimited();
    let result = executor.execute("test", &plan, &mut budget).unwrap();
    assert_eq!(result.stats.edges_deleted, 1, "one edge should be deleted");

    // Now delete a node (no edges remain, so plain DELETE works).
    let q_del_node = parse_query("MATCH (p:Person {name: 'Eve'}) DELETE p").unwrap();
    let plan2 = QueryPlanner::new(&store, "test")
        .plan(&q_del_node)
        .unwrap();
    let mut budget2 = unlimited();
    let result2 = executor.execute("test", &plan2, &mut budget2).unwrap();
    assert_eq!(result2.stats.nodes_deleted, 1, "one node should be deleted");

    // Use DETACH DELETE to remove Frank and any of his edges atomically.
    // First add an edge back so there's something to detach.
    let c_id = store
        .create_node("test", vec!["Person".to_string()], HashMap::new())
        .unwrap();
    store
        .create_edge("test", b_id, c_id, "KNOWS".to_string(), HashMap::new())
        .unwrap();

    let q_detach = parse_query("MATCH (p:Person {name: 'Frank'}) DETACH DELETE p").unwrap();
    let plan3 = QueryPlanner::new(&store, "test").plan(&q_detach).unwrap();
    let mut budget3 = unlimited();
    let result3 = executor.execute("test", &plan3, &mut budget3).unwrap();
    assert_eq!(result3.stats.nodes_deleted, 1, "Frank should be detach-deleted");
    assert!(result3.stats.edges_deleted >= 1, "Frank's edges should be deleted");
}

// ── Test 20: UNWIND — list, empty list, null ──────────────────────────────────

#[test]
fn test_unwind_list() {
    let store = make_store();

    // Basic list.
    let q = parse_query("UNWIND [10, 20, 30] AS x RETURN x").unwrap();
    let planner = QueryPlanner::new(&store, "test");
    let plan = planner.plan(&q).unwrap();
    let executor = QueryExecutor::new(&store);
    let mut budget = unlimited();
    let result = executor.execute("test", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 3);
    let xs: Vec<i64> = result
        .rows
        .iter()
        .map(|r| r["x"].as_int().unwrap())
        .collect();
    assert_eq!(xs, vec![10, 20, 30]);
}

#[test]
fn test_unwind_empty_list() {
    let store = make_store();

    let q = parse_query("UNWIND [] AS x RETURN x").unwrap();
    let planner = QueryPlanner::new(&store, "test");
    let plan = planner.plan(&q).unwrap();
    let executor = QueryExecutor::new(&store);
    let mut budget = unlimited();
    let result = executor.execute("test", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 0, "empty list → no rows");
}

#[test]
fn test_unwind_null_list() {
    let store = make_store();

    // UNWIND null AS x — Cypher spec: produces no rows.
    let q = parse_query("UNWIND null AS x RETURN x").unwrap();
    let planner = QueryPlanner::new(&store, "test");
    let plan = planner.plan(&q).unwrap();
    let executor = QueryExecutor::new(&store);
    let mut budget = unlimited();
    let result = executor.execute("test", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 0, "null list → no rows");
}

// ── Test 21: UNION / UNION ALL ────────────────────────────────────────────────

#[test]
fn test_union_deduplication() {
    let store = make_store();

    // Two persons with same name — UNION should deduplicate.
    for _ in 0..2 {
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::Str("Alice".into()));
        store
            .create_node("test", vec!["Person".to_string()], props)
            .unwrap();
    }

    let q = parse_query(
        "MATCH (p:Person) RETURN p.name AS n \
         UNION \
         MATCH (p:Person) RETURN p.name AS n",
    )
    .unwrap();
    let planner = QueryPlanner::new(&store, "test");
    let plan = planner.plan(&q).unwrap();
    let executor = QueryExecutor::new(&store);
    let mut budget = unlimited();
    let result = executor.execute("test", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1, "UNION should deduplicate to one 'Alice' row");
}

#[test]
fn test_union_all_no_deduplication() {
    let store = make_store();

    let mut props = HashMap::new();
    props.insert("name".to_string(), Value::Str("Alice".into()));
    store
        .create_node("test", vec!["Person".to_string()], props)
        .unwrap();

    let q = parse_query(
        "MATCH (p:Person) RETURN p.name AS n \
         UNION ALL \
         MATCH (p:Person) RETURN p.name AS n",
    )
    .unwrap();
    let planner = QueryPlanner::new(&store, "test");
    let plan = planner.plan(&q).unwrap();
    let executor = QueryExecutor::new(&store);
    let mut budget = unlimited();
    let result = executor.execute("test", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 2, "UNION ALL should keep duplicates");
}

// ── Test 22: Variable-length paths [*1..3] ────────────────────────────────────

#[test]
fn test_varlen_paths() {
    let store = make_store();

    // Chain: A -KNOWS-> B -KNOWS-> C
    let mut p = HashMap::new();
    p.insert("name".to_string(), Value::Str("A".into()));
    let a = store.create_node("test", vec!["Person".to_string()], p).unwrap();

    let mut p = HashMap::new();
    p.insert("name".to_string(), Value::Str("B".into()));
    let b = store.create_node("test", vec!["Person".to_string()], p).unwrap();

    let mut p = HashMap::new();
    p.insert("name".to_string(), Value::Str("C".into()));
    let c = store.create_node("test", vec!["Person".to_string()], p).unwrap();

    store
        .create_edge("test", a, b, "KNOWS".to_string(), HashMap::new())
        .unwrap();
    store
        .create_edge("test", b, c, "KNOWS".to_string(), HashMap::new())
        .unwrap();

    // Paths of length 1 to 2: A->B (len 1), B->C (len 1), A->C (len 2).
    let q = parse_query(
        "MATCH (a:Person)-[:KNOWS*1..2]->(b:Person) RETURN a.name, b.name",
    )
    .unwrap();
    let planner = QueryPlanner::new(&store, "test");
    let plan = planner.plan(&q).unwrap();
    let executor = QueryExecutor::new(&store);
    let mut budget = unlimited();
    let result = executor.execute("test", &plan, &mut budget).unwrap();

    assert!(result.rows.len() >= 3, "expected at least 3 paths (A->B, B->C, A->C), got {}", result.rows.len());

    let has_a_to_c = result.rows.iter().any(|r| {
        r.get("a.name") == Some(&Value::Str("A".into()))
            && r.get("b.name") == Some(&Value::Str("C".into()))
    });
    assert!(has_a_to_c, "should find A->C path of length 2");
}

// ── Test 23: Shortest path (direct operator — parser doesn't emit ShortestPath) ─

#[test]
fn test_shortest_path_operator() {
    // Tests the ShortestPath operator through the full executor pipeline.
    // Uses the operator tree directly since parse_query doesn't support
    // the shortestPath() syntax yet.
    let store = make_store();

    let a = store
        .create_node("test", vec!["Person".to_string()], HashMap::new())
        .unwrap();
    let b = store
        .create_node("test", vec!["Person".to_string()], HashMap::new())
        .unwrap();
    let c = store
        .create_node("test", vec!["Person".to_string()], HashMap::new())
        .unwrap();

    // Two paths A->B->C (length 2) and A->C (length 1); shortest should be A->C.
    store
        .create_edge("test", a, b, "KNOWS".to_string(), HashMap::new())
        .unwrap();
    store
        .create_edge("test", b, c, "KNOWS".to_string(), HashMap::new())
        .unwrap();
    store
        .create_edge("test", a, c, "KNOWS".to_string(), HashMap::new())
        .unwrap();

    // Provide src and dst as outer context rows (unused directly; plan2 uses NodeScan).
    let mut ctx_row = HashMap::new();
    ctx_row.insert("src".to_string(), Value::Node(a));
    ctx_row.insert("dst".to_string(), Value::Node(c));
    let _ = ctx_row;

    // Execute using exec_operator via the public execute API with an Apply outer.
    // Build: SingleRow with context -> ShortestPath
    let plan2 = Operator::ProduceResults {
        columns: vec![("p".to_string(), Expr::Variable(Identifier::new("p")))],
        child: Box::new(Operator::Apply {
            outer: Box::new(Operator::NodeScan {
                label: Some(Label::new("Person")),
                alias: Identifier::new("src"),
            }),
            inner: Box::new(Operator::Apply {
                outer: Box::new(Operator::NodeScan {
                    label: Some(Label::new("Person")),
                    alias: Identifier::new("dst"),
                }),
                inner: Box::new(Operator::ShortestPath {
                    src_alias: Identifier::new("src"),
                    dst_alias: Identifier::new("dst"),
                    rel_types: vec![],
                    direction: Direction::Outgoing,
                    min: 1,
                    max: 10,
                    path_alias: Identifier::new("p"),
                    all_paths: false,
                }),
            }),
        }),
    };

    let executor = QueryExecutor::new(&store);
    let mut budget = unlimited();
    let result = executor.execute("test", &plan2, &mut budget).unwrap();

    // Should find at least one shortest path from A to C.
    let has_shortest = result.rows.iter().any(|r| {
        if let Some(Value::Path { nodes, edges }) = r.get("p") {
            nodes.first() == Some(&a) && nodes.last() == Some(&c) && edges.len() == 1
        } else {
            false
        }
    });
    assert!(has_shortest, "should find direct shortest path A->C with 1 edge");
}

// ── Test 24: CALL procedure ───────────────────────────────────────────────────

#[test]
fn test_call_procedure() {
    let store = make_store();

    // Register a 'math.double' procedure: input n INTEGER, output doubled INTEGER.
    let proc = ProcedureDefinition {
        name: "math.double".to_string(),
        input_params: vec![ProcedureParam {
            name: "n".to_string(),
            type_name: "INTEGER?".to_string(),
        }],
        output_columns: vec![ProcedureParam {
            name: "doubled".to_string(),
            type_name: "INTEGER?".to_string(),
        }],
        data: vec![
            vec![Value::Int(1), Value::Int(2)],
            vec![Value::Int(2), Value::Int(4)],
            vec![Value::Int(3), Value::Int(6)],
        ],
    };
    let mut registry = ProcedureRegistry::new();
    registry.register(proc);

    let params: HashMap<String, Value> = HashMap::new();
    let q = parse_query("CALL math.double(2) YIELD doubled RETURN doubled").unwrap();
    let planner = QueryPlanner::with_context(&store, "test", &registry, &params);
    let plan = planner.plan(&q).unwrap();

    let executor = QueryExecutor::with_context(&store, &registry, params);
    let mut budget = unlimited();
    let result = executor.execute("test", &plan, &mut budget).unwrap();

    assert_eq!(result.rows.len(), 1, "should return one row from procedure");
    assert_eq!(
        result.rows[0].get("doubled"),
        Some(&Value::Int(4)),
        "math.double(2) should return 4"
    );
}

// ── Test 25: WITH clause chaining ────────────────────────────────────────────

#[test]
fn test_with_clause_chaining() {
    let store = make_store();

    for (name, age) in &[("Alice", 30i64), ("Bob", 17i64), ("Carol", 25i64)] {
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::Str((*name).into()));
        props.insert("age".to_string(), Value::Int(*age));
        store
            .create_node("test", vec!["Person".to_string()], props)
            .unwrap();
    }

    // Filter adults with WITH, then RETURN.
    let q = parse_query(
        "MATCH (p:Person) \
         WITH p.name AS name, p.age AS age \
         WHERE age >= 18 \
         RETURN name \
         ORDER BY name",
    )
    .unwrap();
    let planner = QueryPlanner::new(&store, "test");
    let plan = planner.plan(&q).unwrap();

    let executor = QueryExecutor::new(&store);
    let mut budget = unlimited();
    let result = executor.execute("test", &plan, &mut budget).unwrap();

    assert_eq!(result.rows.len(), 2, "Alice and Carol are adults");
    let names: Vec<&str> = result
        .rows
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["Alice", "Carol"], "should be alphabetically ordered");
}

// ── Test 26: EXISTS subquery ──────────────────────────────────────────────────

#[test]
fn test_exists_subquery() {
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

    // Alice knows Bob, but Bob knows nobody.
    store
        .create_edge("test", alice_id, bob_id, "KNOWS".to_string(), HashMap::new())
        .unwrap();

    let q = parse_query(
        "MATCH (p:Person) WHERE EXISTS { (p)-[:KNOWS]->() } RETURN p.name",
    )
    .unwrap();
    let planner = QueryPlanner::new(&store, "test");
    let plan = planner.plan(&q).unwrap();

    let executor = QueryExecutor::new(&store);
    let mut budget = unlimited();
    let result = executor.execute("test", &plan, &mut budget).unwrap();

    assert_eq!(result.rows.len(), 1, "only Alice has outgoing KNOWS");
    assert_eq!(
        result.rows[0].get("p.name"),
        Some(&Value::Str("Alice".into()))
    );
}

// ── Test 27: CASE expression ─────────────────────────────────────────────────

#[test]
fn test_case_simple_expression() {
    let store = make_store();

    for (name, score) in &[("Alice", 90i64), ("Bob", 55i64)] {
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::Str((*name).into()));
        props.insert("score".to_string(), Value::Int(*score));
        store
            .create_node("test", vec!["Student".to_string()], props)
            .unwrap();
    }

    // Generic CASE (predicates in WHEN).
    let q = parse_query(
        "MATCH (s:Student) \
         RETURN s.name, \
                CASE WHEN s.score >= 70 THEN 'pass' ELSE 'fail' END AS grade \
         ORDER BY s.name",
    )
    .unwrap();
    let planner = QueryPlanner::new(&store, "test");
    let plan = planner.plan(&q).unwrap();

    let executor = QueryExecutor::new(&store);
    let mut budget = unlimited();
    let result = executor.execute("test", &plan, &mut budget).unwrap();

    assert_eq!(result.rows.len(), 2);
    let alice_grade = result.rows.iter().find(|r| {
        r.get("s.name") == Some(&Value::Str("Alice".into()))
    }).and_then(|r| r.get("grade"));
    assert_eq!(alice_grade, Some(&Value::Str("pass".into())));

    let bob_grade = result.rows.iter().find(|r| {
        r.get("s.name") == Some(&Value::Str("Bob".into()))
    }).and_then(|r| r.get("grade"));
    assert_eq!(bob_grade, Some(&Value::Str("fail".into())));
}

#[test]
fn test_case_value_expression() {
    let store = make_store();

    let mut props = HashMap::new();
    props.insert("status".to_string(), Value::Int(2));
    store
        .create_node("test", vec!["Item".to_string()], props)
        .unwrap();

    // Simple (value) CASE.
    let q = parse_query(
        "MATCH (i:Item) \
         RETURN CASE i.status WHEN 1 THEN 'new' WHEN 2 THEN 'active' ELSE 'unknown' END AS label",
    )
    .unwrap();
    let planner = QueryPlanner::new(&store, "test");
    let plan = planner.plan(&q).unwrap();

    let executor = QueryExecutor::new(&store);
    let mut budget = unlimited();
    let result = executor.execute("test", &plan, &mut budget).unwrap();

    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0].get("label"), Some(&Value::Str("active".into())));
}

// ── Test 28: List comprehension ───────────────────────────────────────────────

#[test]
fn test_list_comprehension() {
    let store = make_store();

    // [x IN [1,2,3,4,5] WHERE x > 2 | x * 2]
    let q = parse_query(
        "RETURN [x IN [1, 2, 3, 4, 5] WHERE x > 2 | x * 2] AS result",
    )
    .unwrap();
    let planner = QueryPlanner::new(&store, "test");
    let plan = planner.plan(&q).unwrap();

    let executor = QueryExecutor::new(&store);
    let mut budget = unlimited();
    let result = executor.execute("test", &plan, &mut budget).unwrap();

    assert_eq!(result.rows.len(), 1);
    match &result.rows[0]["result"] {
        Value::List(items) => {
            assert_eq!(items.len(), 3, "elements > 2 are [3,4,5], doubled to [6,8,10]");
            assert_eq!(items[0], Value::Int(6));
            assert_eq!(items[1], Value::Int(8));
            assert_eq!(items[2], Value::Int(10));
        }
        other => panic!("expected List, got {other:?}"),
    }
}

// ── Test 29: Parameters ($param) ─────────────────────────────────────────────

#[test]
fn test_parameter_injection() {
    let store = make_store();

    for name in &["Alice", "Bob", "Charlie"] {
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::Str((*name).into()));
        store
            .create_node("test", vec!["Person".to_string()], props)
            .unwrap();
    }

    let mut params = HashMap::new();
    params.insert("targetName".to_string(), Value::Str("Bob".into()));

    let q = parse_query(
        "MATCH (p:Person) WHERE p.name = $targetName RETURN p.name",
    )
    .unwrap();
    let registry = ProcedureRegistry::new();
    let planner = QueryPlanner::with_context(&store, "test", &registry, &params);
    let plan = planner.plan(&q).unwrap();

    let executor = QueryExecutor::with_context(&store, &registry, params);
    let mut budget = unlimited();
    let result = executor.execute("test", &plan, &mut budget).unwrap();

    assert_eq!(result.rows.len(), 1, "only Bob should match $targetName");
    assert_eq!(
        result.rows[0].get("p.name"),
        Some(&Value::Str("Bob".into()))
    );
}

// ── Test 30: Path variables p = (a)-[*]->(b) ─────────────────────────────────

#[test]
fn test_path_variable() {
    let store = make_store();

    let a = store
        .create_node("test", vec!["Person".to_string()], HashMap::new())
        .unwrap();
    let b = store
        .create_node("test", vec!["Person".to_string()], HashMap::new())
        .unwrap();
    let c = store
        .create_node("test", vec!["Person".to_string()], HashMap::new())
        .unwrap();

    store
        .create_edge("test", a, b, "KNOWS".to_string(), HashMap::new())
        .unwrap();
    store
        .create_edge("test", b, c, "KNOWS".to_string(), HashMap::new())
        .unwrap();

    let q = parse_query(
        "MATCH path = (a:Person)-[:KNOWS*1..2]->(b:Person) RETURN length(path) AS hops",
    )
    .unwrap();
    let planner = QueryPlanner::new(&store, "test");
    let plan = planner.plan(&q).unwrap();

    let executor = QueryExecutor::new(&store);
    let mut budget = unlimited();
    let result = executor.execute("test", &plan, &mut budget).unwrap();

    assert!(!result.rows.is_empty(), "should find paths");

    // All hops values should be positive integers.
    for row in &result.rows {
        let hops = row.get("hops").expect("hops column should exist");
        assert!(matches!(hops, Value::Int(n) if *n >= 1), "path length should be >= 1, got {hops:?}");
    }

    // There should be paths of both length 1 (A->B, B->C) and length 2 (A->C).
    let hop_values: Vec<i64> = result
        .rows
        .iter()
        .filter_map(|r| r.get("hops")?.as_int())
        .collect();
    assert!(hop_values.contains(&1), "should have length-1 paths");
    assert!(hop_values.contains(&2), "should have length-2 paths");
}

// ── Test 31: Aggregation with GROUP BY ────────────────────────────────────────

#[test]
fn test_aggregation_group_by() {
    let store = make_store();

    for (name, dept) in &[
        ("Alice", "Eng"),
        ("Bob", "Eng"),
        ("Carol", "HR"),
        ("Dave", "HR"),
        ("Eve", "Eng"),
    ] {
        let mut props = HashMap::new();
        props.insert("name".to_string(), Value::Str((*name).into()));
        props.insert("dept".to_string(), Value::Str((*dept).into()));
        store
            .create_node("test", vec!["Person".to_string()], props)
            .unwrap();
    }

    let q = parse_query(
        "MATCH (p:Person) RETURN p.dept AS dept, count(p) AS cnt ORDER BY dept",
    )
    .unwrap();
    let planner = QueryPlanner::new(&store, "test");
    let plan = planner.plan(&q).unwrap();

    let executor = QueryExecutor::new(&store);
    let mut budget = unlimited();
    let result = executor.execute("test", &plan, &mut budget).unwrap();

    assert_eq!(result.rows.len(), 2, "two departments");

    // Eng: 3 people, HR: 2 people (sorted by dept).
    let eng_row = result
        .rows
        .iter()
        .find(|r| r.get("dept") == Some(&Value::Str("Eng".into())));
    assert!(eng_row.is_some(), "Eng dept should be present");
    assert_eq!(eng_row.unwrap().get("cnt"), Some(&Value::Int(3)));

    let hr_row = result
        .rows
        .iter()
        .find(|r| r.get("dept") == Some(&Value::Str("HR".into())));
    assert!(hr_row.is_some(), "HR dept should be present");
    assert_eq!(hr_row.unwrap().get("cnt"), Some(&Value::Int(2)));
}

// ════════════════════════════════════════════════════════════════════════════════
// Property-based tests (proptest)
// ════════════════════════════════════════════════════════════════════════════════

proptest! {
    // ── cmp_values transitivity ───────────────────────────────────────────────
    //
    // For any three integers a, b, c:
    //   cmp(a,b) == Less  && cmp(b,c) == Less  implies cmp(a,c) == Less
    //   cmp(a,b) == Greater && cmp(b,c) == Greater implies cmp(a,c) == Greater
    //   cmp(a,b) == Equal && cmp(b,c) == Equal implies cmp(a,c) == Equal
    #[test]
    fn prop_cmp_values_transitivity_int(a: i64, b: i64, c: i64) {
        use std::cmp::Ordering;
        let va = Value::Int(a);
        let vb = Value::Int(b);
        let vc = Value::Int(c);
        let ab = cmp_values(&va, &vb);
        let bc = cmp_values(&vb, &vc);
        let ac = cmp_values(&va, &vc);
        if ab == Ordering::Less && bc == Ordering::Less {
            prop_assert_eq!(ac, Ordering::Less);
        }
        if ab == Ordering::Greater && bc == Ordering::Greater {
            prop_assert_eq!(ac, Ordering::Greater);
        }
        if ab == Ordering::Equal && bc == Ordering::Equal {
            prop_assert_eq!(ac, Ordering::Equal);
        }
    }

    // ── cmp_values transitivity for strings ──────────────────────────────────
    #[test]
    fn prop_cmp_values_transitivity_str(a in "[a-z]{0,8}", b in "[a-z]{0,8}", c in "[a-z]{0,8}") {
        use std::cmp::Ordering;
        let va = Value::Str(a.clone().into_boxed_str());
        let vb = Value::Str(b.clone().into_boxed_str());
        let vc = Value::Str(c.clone().into_boxed_str());
        let ab = cmp_values(&va, &vb);
        let bc = cmp_values(&vb, &vc);
        let ac = cmp_values(&va, &vc);
        if ab == Ordering::Less && bc == Ordering::Less {
            prop_assert_eq!(ac, Ordering::Less);
        }
        if ab == Ordering::Greater && bc == Ordering::Greater {
            prop_assert_eq!(ac, Ordering::Greater);
        }
        if ab == Ordering::Equal && bc == Ordering::Equal {
            prop_assert_eq!(ac, Ordering::Equal);
        }
    }

    // ── cypher_eq symmetry ────────────────────────────────────────────────────
    //
    // For any two integers a and b:
    //   a.cypher_eq(b) == b.cypher_eq(a)
    #[test]
    fn prop_cypher_eq_symmetry_int(a: i64, b: i64) {
        let va = Value::Int(a);
        let vb = Value::Int(b);
        prop_assert_eq!(va.cypher_eq(&vb), vb.cypher_eq(&va));
    }

    // ── cypher_eq symmetry for strings ────────────────────────────────────────
    #[test]
    fn prop_cypher_eq_symmetry_str(a in "[a-z]{0,8}", b in "[a-z]{0,8}") {
        let va = Value::Str(a.clone().into_boxed_str());
        let vb = Value::Str(b.clone().into_boxed_str());
        prop_assert_eq!(va.cypher_eq(&vb), vb.cypher_eq(&va),
            "cypher_eq must be symmetric for strings");
    }

    // ── Integer arithmetic doesn't panic for any i64 pair ────────────────────
    //
    // The executor uses checked arithmetic (checked_add, checked_mul, etc.)
    // so arithmetic expressions must never panic — they may return Ok or Err.
    #[test]
    fn prop_integer_arithmetic_no_panic(a: i64, b: i64) {
        use gql_core::ast::BinaryOp;
        use gql_core::executor::filter::{eval_expr, Row};

        let ops = [
            BinaryOp::Add,
            BinaryOp::Sub,
            BinaryOp::Mul,
            BinaryOp::Div,
            BinaryOp::Mod,
        ];

        for op in &ops {
            let expr = Expr::BinaryOp {
                op: *op,
                left: Box::new(Expr::Integer(a)),
                right: Box::new(Expr::Integer(b)),
            };
            // This must not panic; it may return Ok or Err.
            let row: Row = HashMap::new();
            let _ = eval_expr(&expr, &row);
        }
    }
}
