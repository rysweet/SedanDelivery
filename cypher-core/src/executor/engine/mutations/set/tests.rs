//! Tests for SET and REMOVE operator implementations.

use crate::ast::{Expr, Identifier, Label};
use crate::executor::budget::ExecutionBudget;
use crate::executor::engine::{MutationStats, QueryExecutor, Row};
use crate::planner::{Operator, RemoveItem, SetItem};
use crate::storage::{GraphStore, InMemoryGraphStore};
use crate::types::Value;
use indexmap::IndexMap;

fn make_store() -> InMemoryGraphStore {
    let store = InMemoryGraphStore::new();
    store.create_graph("g").unwrap();
    store
}

#[test]
fn set_property() {
    let store = make_store();
    let nid = store
        .create_node("g", vec!["P".to_string()], IndexMap::new())
        .unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Set {
        items: vec![SetItem::Property {
            alias: Identifier::new("n"),
            key: "x".to_string(),
            value: Expr::Integer(1),
        }],
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("P")),
            alias: Identifier::new("n"),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.stats.properties_set, 1);
    assert_eq!(
        store
            .get_node("g", nid)
            .unwrap()
            .unwrap()
            .properties
            .get("x"),
        Some(&Value::Int(1))
    );
}

#[test]
fn remove_property() {
    let store = make_store();
    let mut props = IndexMap::new();
    props.insert("x".to_string(), Value::Int(1));
    let nid = store
        .create_node("g", vec!["P".to_string()], props)
        .unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Remove {
        items: vec![RemoveItem::Property {
            alias: Identifier::new("n"),
            key: "x".to_string(),
        }],
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("P")),
            alias: Identifier::new("n"),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let _ = executor.execute("g", &plan, &mut budget).unwrap();
    assert!(!store
        .get_node("g", nid)
        .unwrap()
        .unwrap()
        .properties
        .contains_key("x"));
}

#[test]
fn set_null_propagates_removal_error() {
    let store = make_store();
    let nid = store
        .create_node("g", vec!["P".to_string()], IndexMap::new())
        .unwrap();
    store.delete_node("g", nid, true).unwrap();

    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert("n".to_string(), Value::Node(nid));
    let mut stats = MutationStats::default();

    let item = SetItem::Property {
        alias: Identifier::new("n"),
        key: "prop".to_string(),
        value: Expr::Null,
    };
    let result = executor.apply_set_item("g", &row, &item, &mut stats);
    assert!(
        result.is_err(),
        "expected error when removing property from deleted node"
    );
}

#[test]
fn set_property_on_edge() {
    use crate::ast::{Direction, RelType};
    let store = make_store();
    let a = store
        .create_node("g", vec!["N".to_string()], IndexMap::new())
        .unwrap();
    let b = store
        .create_node("g", vec!["N".to_string()], IndexMap::new())
        .unwrap();
    let eid = store
        .create_edge("g", a, b, "R".to_string(), IndexMap::new())
        .unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Set {
        items: vec![SetItem::Property {
            alias: Identifier::new("r"),
            key: "weight".to_string(),
            value: Expr::Integer(5),
        }],
        child: Box::new(Operator::Expand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("R")],
            src_alias: Identifier::new("a"),
            rel_alias: Some(Identifier::new("r")),
            dst_alias: Identifier::new("b"),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("a"),
            }),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.stats.properties_set, 1);
    let edge = store.get_edge("g", eid).unwrap().unwrap();
    assert_eq!(edge.properties.get("weight"), Some(&Value::Int(5)));
}

#[test]
fn set_null_property_removes_it() {
    let store = make_store();
    let mut props = IndexMap::new();
    props.insert("x".to_string(), Value::Int(42));
    let nid = store
        .create_node("g", vec!["P".to_string()], props)
        .unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Set {
        items: vec![SetItem::Property {
            alias: Identifier::new("n"),
            key: "x".to_string(),
            value: Expr::Null,
        }],
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("P")),
            alias: Identifier::new("n"),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let _ = executor.execute("g", &plan, &mut budget).unwrap();
    let node = store.get_node("g", nid).unwrap().unwrap();
    assert!(
        !node.properties.contains_key("x"),
        "SET prop = null should remove the property"
    );
}

#[test]
fn set_labels() {
    let store = make_store();
    let nid = store
        .create_node("g", vec!["P".to_string()], IndexMap::new())
        .unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Set {
        items: vec![SetItem::Labels {
            alias: Identifier::new("n"),
            labels: vec![Label::new("Extra")],
        }],
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("P")),
            alias: Identifier::new("n"),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.stats.labels_added, 1);
    let node = store.get_node("g", nid).unwrap().unwrap();
    assert!(node.labels.contains(&"Extra".to_string()));
    assert!(node.labels.contains(&"P".to_string()));
}

#[test]
fn remove_labels() {
    let store = make_store();
    let nid = store
        .create_node("g", vec!["P".to_string(), "Q".to_string()], IndexMap::new())
        .unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Remove {
        items: vec![RemoveItem::Labels {
            alias: Identifier::new("n"),
            labels: vec![Label::new("Q")],
        }],
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("P")),
            alias: Identifier::new("n"),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.stats.labels_removed, 1);
    let node = store.get_node("g", nid).unwrap().unwrap();
    assert!(node.labels.contains(&"P".to_string()));
    assert!(!node.labels.contains(&"Q".to_string()));
}

#[test]
fn set_map_merge_preserves_existing() {
    let store = make_store();
    let mut props = IndexMap::new();
    props.insert("name".to_string(), Value::Str("Alice".into()));
    let nid = store
        .create_node("g", vec!["P".to_string()], props)
        .unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Set {
        items: vec![SetItem::Map {
            alias: Identifier::new("n"),
            map: Expr::MapLiteral(vec![(smol_str::SmolStr::new("age"), Expr::Integer(30))]),
            merge: true,
        }],
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("P")),
            alias: Identifier::new("n"),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let _ = executor.execute("g", &plan, &mut budget).unwrap();
    let node = store.get_node("g", nid).unwrap().unwrap();
    assert_eq!(
        node.properties.get("name"),
        Some(&Value::Str("Alice".into()))
    );
    assert_eq!(node.properties.get("age"), Some(&Value::Int(30)));
}

#[test]
fn set_map_replace_removes_old() {
    let store = make_store();
    let mut props = IndexMap::new();
    props.insert("name".to_string(), Value::Str("Alice".into()));
    props.insert("age".to_string(), Value::Int(30));
    let nid = store
        .create_node("g", vec!["P".to_string()], props)
        .unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Set {
        items: vec![SetItem::Map {
            alias: Identifier::new("n"),
            map: Expr::MapLiteral(vec![(
                smol_str::SmolStr::new("title"),
                Expr::Str("Dr".into()),
            )]),
            merge: false,
        }],
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("P")),
            alias: Identifier::new("n"),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let _ = executor.execute("g", &plan, &mut budget).unwrap();
    let node = store.get_node("g", nid).unwrap().unwrap();
    assert!(
        !node.properties.contains_key("name"),
        "old properties should be replaced"
    );
    assert!(
        !node.properties.contains_key("age"),
        "old properties should be replaced"
    );
    assert_eq!(node.properties.get("title"), Some(&Value::Str("Dr".into())));
}

#[test]
fn set_on_null_variable_is_noop() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert("n".to_string(), Value::Null);
    let mut stats = MutationStats::default();
    let item = SetItem::Property {
        alias: Identifier::new("n"),
        key: "x".to_string(),
        value: Expr::Integer(1),
    };
    executor
        .apply_set_item("g", &row, &item, &mut stats)
        .unwrap();
    assert_eq!(stats.properties_set, 0);
}
