//! Tests for `MERGE (n {...})` node merging.

use indexmap::IndexMap;

use super::make_store;
use crate::ast::{Expr, Identifier, Label};
use crate::executor::budget::ExecutionBudget;
use crate::executor::engine::QueryExecutor;
use crate::planner::{Operator, SetItem};
use crate::storage::GraphStore;
use crate::types::Value;

#[test]
fn merge_creates_when_not_found() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Merge {
        node_alias: Some(Identifier::new("n")),
        labels: vec![Label::new("P")],
        properties: vec![],
        on_create: vec![],
        on_match: vec![],
        child: None,
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.stats.nodes_created, 1);
}

#[test]
fn merge_matches_existing() {
    let store = make_store();
    store
        .create_node("g", vec!["P".to_string()], IndexMap::new())
        .unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Merge {
        node_alias: Some(Identifier::new("n")),
        labels: vec![Label::new("P")],
        properties: vec![],
        on_create: vec![],
        on_match: vec![],
        child: None,
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.stats.nodes_created, 0);
}

#[test]
fn merge_returns_all_matches_no_create() {
    let store = make_store();
    // Create 5 nodes with the same label.
    for _ in 0..5 {
        store
            .create_node("g", vec!["P".to_string()], IndexMap::new())
            .unwrap();
    }
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Merge {
        node_alias: Some(Identifier::new("n")),
        labels: vec![Label::new("P")],
        properties: vec![],
        on_create: vec![],
        on_match: vec![],
        child: None,
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    // All 5 existing nodes should match; no new nodes created.
    assert_eq!(result.stats.nodes_created, 0);
    assert_eq!(
        result.rows.len(),
        5,
        "MERGE should produce one row per matching node"
    );
}

#[test]
fn merge_on_create_sets_property() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Merge {
        node_alias: Some(Identifier::new("n")),
        labels: vec![Label::new("P")],
        properties: vec![],
        on_create: vec![SetItem::Property {
            alias: Identifier::new("n"),
            key: "created".to_string(),
            value: Expr::Bool(true),
        }],
        on_match: vec![],
        child: None,
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.stats.nodes_created, 1);
    let ids = store.find_nodes_by_label("g", "P").unwrap();
    let node = store.get_node("g", ids[0]).unwrap().unwrap();
    assert_eq!(node.properties.get("created"), Some(&Value::Bool(true)));
}

#[test]
fn merge_on_match_sets_property() {
    let store = make_store();
    let nid = store
        .create_node("g", vec!["P".to_string()], IndexMap::new())
        .unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Merge {
        node_alias: Some(Identifier::new("n")),
        labels: vec![Label::new("P")],
        properties: vec![],
        on_create: vec![],
        on_match: vec![SetItem::Property {
            alias: Identifier::new("n"),
            key: "matched".to_string(),
            value: Expr::Bool(true),
        }],
        child: None,
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.stats.nodes_created, 0);
    let node = store.get_node("g", nid).unwrap().unwrap();
    assert_eq!(node.properties.get("matched"), Some(&Value::Bool(true)));
}

#[test]
fn merge_with_properties_creates_matching_node() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Merge {
        node_alias: Some(Identifier::new("n")),
        labels: vec![Label::new("P")],
        properties: vec![("name".to_string(), Expr::Str("Alice".into()))],
        on_create: vec![],
        on_match: vec![],
        child: None,
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.stats.nodes_created, 1);
    let ids = store.find_nodes_by_label("g", "P").unwrap();
    let node = store.get_node("g", ids[0]).unwrap().unwrap();
    assert_eq!(
        node.properties.get("name"),
        Some(&Value::Str("Alice".into()))
    );
}

#[test]
fn merge_with_properties_finds_existing() {
    let store = make_store();
    let mut props = IndexMap::new();
    props.insert("name".to_string(), Value::Str("Alice".into()));
    store
        .create_node("g", vec!["P".to_string()], props)
        .unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Merge {
        node_alias: Some(Identifier::new("n")),
        labels: vec![Label::new("P")],
        properties: vec![("name".to_string(), Expr::Str("Alice".into()))],
        on_create: vec![],
        on_match: vec![],
        child: None,
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.stats.nodes_created, 0);
    assert_eq!(result.rows.len(), 1);
}

#[test]
fn merge_with_null_property_returns_error() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Merge {
        node_alias: Some(Identifier::new("n")),
        labels: vec![Label::new("P")],
        properties: vec![("name".to_string(), Expr::Null)],
        on_create: vec![],
        on_match: vec![],
        child: None,
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget);
    assert!(
        result.is_err(),
        "MERGE with null property should be an error"
    );
}
