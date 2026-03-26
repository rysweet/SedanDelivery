use std::collections::HashMap;
use std::time::Duration;

use indexmap::IndexMap;

use gql_core::ast::{Expr, Identifier, Label};
use gql_core::executor::{ExecutionBudget, QueryExecutor};
use gql_core::planner::{CreatePattern, Operator};
use gql_core::procedure::ProcedureRegistry;
use gql_core::storage::{GraphStore, InMemoryGraphStore};
use gql_core::types::Value;

fn make_store() -> InMemoryGraphStore {
    let store = InMemoryGraphStore::new();
    store.create_graph("test").unwrap();
    store
}

fn unlimited() -> ExecutionBudget {
    ExecutionBudget::new(Duration::from_secs(3600), usize::MAX)
}

#[test]
fn parameterized_unwind_create_does_not_copy_batch_params_into_rows() {
    let store = make_store();
    let mut params = HashMap::new();

    let mut first = IndexMap::new();
    first.insert("name".into(), Value::Str("Alice".into()));
    let mut second = IndexMap::new();
    second.insert("name".into(), Value::Str("Bob".into()));

    params.insert(
        "resources".to_string(),
        Value::List(vec![Value::Map(first), Value::Map(second)]),
    );
    params.insert("tenant".to_string(), Value::Str("acme".into()));

    let registry = ProcedureRegistry::new();
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

    let mut budget = unlimited();
    let result = executor.execute("test", &plan, &mut budget).unwrap();

    assert_eq!(result.stats.nodes_created, 2);
    assert_eq!(result.rows.len(), 2);
    for row in &result.rows {
        assert!(row.contains_key("resource"));
        assert!(row.contains_key("n"));
        assert!(!row.contains_key("$resources"));
        assert!(!row.contains_key("$tenant"));
    }

    let mut names = Vec::new();
    for node_id in store.find_nodes_by_label("test", "Person").unwrap() {
        let node = store.get_node("test", node_id).unwrap().unwrap();
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
