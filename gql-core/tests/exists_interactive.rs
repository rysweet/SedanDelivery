//! Interactive tests for EXISTS { subquery } and bare pattern predicates.
//! These tests verify the end-to-end behavior of the existential subquery feature.

use gql_core::executor::{budget::ExecutionBudget, engine::QueryExecutor};
use gql_core::parser::parse_query;
use gql_core::planner::QueryPlanner;
use gql_core::storage::{GraphStore, InMemoryGraphStore};
use gql_core::types::Value;
use indexmap::IndexMap;
use std::collections::HashMap;

fn setup_graph() -> InMemoryGraphStore {
    let store = InMemoryGraphStore::default();
    store.create_graph("test").unwrap();

    let alice = store
        .create_node(
            "test",
            vec!["Person".into()],
            IndexMap::from([
                ("name".into(), Value::Str("Alice".into())),
                ("age".into(), Value::Int(30)),
            ]),
        )
        .unwrap();
    let bob = store
        .create_node(
            "test",
            vec!["Person".into()],
            IndexMap::from([
                ("name".into(), Value::Str("Bob".into())),
                ("age".into(), Value::Int(25)),
            ]),
        )
        .unwrap();
    let _charlie = store
        .create_node(
            "test",
            vec!["Person".into()],
            IndexMap::from([
                ("name".into(), Value::Str("Charlie".into())),
                ("age".into(), Value::Int(20)),
            ]),
        )
        .unwrap();

    store
        .create_edge("test", alice, bob, "KNOWS".into(), IndexMap::new())
        .unwrap();
    store
}

fn run_query(store: &InMemoryGraphStore, query: &str) -> Vec<HashMap<String, Value>> {
    let ast = parse_query(query).expect("parse error");
    let planner = QueryPlanner::new(store, "test");
    let plan = planner.plan(&ast).expect("plan error");
    let executor = QueryExecutor::new(store);
    let mut budget = ExecutionBudget::default_budget();
    executor
        .execute("test", &plan, &mut budget)
        .expect("exec error")
        .rows
}

#[test]
fn test_exists_simple_pattern() {
    let store = setup_graph();
    let rows = run_query(
        &store,
        "MATCH (n) WHERE EXISTS { (n)-[:KNOWS]->() } RETURN n",
    );
    assert_eq!(rows.len(), 1, "Only Alice has an outgoing KNOWS edge");
}

#[test]
fn test_exists_full_match_form() {
    let store = setup_graph();
    let rows = run_query(
        &store,
        "MATCH (n) WHERE EXISTS { MATCH (n)-[:KNOWS]->(m) } RETURN n",
    );
    assert_eq!(rows.len(), 1, "Full MATCH form: only Alice has KNOWS");
}

#[test]
fn test_exists_as_return_projection() {
    let store = setup_graph();
    let rows = run_query(
        &store,
        "MATCH (n:Person) RETURN n.name AS name, EXISTS { (n)-[:KNOWS]->() } AS hasKnows",
    );
    assert_eq!(rows.len(), 3, "All 3 persons returned");
    let alice = rows
        .iter()
        .find(|r| r.get("name") == Some(&Value::Str("Alice".into())))
        .unwrap();
    assert_eq!(alice.get("hasKnows"), Some(&Value::Bool(true)));
    let bob = rows
        .iter()
        .find(|r| r.get("name") == Some(&Value::Str("Bob".into())))
        .unwrap();
    assert_eq!(bob.get("hasKnows"), Some(&Value::Bool(false)));
}

#[test]
fn test_bare_pattern_predicate() {
    let store = setup_graph();
    let rows = run_query(&store, "MATCH (n) WHERE (n)-[:KNOWS]->() RETURN n");
    assert_eq!(rows.len(), 1, "Pattern predicate: only Alice matches");
}

#[test]
fn test_exists_with_where_clause() {
    let store = setup_graph();
    let rows = run_query(
        &store,
        "MATCH (n) WHERE EXISTS { (n)-[:KNOWS]->(m) WHERE m.age < 28 } RETURN n",
    );
    assert_eq!(rows.len(), 1, "Alice knows Bob whose age (25) < 28");
}

#[test]
fn test_not_exists() {
    let store = setup_graph();
    let rows = run_query(
        &store,
        "MATCH (n) WHERE NOT EXISTS { (n)-[:KNOWS]->() } RETURN n",
    );
    assert_eq!(
        rows.len(),
        2,
        "Bob and Charlie have no outgoing KNOWS edges"
    );
}
