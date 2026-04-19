//! Tests for the `Merge` operator (create / match / without child).

use super::common::make_store;
use crate::ast::{Expr, Identifier, Label};
use crate::executor::budget::ExecutionBudget;
use crate::executor::engine::QueryExecutor;
use crate::planner::{Operator, SetItem};
use crate::storage::GraphStore;
use crate::types::Value;
use indexmap::IndexMap;

#[test]
fn merge_creates_when_not_found() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Merge {
        node_alias: Some(Identifier::new("n")),
        labels: vec![Label::new("Person")],
        properties: vec![],
        on_create: vec![SetItem::Property {
            alias: Identifier::new("n"),
            key: "name".to_string(),
            value: Expr::Str("Alice".into()),
        }],
        on_match: vec![],
        child: None,
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.stats.nodes_created, 1);
    assert_eq!(result.stats.properties_set, 1);
    let ids = store.find_nodes_by_label("g", "Person").unwrap();
    assert_eq!(ids.len(), 1);
    let node = store.get_node("g", ids[0]).unwrap().unwrap();
    assert_eq!(
        node.properties.get("name"),
        Some(&Value::Str("Alice".into()))
    );
}

#[test]
fn merge_matches_existing() {
    let store = make_store();
    let mut props = IndexMap::new();
    props.insert("name".to_string(), Value::Str("Alice".into()));
    let existing = store
        .create_node("g", vec!["Person".into()], props)
        .unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Merge {
        node_alias: Some(Identifier::new("n")),
        labels: vec![Label::new("Person")],
        properties: vec![],
        on_create: vec![],
        on_match: vec![SetItem::Property {
            alias: Identifier::new("n"),
            key: "seen".to_string(),
            value: Expr::Bool(true),
        }],
        child: None,
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.stats.nodes_created, 0);
    assert_eq!(result.stats.properties_set, 1);
    let node = store.get_node("g", existing).unwrap().unwrap();
    assert_eq!(node.properties.get("seen"), Some(&Value::Bool(true)));
}

#[test]
fn merge_without_child() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Merge {
        node_alias: Some(Identifier::new("n")),
        labels: vec![Label::new("City")],
        properties: vec![],
        on_create: vec![],
        on_match: vec![],
        child: None,
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.stats.nodes_created, 1);
    assert_eq!(result.rows.len(), 1);
    assert!(result.rows[0].contains_key("n"));
}
