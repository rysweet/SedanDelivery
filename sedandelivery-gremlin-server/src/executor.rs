//! Bridge from Gremlin text queries to cypher-core execution.
//!
//! Flow: text query → parse_traversal → GremlinCompiler → QueryExecutor → GraphSON v3

use std::collections::HashMap;

use cypher_core::error::{ExecutionError, GqlError, StorageError};
use cypher_core::executor::{ExecutionBudget, QueryExecutor};
use cypher_core::storage::GraphStore;
use gremlin_core::compiler::{CompileError, GremlinCompiler};
use gremlin_core::graphson::to_graphson;
use gremlin_core::text_parser::parse_traversal;
use gremlin_core::types::GremlinValue;

/// Execute a Gremlin text query against the store.
///
/// Parses the traversal string, compiles to an operator tree, executes,
/// and converts results to GraphSON v3 JSON values.
pub fn execute_gremlin_text(
    store: &dyn GraphStore,
    graph: &str,
    gremlin: &str,
    bindings: &HashMap<String, serde_json::Value>,
) -> Result<Vec<serde_json::Value>, GqlError> {
    ensure_graph_exists(store, graph)?;

    // Convert bindings to GremlinValues for parameter resolution
    let resolved_bindings: HashMap<String, GremlinValue> = bindings
        .iter()
        .map(|(k, v)| (k.clone(), gremlin_core::graphson::from_graphson(v)))
        .collect();

    // Parse text traversal into steps
    let steps = parse_traversal(gremlin, &|name| resolved_bindings.get(name).cloned())
        .map_err(|e| gremlin_err(format!("parse error: {e}")))?;

    // Compile steps to operator tree
    let mut compiler = GremlinCompiler::new();
    let operator = match compiler.compile(&steps) {
        Ok(op) => op,
        Err(CompileError::UnsupportedStep(step)) => {
            return Err(gremlin_err(format!("unsupported step: {step}")));
        }
        Err(e) => return Err(gremlin_err(e.to_string())),
    };

    tracing::debug!(?operator, "compiled gremlin text to operator tree");

    // Execute
    let executor = QueryExecutor::new(store).with_gremlin_mode();
    let mut budget = ExecutionBudget::default_budget();
    let result = executor.execute(graph, &operator, &mut budget)?;

    // Convert to GraphSON v3
    let single_col = result.columns.len() == 1;
    let graphson_results: Vec<serde_json::Value> = result
        .rows
        .iter()
        .map(|row| {
            if single_col {
                let col = &result.columns[0];
                row.get(col.as_str())
                    .map(|v| to_graphson(&value_to_gremlin(v)))
                    .unwrap_or(serde_json::Value::Null)
            } else {
                let obj: serde_json::Map<String, serde_json::Value> = result
                    .columns
                    .iter()
                    .map(|col| {
                        let v = row
                            .get(col.as_str())
                            .map(|v| to_graphson(&value_to_gremlin(v)))
                            .unwrap_or(serde_json::Value::Null);
                        (col.clone(), v)
                    })
                    .collect();
                serde_json::Value::Object(obj)
            }
        })
        .collect();

    Ok(graphson_results)
}

/// Convert a cypher-core `Value` to a `GremlinValue`.
fn value_to_gremlin(val: &cypher_core::types::Value) -> GremlinValue {
    use cypher_core::types::Value;
    match val {
        Value::Null => GremlinValue::Null,
        Value::Bool(b) => GremlinValue::Bool(*b),
        Value::Int(n) => GremlinValue::Integer(*n),
        Value::Float(f) => GremlinValue::Float(*f),
        Value::Str(s) => GremlinValue::String(s.to_string()),
        Value::List(items) => GremlinValue::List(items.iter().map(value_to_gremlin).collect()),
        Value::Map(map) => {
            let entries = map
                .iter()
                .map(|(k, v)| (k.to_string(), value_to_gremlin(v)))
                .collect();
            GremlinValue::Map(entries)
        }
        Value::Node(nid) => GremlinValue::Vertex {
            id: nid.0 as i64,
            label: "vertex".to_string(),
            properties: indexmap::IndexMap::new(),
        },
        Value::Edge(eid) => GremlinValue::Edge {
            id: eid.0 as i64,
            label: "edge".to_string(),
            in_v: 0,
            out_v: 0,
            properties: indexmap::IndexMap::new(),
        },
        Value::Path { nodes, edges: _ } => {
            let objects = nodes
                .iter()
                .map(|nid| GremlinValue::Vertex {
                    id: nid.0 as i64,
                    label: "vertex".to_string(),
                    properties: indexmap::IndexMap::new(),
                })
                .collect();
            GremlinValue::Path {
                labels: Vec::new(),
                objects,
            }
        }
        other => GremlinValue::String(format!("{other:?}")),
    }
}

fn ensure_graph_exists(store: &dyn GraphStore, graph: &str) -> Result<(), GqlError> {
    if store.graph_exists(graph).map_err(GqlError::from)? {
        return Ok(());
    }
    match store.create_graph(graph) {
        Ok(()) | Err(StorageError::GraphAlreadyExists { .. }) => Ok(()),
        Err(err) => Err(GqlError::from(err)),
    }
}

fn gremlin_err(msg: impl Into<String>) -> GqlError {
    GqlError::Execution(ExecutionError::UnsupportedFeature {
        feature: msg.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cypher_core::storage::{EdgeId, InMemoryGraphStore, NodeId};
    use cypher_core::types::Value;

    // ── value_to_gremlin conversion ─────────────────────────────────────

    #[test]
    fn value_to_gremlin_integer() {
        let gv = value_to_gremlin(&Value::Int(42));
        assert_eq!(gv, GremlinValue::Integer(42));
    }

    #[test]
    fn value_to_gremlin_string() {
        let gv = value_to_gremlin(&Value::Str("hello".into()));
        assert_eq!(gv, GremlinValue::String("hello".to_string()));
    }

    #[test]
    fn value_to_gremlin_null() {
        let gv = value_to_gremlin(&Value::Null);
        assert_eq!(gv, GremlinValue::Null);
    }

    #[test]
    fn value_to_gremlin_bool() {
        assert_eq!(
            value_to_gremlin(&Value::Bool(true)),
            GremlinValue::Bool(true)
        );
        assert_eq!(
            value_to_gremlin(&Value::Bool(false)),
            GremlinValue::Bool(false)
        );
    }

    #[test]
    fn value_to_gremlin_float() {
        let gv = value_to_gremlin(&Value::Float(1.5));
        assert_eq!(gv, GremlinValue::Float(1.5));
    }

    #[test]
    fn value_to_gremlin_list() {
        let list = Value::List(vec![Value::Int(1), Value::Str("two".into())]);
        let gv = value_to_gremlin(&list);
        assert_eq!(
            gv,
            GremlinValue::List(vec![
                GremlinValue::Integer(1),
                GremlinValue::String("two".to_string()),
            ])
        );
    }

    #[test]
    fn value_to_gremlin_map() {
        let mut map = indexmap::IndexMap::new();
        map.insert("key".into(), Value::Int(99));
        let gv = value_to_gremlin(&Value::Map(map));
        match gv {
            GremlinValue::Map(entries) => {
                assert_eq!(entries.len(), 1);
                assert_eq!(entries.get("key"), Some(&GremlinValue::Integer(99)));
            }
            other => panic!("expected Map, got {other:?}"),
        }
    }

    #[test]
    fn value_to_gremlin_node() {
        let gv = value_to_gremlin(&Value::Node(NodeId(42)));
        match gv {
            GremlinValue::Vertex { id, label, .. } => {
                assert_eq!(id, 42);
                assert_eq!(label, "vertex");
            }
            other => panic!("expected Vertex, got {other:?}"),
        }
    }

    #[test]
    fn value_to_gremlin_edge() {
        let gv = value_to_gremlin(&Value::Edge(EdgeId(7)));
        match gv {
            GremlinValue::Edge { id, label, .. } => {
                assert_eq!(id, 7);
                assert_eq!(label, "edge");
            }
            other => panic!("expected Edge, got {other:?}"),
        }
    }

    #[test]
    fn value_to_gremlin_path() {
        let gv = value_to_gremlin(&Value::Path {
            nodes: vec![NodeId(1), NodeId(2)],
            edges: vec![EdgeId(10)],
        });
        match gv {
            GremlinValue::Path { labels, objects } => {
                assert!(labels.is_empty());
                assert_eq!(objects.len(), 2);
            }
            other => panic!("expected Path, got {other:?}"),
        }
    }

    // ── ensure_graph_exists ─────────────────────────────────────────────

    #[test]
    fn ensure_graph_creates_missing_graph() {
        let store = InMemoryGraphStore::new();
        assert!(!store.graph_exists("test_g").unwrap());
        ensure_graph_exists(&store, "test_g").unwrap();
        assert!(store.graph_exists("test_g").unwrap());
    }

    #[test]
    fn ensure_graph_idempotent() {
        let store = InMemoryGraphStore::new();
        ensure_graph_exists(&store, "g1").unwrap();
        ensure_graph_exists(&store, "g1").unwrap(); // second call should not fail
        assert!(store.graph_exists("g1").unwrap());
    }

    // ── execute_gremlin_text integration ────────────────────────────────

    #[test]
    fn execute_gremlin_count_empty_graph() {
        let store = InMemoryGraphStore::new();
        let result = execute_gremlin_text(&store, "default", "g.V().count()", &HashMap::new());
        let data = result.unwrap();
        // Empty graph should return count of 0
        assert_eq!(data.len(), 1);
    }

    #[test]
    fn execute_gremlin_add_vertex() {
        let store = InMemoryGraphStore::new();
        let result = execute_gremlin_text(
            &store,
            "default",
            r#"g.addV("person").property("name","alice")"#,
            &HashMap::new(),
        );
        assert!(result.is_ok(), "addV should succeed: {result:?}");
        let data = result.unwrap();
        assert_eq!(data.len(), 1, "addV should return the new vertex");
    }

    #[test]
    fn execute_gremlin_invalid_query_returns_error() {
        let store = InMemoryGraphStore::new();
        let result = execute_gremlin_text(&store, "default", "g.INVALID_STEP()", &HashMap::new());
        assert!(result.is_err(), "invalid step should produce an error");
    }

    #[test]
    fn execute_gremlin_with_bindings() {
        let store = InMemoryGraphStore::new();
        // Create a vertex first
        execute_gremlin_text(
            &store,
            "default",
            r#"g.addV("person").property("name","bob")"#,
            &HashMap::new(),
        )
        .unwrap();

        // Query with bindings
        let mut bindings = HashMap::new();
        bindings.insert("name".to_string(), serde_json::json!("bob"));
        let result = execute_gremlin_text(
            &store,
            "default",
            r#"g.V().has("person","name",name)"#,
            &bindings,
        );
        assert!(
            result.is_ok(),
            "query with bindings should succeed: {result:?}"
        );
    }

    #[test]
    fn execute_gremlin_creates_graph_on_demand() {
        let store = InMemoryGraphStore::new();
        assert!(!store.graph_exists("new_graph").unwrap());
        let result = execute_gremlin_text(&store, "new_graph", "g.V().count()", &HashMap::new());
        assert!(result.is_ok());
        assert!(store.graph_exists("new_graph").unwrap());
    }
}
