//! Tests for the CREATE operator.

use super::*;
use crate::ast::{Expr, Identifier, Label};
use crate::executor::budget::ExecutionBudget;
use crate::planner::{CreatePattern, Operator};
use crate::storage::{GraphStore, InMemoryGraphStore};

fn make_store() -> InMemoryGraphStore {
    let store = InMemoryGraphStore::new();
    store.create_graph("g").unwrap();
    store
}

#[test]
fn create_node() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Create {
        patterns: vec![CreatePattern {
            nodes: vec![(
                Some(Identifier::new("n")),
                vec![Label::new("P")],
                vec![("name".to_string(), Expr::Str("A".into()))],
            )],
            edges: vec![],
        }],
        child: None,
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.stats.nodes_created, 1);
}

#[test]
fn create_node_with_multiple_labels() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Create {
        patterns: vec![CreatePattern {
            nodes: vec![(
                Some(Identifier::new("n")),
                vec![Label::new("Person"), Label::new("Employee")],
                vec![],
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
    let ids2 = store.find_nodes_by_label("g", "Employee").unwrap();
    assert_eq!(ids2.len(), 1);
}

#[test]
fn create_node_null_properties_filtered() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Create {
        patterns: vec![CreatePattern {
            nodes: vec![(
                Some(Identifier::new("n")),
                vec![Label::new("P")],
                vec![
                    ("name".to_string(), Expr::Str("Alice".into())),
                    ("age".to_string(), Expr::Null),
                ],
            )],
            edges: vec![],
        }],
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
    assert!(
        !node.properties.contains_key("age"),
        "null property should be filtered out"
    );
}

#[test]
fn parameterized_unwind_create_does_not_retain_batch_parameter_in_rows() {
    let store = make_store();
    let mut params = std::collections::HashMap::new();

    let mut first = IndexMap::new();
    first.insert("name".into(), Value::Str("Alice".into()));
    let mut second = IndexMap::new();
    second.insert("name".into(), Value::Str("Bob".into()));

    params.insert(
        "resources".to_string(),
        Value::List(vec![Value::Map(first), Value::Map(second)]),
    );
    params.insert("tenant".to_string(), Value::Str("acme".into()));

    let registry = crate::procedure::ProcedureRegistry::new();
    let executor = QueryExecutor::with_context(&store, &registry, params);
    let plan = Operator::Create {
        patterns: vec![CreatePattern {
            nodes: vec![(
                Some(Identifier::new("n")),
                vec![Label::new("Person")],
                vec![
                    (
                        "name".to_string(),
                        Expr::Property(
                            Box::new(Expr::Variable(Identifier::new("resource"))),
                            "name".into(),
                        ),
                    ),
                    ("tenant".to_string(), Expr::Parameter("tenant".to_string())),
                ],
            )],
            edges: vec![],
        }],
        child: Some(Box::new(Operator::Unwind {
            list_expr: Expr::Parameter("resources".to_string()),
            alias: Identifier::new("resource"),
            child: Box::new(Operator::SingleRow),
        })),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();

    assert_eq!(result.stats.nodes_created, 2);
    assert_eq!(result.rows.len(), 2);
    for row in &result.rows {
        assert!(row.contains_key("resource"));
        assert!(row.contains_key("n"));
        assert!(!row.contains_key("$resources"));
        assert!(!row.contains_key("$tenant"));
    }

    let node_ids = store.find_nodes_by_label("g", "Person").unwrap();
    let mut names = Vec::new();
    for node_id in node_ids {
        let node = store.get_node("g", node_id).unwrap().unwrap();
        names.push(node.properties.get("name").cloned().unwrap());
        assert_eq!(
            node.properties.get("tenant"),
            Some(&Value::Str("acme".into()))
        );
    }
    names.sort_by(|left, right| format!("{left:?}").cmp(&format!("{right:?}")));
    assert_eq!(
        names,
        vec![Value::Str("Alice".into()), Value::Str("Bob".into())]
    );
}

#[test]
fn create_node_binds_variable_in_row() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Create {
        patterns: vec![CreatePattern {
            nodes: vec![(Some(Identifier::new("n")), vec![Label::new("P")], vec![])],
            edges: vec![],
        }],
        child: None,
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    assert!(
        matches!(result.rows[0].get("n"), Some(Value::Node(_))),
        "created node should be bound in the output row"
    );
}

#[test]
fn create_edge_between_new_nodes() {
    use crate::ast::RelType;
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Create {
        patterns: vec![CreatePattern {
            nodes: vec![
                (Some(Identifier::new("a")), vec![Label::new("P")], vec![]),
                (Some(Identifier::new("b")), vec![Label::new("P")], vec![]),
            ],
            edges: vec![(
                Some(Identifier::new("r")),
                Identifier::new("a"),
                Identifier::new("b"),
                RelType::new("KNOWS"),
                Direction::Outgoing,
                vec![],
            )],
        }],
        child: None,
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.stats.nodes_created, 2);
    assert_eq!(result.stats.edges_created, 1);
}

#[test]
fn create_edge_with_properties() {
    use crate::ast::RelType;
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Create {
        patterns: vec![CreatePattern {
            nodes: vec![
                (Some(Identifier::new("a")), vec![Label::new("P")], vec![]),
                (Some(Identifier::new("b")), vec![Label::new("P")], vec![]),
            ],
            edges: vec![(
                Some(Identifier::new("r")),
                Identifier::new("a"),
                Identifier::new("b"),
                RelType::new("KNOWS"),
                Direction::Outgoing,
                vec![("since".to_string(), Expr::Integer(2020))],
            )],
        }],
        child: None,
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.stats.edges_created, 1);
    // Verify the edge has the property
    if let Some(Value::Edge(eid)) = result.rows[0].get("r") {
        let edge = store.get_edge("g", *eid).unwrap().unwrap();
        assert_eq!(edge.properties.get("since"), Some(&Value::Int(2020)));
    } else {
        panic!("expected edge in row");
    }
}

#[test]
fn create_multiple_nodes_in_single_pattern() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Create {
        patterns: vec![CreatePattern {
            nodes: vec![
                (Some(Identifier::new("a")), vec![Label::new("A")], vec![]),
                (Some(Identifier::new("b")), vec![Label::new("B")], vec![]),
                (Some(Identifier::new("c")), vec![Label::new("C")], vec![]),
            ],
            edges: vec![],
        }],
        child: None,
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.stats.nodes_created, 3);
    assert_eq!(result.rows.len(), 1);
    assert!(result.rows[0].contains_key("a"));
    assert!(result.rows[0].contains_key("b"));
    assert!(result.rows[0].contains_key("c"));
}

#[test]
fn create_reuses_pending_alias_in_same_pattern() {
    use crate::ast::RelType;

    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Create {
        patterns: vec![CreatePattern {
            nodes: vec![
                (Some(Identifier::new("a")), vec![Label::new("P")], vec![]),
                (Some(Identifier::new("a")), vec![Label::new("P")], vec![]),
            ],
            edges: vec![(
                Some(Identifier::new("r")),
                Identifier::new("a"),
                Identifier::new("a"),
                RelType::new("SELF"),
                Direction::Outgoing,
                vec![],
            )],
        }],
        child: None,
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();

    assert_eq!(
        result.stats.nodes_created, 1,
        "reusing the same alias in one CREATE pattern must not allocate two nodes"
    );
    assert_eq!(result.stats.edges_created, 1);

    let a_id = match result.rows[0].get("a") {
        Some(Value::Node(id)) => *id,
        other => panic!("expected alias a to bind a node, got {other:?}"),
    };
    let edge_id = match result.rows[0].get("r") {
        Some(Value::Edge(id)) => *id,
        other => panic!("expected alias r to bind an edge, got {other:?}"),
    };
    let edge = store.get_edge("g", edge_id).unwrap().unwrap();
    assert_eq!(edge.src, a_id);
    assert_eq!(edge.dst, a_id);
}

#[test]
fn create_later_node_properties_can_reference_earlier_created_alias() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Create {
        patterns: vec![CreatePattern {
            nodes: vec![
                (
                    Some(Identifier::new("a")),
                    vec![Label::new("Person")],
                    vec![("name".to_string(), Expr::Str("Alice".into()))],
                ),
                (
                    Some(Identifier::new("b")),
                    vec![Label::new("Person")],
                    vec![(
                        "friend".to_string(),
                        Expr::Property(
                            Box::new(Expr::Variable(Identifier::new("a"))),
                            "name".into(),
                        ),
                    )],
                ),
            ],
            edges: vec![],
        }],
        child: None,
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();

    assert_eq!(result.stats.nodes_created, 2);
    let b_id = match result.rows[0].get("b") {
        Some(Value::Node(id)) => *id,
        other => panic!("expected alias b to bind a node, got {other:?}"),
    };
    let b_node = store.get_node("g", b_id).unwrap().unwrap();
    assert_eq!(
        b_node.properties.get("friend"),
        Some(&Value::Str("Alice".into())),
        "later CREATE node properties must see aliases created earlier in the same pattern"
    );
}

#[test]
fn create_later_edge_properties_can_reference_earlier_created_alias() {
    use crate::ast::RelType;

    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Create {
        patterns: vec![CreatePattern {
            nodes: vec![
                (Some(Identifier::new("a")), vec![Label::new("P")], vec![]),
                (Some(Identifier::new("b")), vec![Label::new("P")], vec![]),
                (Some(Identifier::new("c")), vec![Label::new("P")], vec![]),
            ],
            edges: vec![
                (
                    Some(Identifier::new("r1")),
                    Identifier::new("a"),
                    Identifier::new("b"),
                    RelType::new("KNOWS"),
                    Direction::Outgoing,
                    vec![("weight".to_string(), Expr::Integer(7))],
                ),
                (
                    Some(Identifier::new("r2")),
                    Identifier::new("b"),
                    Identifier::new("c"),
                    RelType::new("FOLLOWS"),
                    Direction::Outgoing,
                    vec![(
                        "copied".to_string(),
                        Expr::Property(
                            Box::new(Expr::Variable(Identifier::new("r1"))),
                            "weight".into(),
                        ),
                    )],
                ),
            ],
        }],
        child: None,
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();

    assert_eq!(result.stats.edges_created, 2);
    let r2_id = match result.rows[0].get("r2") {
        Some(Value::Edge(id)) => *id,
        other => panic!("expected alias r2 to bind an edge, got {other:?}"),
    };
    let r2 = store.get_edge("g", r2_id).unwrap().unwrap();
    assert_eq!(
        r2.properties.get("copied"),
        Some(&Value::Int(7)),
        "later CREATE edge properties must see aliases created earlier in the same pattern"
    );
}

#[test]
fn create_incoming_edge_flips_direction() {
    use crate::ast::RelType;
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Create {
        patterns: vec![CreatePattern {
            nodes: vec![
                (Some(Identifier::new("a")), vec![Label::new("P")], vec![]),
                (Some(Identifier::new("b")), vec![Label::new("P")], vec![]),
            ],
            edges: vec![(
                Some(Identifier::new("r")),
                Identifier::new("a"),
                Identifier::new("b"),
                RelType::new("R"),
                Direction::Incoming, // a <-[:R]- b, stored as b -> a
                vec![],
            )],
        }],
        child: None,
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.stats.edges_created, 1);
    // Verify the edge goes from b to a (src=b, dst=a)
    let a_id = match result.rows[0].get("a") {
        Some(Value::Node(id)) => *id,
        _ => panic!("expected node a"),
    };
    let b_id = match result.rows[0].get("b") {
        Some(Value::Node(id)) => *id,
        _ => panic!("expected node b"),
    };
    // b should have outgoing edge, a should have incoming
    let out_edges = store.out_edge_ids("g", b_id).unwrap();
    assert_eq!(out_edges.len(), 1);
    let in_edges = store.in_edge_ids("g", a_id).unwrap();
    assert_eq!(in_edges.len(), 1);
}

#[test]
fn create_reuses_existing_bound_node() {
    let store = make_store();
    // Pre-create a node
    let existing = store
        .create_node("g", vec!["P".to_string()], IndexMap::new())
        .unwrap();

    let executor = QueryExecutor::new(&store);
    // Scan existing node, then CREATE (n)-[:R]->(m)
    // n is already bound from the scan, so it should be reused
    let plan = Operator::Create {
        patterns: vec![CreatePattern {
            nodes: vec![
                (Some(Identifier::new("n")), vec![Label::new("P")], vec![]),
                (Some(Identifier::new("m")), vec![Label::new("Q")], vec![]),
            ],
            edges: vec![],
        }],
        child: Some(Box::new(Operator::NodeScan {
            label: Some(Label::new("P")),
            alias: Identifier::new("n"),
        })),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    // n was already bound, so only m is new
    assert_eq!(result.stats.nodes_created, 1);
    // Verify n is still the original node
    assert_eq!(result.rows[0].get("n"), Some(&Value::Node(existing)));
}
