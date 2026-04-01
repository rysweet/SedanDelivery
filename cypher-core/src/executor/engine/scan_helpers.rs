//! Scan-related helper tests for the query executor engine.

#[cfg(test)]
mod tests {
    use crate::ast::{Direction, Expr, Identifier, Label, RelType};
    use crate::executor::budget::ExecutionBudget;
    use crate::executor::engine::QueryExecutor;
    use crate::planner::{CreatePattern, Operator, SetItem};
    use crate::storage::{GraphStore, InMemoryGraphStore};
    use crate::types::Value;
    use indexmap::IndexMap;

    fn make_store() -> InMemoryGraphStore {
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();
        store
    }

    #[test]
    fn node_scan_no_label() {
        let store = make_store();
        store
            .create_node("g", vec!["Person".into()], IndexMap::new())
            .unwrap();
        store
            .create_node("g", vec!["Movie".into()], IndexMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::NodeScan {
            label: None,
            alias: Identifier::new("n"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 2);
    }

    #[test]
    fn node_scan_with_label() {
        let store = make_store();
        store
            .create_node("g", vec!["Person".into()], IndexMap::new())
            .unwrap();
        store
            .create_node("g", vec!["Movie".into()], IndexMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::NodeScan {
            label: Some(Label::new("Person")),
            alias: Identifier::new("n"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
    }

    #[test]
    fn filter_operator() {
        let store = make_store();
        let mut props = IndexMap::new();
        props.insert("age".to_string(), Value::Int(30));
        store
            .create_node("g", vec!["Person".into()], props)
            .unwrap();
        let mut props2 = IndexMap::new();
        props2.insert("age".to_string(), Value::Int(20));
        store
            .create_node("g", vec!["Person".into()], props2)
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Filter {
            predicate: Expr::BinaryOp {
                op: crate::ast::BinaryOp::Gte,
                left: Box::new(Expr::Property(
                    Box::new(Expr::Variable(Identifier::new("n"))),
                    "age".into(),
                )),
                right: Box::new(Expr::Integer(25)),
            },
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("Person")),
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
    }

    #[test]
    fn expand_operator() {
        let store = make_store();
        let alice = store
            .create_node("g", vec!["Person".into()], IndexMap::new())
            .unwrap();
        let bob = store
            .create_node("g", vec!["Person".into()], IndexMap::new())
            .unwrap();
        store
            .create_edge("g", alice, bob, "KNOWS".into(), IndexMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Expand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("KNOWS")],
            src_alias: Identifier::new("a"),
            rel_alias: Some(Identifier::new("r")),
            dst_alias: Identifier::new("b"),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("Person")),
                alias: Identifier::new("a"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
        assert!(result.rows[0].contains_key("b"));
    }

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

    #[test]
    fn delete_node_detach() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["Person".into()], IndexMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["Person".into()], IndexMap::new())
            .unwrap();
        store
            .create_edge("g", a, b, "KNOWS".into(), IndexMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Delete {
            exprs: vec![Expr::Variable(Identifier::new("n"))],
            detach: true,
            child: Box::new(Operator::Filter {
                predicate: Expr::Bool(true),
                child: Box::new(Operator::NodeScan {
                    label: Some(Label::new("Person")),
                    alias: Identifier::new("n"),
                }),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.nodes_deleted, 2);
    }

    #[test]
    fn delete_null_is_noop() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Delete {
            exprs: vec![Expr::Null],
            detach: false,
            child: Box::new(Operator::NodeScan {
                label: None,
                alias: Identifier::new("n"),
            }),
        };
        store.create_node("g", vec![], IndexMap::new()).unwrap();
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.stats.nodes_deleted, 0);
    }

    #[test]
    fn delete_edge() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".into()], IndexMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["N".into()], IndexMap::new())
            .unwrap();
        let eid = store
            .create_edge("g", a, b, "R".into(), IndexMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Delete {
            exprs: vec![Expr::Variable(Identifier::new("r"))],
            detach: false,
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
        assert_eq!(result.stats.edges_deleted, 1);
        assert!(store.get_edge("g", eid).unwrap().is_none());
    }

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

    #[test]
    fn union_all_concatenates() {
        let store = make_store();
        store
            .create_node("g", vec!["Person".into()], IndexMap::new())
            .unwrap();
        store
            .create_node("g", vec!["Movie".into()], IndexMap::new())
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
            .create_node("g", vec!["Both".into()], IndexMap::new())
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
    fn left_outer_apply_with_match() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["Person".into()], IndexMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["Person".into()], IndexMap::new())
            .unwrap();
        store
            .create_edge("g", a, b, "KNOWS".into(), IndexMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::LeftOuterApply {
            preserved: Box::new(Operator::NodeScan {
                label: Some(Label::new("Person")),
                alias: Identifier::new("a"),
            }),
            optional_pattern: Box::new(Operator::Expand {
                direction: Direction::Outgoing,
                rel_types: vec![RelType::new("KNOWS")],
                src_alias: Identifier::new("a"),
                rel_alias: Some(Identifier::new("r")),
                dst_alias: Identifier::new("friend"),
                child: Box::new(Operator::NodeScan {
                    label: Some(Label::new("Person")),
                    alias: Identifier::new("a"),
                }),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 2);
    }

    #[test]
    fn left_outer_apply_no_match_produces_nulls() {
        let store = make_store();
        store
            .create_node("g", vec!["Person".into()], IndexMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::LeftOuterApply {
            preserved: Box::new(Operator::NodeScan {
                label: Some(Label::new("Person")),
                alias: Identifier::new("a"),
            }),
            optional_pattern: Box::new(Operator::Expand {
                direction: Direction::Outgoing,
                rel_types: vec![RelType::new("KNOWS")],
                src_alias: Identifier::new("a"),
                rel_alias: None,
                dst_alias: Identifier::new("b"),
                child: Box::new(Operator::NodeScan {
                    label: Some(Label::new("Person")),
                    alias: Identifier::new("a"),
                }),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0].get("b"), Some(&Value::Null));
    }

    #[test]
    fn apply_combines_outer_inner() {
        let store = make_store();
        store
            .create_node("g", vec!["A".into()], IndexMap::new())
            .unwrap();
        store
            .create_node("g", vec!["B".into()], IndexMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Apply {
            outer: Box::new(Operator::NodeScan {
                label: Some(Label::new("A")),
                alias: Identifier::new("a"),
            }),
            inner: Box::new(Operator::NodeScan {
                label: Some(Label::new("B")),
                alias: Identifier::new("b"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
        assert!(result.rows[0].contains_key("a"));
        assert!(result.rows[0].contains_key("b"));
    }

    #[test]
    fn apply_empty_inner_passes_outer() {
        let store = make_store();
        store
            .create_node("g", vec!["A".into()], IndexMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Apply {
            outer: Box::new(Operator::NodeScan {
                label: Some(Label::new("A")),
                alias: Identifier::new("a"),
            }),
            inner: Box::new(Operator::NodeScan {
                label: Some(Label::new("NonExistent")),
                alias: Identifier::new("b"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 0);
    }

    #[test]
    fn index_scan_finds_nodes() {
        let store = make_store();
        let mut p1 = IndexMap::new();
        p1.insert("age".to_string(), Value::Int(30));
        store.create_node("g", vec!["Person".into()], p1).unwrap();
        let mut p2 = IndexMap::new();
        p2.insert("age".to_string(), Value::Int(25));
        store.create_node("g", vec!["Person".into()], p2).unwrap();
        store
            .create_node("g", vec!["Person".into()], IndexMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::IndexScan {
            label: Label::new("Person"),
            property: "age".to_string(),
            value: None,
            alias: Identifier::new("n"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 2);
    }

    #[test]
    fn index_scan_empty_result() {
        let store = make_store();
        store
            .create_node("g", vec!["Person".into()], IndexMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::IndexScan {
            label: Label::new("Person"),
            property: "nonexistent".to_string(),
            value: None,
            alias: Identifier::new("n"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 0);
    }

    #[test]
    fn node_by_id_seek_found() {
        let store = make_store();
        let node_id = store
            .create_node("g", vec!["Person".into()], IndexMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::NodeByIdSeek {
            id_expr: Expr::Integer(node_id.0 as i64),
            alias: Identifier::new("n"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0]["n"], Value::Node(node_id));
    }

    #[test]
    fn node_by_id_seek_not_found() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::NodeByIdSeek {
            id_expr: Expr::Integer(99999),
            alias: Identifier::new("n"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 0);
    }

    #[test]
    fn node_by_id_seek_negative_id_is_error() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::NodeByIdSeek {
            id_expr: Expr::Integer(-1),
            alias: Identifier::new("n"),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget);
        assert!(result.is_err());
    }
}
