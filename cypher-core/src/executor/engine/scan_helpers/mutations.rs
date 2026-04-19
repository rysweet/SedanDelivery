//! Tests for `Create`, `Set`, and `Remove` operators.

use super::common::make_store;
use crate::ast::{Expr, Identifier, Label};
use crate::executor::budget::ExecutionBudget;
use crate::executor::engine::QueryExecutor;
use crate::planner::{CreatePattern, Operator, SetItem};
use crate::storage::GraphStore;
use crate::types::Value;
use indexmap::IndexMap;

#[test]
fn create_operator() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Create {
        patterns: vec![CreatePattern {
            nodes: vec![(
                Some(Identifier::new("n")),
                vec![Label::new("Person")],
                vec![("name".to_string(), Expr::Str("Dave".into()))],
            )],
            edges: vec![],
        }],
        child: None,
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.stats.nodes_created, 1);
    let ids = store.find_nodes_by_label("g", "Person").unwrap();
    assert_eq!(ids.len(), 1);
}

#[test]
fn set_property() {
    let store = make_store();
    let mut props = IndexMap::new();
    props.insert("name".to_string(), Value::Str("Alice".into()));
    let node_id = store
        .create_node("g", vec!["Person".into()], props)
        .unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Set {
        items: vec![SetItem::Property {
            alias: Identifier::new("n"),
            key: "age".to_string(),
            value: Expr::Integer(30),
        }],
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("Person")),
            alias: Identifier::new("n"),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.stats.properties_set, 1);
    let node = store.get_node("g", node_id).unwrap().unwrap();
    assert_eq!(node.properties.get("age"), Some(&Value::Int(30)));
    assert_eq!(
        node.properties.get("name"),
        Some(&Value::Str("Alice".into()))
    );
}

#[test]
fn set_labels() {
    let store = make_store();
    let node_id = store
        .create_node("g", vec!["Person".into()], IndexMap::new())
        .unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Set {
        items: vec![SetItem::Labels {
            alias: Identifier::new("n"),
            labels: vec![Label::new("Employee")],
        }],
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("Person")),
            alias: Identifier::new("n"),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.stats.labels_added, 1);
    let node = store.get_node("g", node_id).unwrap().unwrap();
    assert!(node.labels.contains(&"Employee".to_string()));
    assert!(node.labels.contains(&"Person".to_string()));
}

#[test]
fn set_map_merge() {
    let store = make_store();
    let mut props = IndexMap::new();
    props.insert("name".to_string(), Value::Str("Alice".into()));
    let node_id = store
        .create_node("g", vec!["Person".into()], props)
        .unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Set {
        items: vec![SetItem::Map {
            alias: Identifier::new("n"),
            map: Expr::MapLiteral(vec![(smol_str::SmolStr::new("age"), Expr::Integer(25))]),
            merge: true,
        }],
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("Person")),
            alias: Identifier::new("n"),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.stats.properties_set, 1);
    let node = store.get_node("g", node_id).unwrap().unwrap();
    assert_eq!(
        node.properties.get("name"),
        Some(&Value::Str("Alice".into()))
    );
    assert_eq!(node.properties.get("age"), Some(&Value::Int(25)));
}

#[test]
fn remove_property() {
    let store = make_store();
    let mut props = IndexMap::new();
    props.insert("name".to_string(), Value::Str("Alice".into()));
    props.insert("age".to_string(), Value::Int(30));
    let node_id = store
        .create_node("g", vec!["Person".into()], props)
        .unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Remove {
        items: vec![crate::planner::RemoveItem::Property {
            alias: Identifier::new("n"),
            key: "age".to_string(),
        }],
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("Person")),
            alias: Identifier::new("n"),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.stats.properties_set, 0);
    let node = store.get_node("g", node_id).unwrap().unwrap();
    assert!(!node.properties.contains_key("age"));
    assert_eq!(
        node.properties.get("name"),
        Some(&Value::Str("Alice".into()))
    );
}

#[test]
fn remove_labels() {
    let store = make_store();
    let node_id = store
        .create_node(
            "g",
            vec!["Person".into(), "Employee".into()],
            IndexMap::new(),
        )
        .unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Remove {
        items: vec![crate::planner::RemoveItem::Labels {
            alias: Identifier::new("n"),
            labels: vec![Label::new("Employee")],
        }],
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("Person")),
            alias: Identifier::new("n"),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let _result = executor.execute("g", &plan, &mut budget).unwrap();
    let node = store.get_node("g", node_id).unwrap().unwrap();
    assert!(node.labels.contains(&"Person".to_string()));
    assert!(!node.labels.contains(&"Employee".to_string()));
}
