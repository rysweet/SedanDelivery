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
                    .map(|v| to_graphson(&value_to_gremlin(v, store, graph)))
                    .unwrap_or(serde_json::Value::Null)
            } else {
                let obj: serde_json::Map<String, serde_json::Value> = result
                    .columns
                    .iter()
                    .map(|col| {
                        let v = row
                            .get(col.as_str())
                            .map(|v| to_graphson(&value_to_gremlin(v, store, graph)))
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

/// Convert property values from cypher-core `Value` to `GremlinValue`.
fn convert_props(
    props: &indexmap::IndexMap<String, cypher_core::types::Value>,
    store: &dyn GraphStore,
    graph: &str,
) -> indexmap::IndexMap<String, GremlinValue> {
    props
        .iter()
        .map(|(k, v)| (k.clone(), value_to_gremlin(v, store, graph)))
        .collect()
}

/// Convert a cypher-core `Value` to a `GremlinValue`, resolving vertex/edge
/// data from the store so that labels and properties are included.
fn value_to_gremlin(
    val: &cypher_core::types::Value,
    store: &dyn GraphStore,
    graph: &str,
) -> GremlinValue {
    use cypher_core::types::Value;
    match val {
        Value::Null => GremlinValue::Null,
        Value::Bool(b) => GremlinValue::Bool(*b),
        Value::Int(n) => GremlinValue::Integer(*n),
        Value::Float(f) => GremlinValue::Float(*f),
        Value::Str(s) => GremlinValue::String(s.to_string()),
        Value::List(items) => GremlinValue::List(
            items
                .iter()
                .map(|v| value_to_gremlin(v, store, graph))
                .collect(),
        ),
        Value::Map(map) => {
            let entries = map
                .iter()
                .map(|(k, v)| (k.to_string(), value_to_gremlin(v, store, graph)))
                .collect();
            GremlinValue::Map(entries)
        }
        Value::Node(nid) => match store.get_node(graph, *nid) {
            Ok(Some(data)) => {
                let label = if data.labels.is_empty() {
                    "vertex".to_string()
                } else {
                    data.labels.join(":")
                };
                GremlinValue::Vertex {
                    id: nid.0 as i64,
                    label,
                    properties: convert_props(&data.properties, store, graph),
                }
            }
            _ => GremlinValue::Vertex {
                id: nid.0 as i64,
                label: "vertex".to_string(),
                properties: indexmap::IndexMap::new(),
            },
        },
        Value::Edge(eid) => match store.get_edge(graph, *eid) {
            Ok(Some(data)) => GremlinValue::Edge {
                id: eid.0 as i64,
                label: data.rel_type.clone(),
                in_v: data.dst.0 as i64,
                out_v: data.src.0 as i64,
                properties: convert_props(&data.properties, store, graph),
            },
            _ => GremlinValue::Edge {
                id: eid.0 as i64,
                label: "edge".to_string(),
                in_v: 0,
                out_v: 0,
                properties: indexmap::IndexMap::new(),
            },
        },
        Value::Path { nodes, edges } => {
            // Interleave nodes and edges: node[0], edge[0], node[1], edge[1], ...
            let mut objects = Vec::with_capacity(nodes.len() + edges.len());
            for (i, nid) in nodes.iter().enumerate() {
                objects.push(value_to_gremlin(&Value::Node(*nid), store, graph));
                if i < edges.len() {
                    objects.push(value_to_gremlin(&Value::Edge(edges[i]), store, graph));
                }
            }
            // One empty label set per path object (TinkerPop convention)
            let labels = (0..objects.len()).map(|_| Vec::new()).collect();
            GremlinValue::Path { labels, objects }
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

    /// Helper: create an InMemoryGraphStore with a graph already created.
    fn test_store(graph: &str) -> InMemoryGraphStore {
        let store = InMemoryGraphStore::new();
        store.create_graph(graph).unwrap();
        store
    }

    // ── value_to_gremlin: scalar conversions ────────────────────────────
    //
    // These pass a store + graph to `value_to_gremlin` even though scalars
    // don't need store access — validates the new signature works for the
    // no-lookup path.

    #[test]
    fn value_to_gremlin_integer() {
        let store = test_store("g");
        let gv = value_to_gremlin(&Value::Int(42), &store, "g");
        assert_eq!(gv, GremlinValue::Integer(42));
    }

    #[test]
    fn value_to_gremlin_string() {
        let store = test_store("g");
        let gv = value_to_gremlin(&Value::Str("hello".into()), &store, "g");
        assert_eq!(gv, GremlinValue::String("hello".to_string()));
    }

    #[test]
    fn value_to_gremlin_null() {
        let store = test_store("g");
        let gv = value_to_gremlin(&Value::Null, &store, "g");
        assert_eq!(gv, GremlinValue::Null);
    }

    #[test]
    fn value_to_gremlin_bool() {
        let store = test_store("g");
        assert_eq!(
            value_to_gremlin(&Value::Bool(true), &store, "g"),
            GremlinValue::Bool(true)
        );
        assert_eq!(
            value_to_gremlin(&Value::Bool(false), &store, "g"),
            GremlinValue::Bool(false)
        );
    }

    #[test]
    fn value_to_gremlin_float() {
        let store = test_store("g");
        let gv = value_to_gremlin(&Value::Float(1.5), &store, "g");
        assert_eq!(gv, GremlinValue::Float(1.5));
    }

    #[test]
    fn value_to_gremlin_list() {
        let store = test_store("g");
        let list = Value::List(vec![Value::Int(1), Value::Str("two".into())]);
        let gv = value_to_gremlin(&list, &store, "g");
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
        let store = test_store("g");
        let mut map = indexmap::IndexMap::new();
        map.insert("key".into(), Value::Int(99));
        let gv = value_to_gremlin(&Value::Map(map), &store, "g");
        match gv {
            GremlinValue::Map(entries) => {
                assert_eq!(entries.len(), 1);
                assert_eq!(entries.get("key"), Some(&GremlinValue::Integer(99)));
            }
            other => panic!("expected Map, got {other:?}"),
        }
    }

    // ── value_to_gremlin: vertex with real store data ───────────────────

    #[test]
    fn value_to_gremlin_node_resolves_label_from_store() {
        let store = test_store("g");
        let mut props = indexmap::IndexMap::new();
        props.insert("name".to_string(), Value::Str("alice".into()));
        let nid = store
            .create_node("g", vec!["person".to_string()], props)
            .unwrap();
        let gv = value_to_gremlin(&Value::Node(nid), &store, "g");
        match gv {
            GremlinValue::Vertex { id, label, .. } => {
                assert_eq!(id, nid.0 as i64);
                assert_eq!(
                    label, "person",
                    "label must come from store, not hardcoded 'vertex'"
                );
            }
            other => panic!("expected Vertex, got {other:?}"),
        }
    }

    #[test]
    fn value_to_gremlin_node_includes_properties() {
        let store = test_store("g");
        let mut props = indexmap::IndexMap::new();
        props.insert("name".to_string(), Value::Str("alice".into()));
        props.insert("age".to_string(), Value::Int(30));
        props.insert("score".to_string(), Value::Float(9.5));
        let nid = store
            .create_node("g", vec!["person".to_string()], props)
            .unwrap();
        let gv = value_to_gremlin(&Value::Node(nid), &store, "g");
        match gv {
            GremlinValue::Vertex { properties, .. } => {
                assert_eq!(properties.len(), 3, "all 3 properties must be present");
                assert_eq!(
                    properties.get("name"),
                    Some(&GremlinValue::String("alice".to_string()))
                );
                assert_eq!(properties.get("age"), Some(&GremlinValue::Integer(30)));
                assert_eq!(properties.get("score"), Some(&GremlinValue::Float(9.5)));
            }
            other => panic!("expected Vertex, got {other:?}"),
        }
    }

    #[test]
    fn value_to_gremlin_node_multiple_labels_joined() {
        let store = test_store("g");
        let nid = store
            .create_node(
                "g",
                vec!["person".to_string(), "employee".to_string()],
                indexmap::IndexMap::new(),
            )
            .unwrap();
        let gv = value_to_gremlin(&Value::Node(nid), &store, "g");
        match gv {
            GremlinValue::Vertex { label, .. } => {
                assert_eq!(
                    label, "person:employee",
                    "multiple labels must be colon-joined"
                );
            }
            other => panic!("expected Vertex, got {other:?}"),
        }
    }

    #[test]
    fn value_to_gremlin_node_empty_labels_fallback() {
        let store = test_store("g");
        let nid = store
            .create_node("g", vec![], indexmap::IndexMap::new())
            .unwrap();
        let gv = value_to_gremlin(&Value::Node(nid), &store, "g");
        match gv {
            GremlinValue::Vertex { label, .. } => {
                assert_eq!(label, "vertex", "empty labels must fall back to 'vertex'");
            }
            other => panic!("expected Vertex, got {other:?}"),
        }
    }

    #[test]
    fn value_to_gremlin_node_not_found_fallback() {
        let store = test_store("g");
        // NodeId 9999 doesn't exist in the store
        let gv = value_to_gremlin(&Value::Node(NodeId(9999)), &store, "g");
        match gv {
            GremlinValue::Vertex {
                id,
                label,
                properties,
            } => {
                assert_eq!(id, 9999);
                assert_eq!(label, "vertex", "missing node must fall back to 'vertex'");
                assert!(
                    properties.is_empty(),
                    "missing node must have empty properties"
                );
            }
            other => panic!("expected Vertex, got {other:?}"),
        }
    }

    // ── value_to_gremlin: edge with real store data ─────────────────────

    #[test]
    fn value_to_gremlin_edge_resolves_label_and_endpoints() {
        let store = test_store("g");
        let src = store
            .create_node("g", vec!["person".to_string()], indexmap::IndexMap::new())
            .unwrap();
        let dst = store
            .create_node("g", vec!["person".to_string()], indexmap::IndexMap::new())
            .unwrap();
        let eid = store
            .create_edge(
                "g",
                src,
                dst,
                "knows".to_string(),
                indexmap::IndexMap::new(),
            )
            .unwrap();
        let gv = value_to_gremlin(&Value::Edge(eid), &store, "g");
        match gv {
            GremlinValue::Edge {
                id,
                label,
                in_v,
                out_v,
                ..
            } => {
                assert_eq!(id, eid.0 as i64);
                assert_eq!(label, "knows", "edge label must come from store rel_type");
                assert_eq!(in_v, dst.0 as i64, "in_v must be destination node");
                assert_eq!(out_v, src.0 as i64, "out_v must be source node");
            }
            other => panic!("expected Edge, got {other:?}"),
        }
    }

    #[test]
    fn value_to_gremlin_edge_includes_properties() {
        let store = test_store("g");
        let src = store
            .create_node("g", vec!["person".to_string()], indexmap::IndexMap::new())
            .unwrap();
        let dst = store
            .create_node("g", vec!["person".to_string()], indexmap::IndexMap::new())
            .unwrap();
        let mut edge_props = indexmap::IndexMap::new();
        edge_props.insert("since".to_string(), Value::Int(2020));
        edge_props.insert("weight".to_string(), Value::Float(0.85));
        let eid = store
            .create_edge("g", src, dst, "knows".to_string(), edge_props)
            .unwrap();
        let gv = value_to_gremlin(&Value::Edge(eid), &store, "g");
        match gv {
            GremlinValue::Edge { properties, .. } => {
                assert_eq!(properties.len(), 2, "edge must have 2 properties");
                assert_eq!(properties.get("since"), Some(&GremlinValue::Integer(2020)));
                assert_eq!(properties.get("weight"), Some(&GremlinValue::Float(0.85)));
            }
            other => panic!("expected Edge, got {other:?}"),
        }
    }

    #[test]
    fn value_to_gremlin_edge_not_found_fallback() {
        let store = test_store("g");
        let gv = value_to_gremlin(&Value::Edge(EdgeId(9999)), &store, "g");
        match gv {
            GremlinValue::Edge {
                id,
                label,
                in_v,
                out_v,
                properties,
            } => {
                assert_eq!(id, 9999);
                assert_eq!(label, "edge", "missing edge must fall back to 'edge'");
                assert_eq!(in_v, 0, "missing edge must default in_v to 0");
                assert_eq!(out_v, 0, "missing edge must default out_v to 0");
                assert!(
                    properties.is_empty(),
                    "missing edge must have empty properties"
                );
            }
            other => panic!("expected Edge, got {other:?}"),
        }
    }

    // ── value_to_gremlin: path with real store data ─────────────────────

    #[test]
    fn value_to_gremlin_path_interleaves_nodes_and_edges() {
        let store = test_store("g");
        let mut props_a = indexmap::IndexMap::new();
        props_a.insert("name".to_string(), Value::Str("a".into()));
        let n1 = store
            .create_node("g", vec!["person".to_string()], props_a)
            .unwrap();
        let mut props_b = indexmap::IndexMap::new();
        props_b.insert("name".to_string(), Value::Str("b".into()));
        let n2 = store
            .create_node("g", vec!["person".to_string()], props_b)
            .unwrap();
        let mut edge_props = indexmap::IndexMap::new();
        edge_props.insert("since".to_string(), Value::Int(2021));
        let e1 = store
            .create_edge("g", n1, n2, "knows".to_string(), edge_props)
            .unwrap();

        let gv = value_to_gremlin(
            &Value::Path {
                nodes: vec![n1, n2],
                edges: vec![e1],
            },
            &store,
            "g",
        );
        match gv {
            GremlinValue::Path { labels, objects } => {
                // Interleaved: node, edge, node = 3 objects
                assert_eq!(objects.len(), 3, "path must interleave nodes and edges");
                // First object: vertex n1 with properties
                match &objects[0] {
                    GremlinValue::Vertex {
                        id,
                        properties,
                        label,
                        ..
                    } => {
                        assert_eq!(*id, n1.0 as i64);
                        assert_eq!(label, "person");
                        assert_eq!(
                            properties.get("name"),
                            Some(&GremlinValue::String("a".to_string()))
                        );
                    }
                    other => panic!("expected Vertex at index 0, got {other:?}"),
                }
                // Second object: edge e1 with properties
                match &objects[1] {
                    GremlinValue::Edge {
                        id,
                        label,
                        properties,
                        ..
                    } => {
                        assert_eq!(*id, e1.0 as i64);
                        assert_eq!(label, "knows");
                        assert_eq!(properties.get("since"), Some(&GremlinValue::Integer(2021)));
                    }
                    other => panic!("expected Edge at index 1, got {other:?}"),
                }
                // Third object: vertex n2 with properties
                match &objects[2] {
                    GremlinValue::Vertex {
                        id,
                        properties,
                        label,
                        ..
                    } => {
                        assert_eq!(*id, n2.0 as i64);
                        assert_eq!(label, "person");
                        assert_eq!(
                            properties.get("name"),
                            Some(&GremlinValue::String("b".to_string()))
                        );
                    }
                    other => panic!("expected Vertex at index 2, got {other:?}"),
                }
                // labels: one empty vec per object
                assert_eq!(
                    labels.len(),
                    3,
                    "labels must have one entry per path object"
                );
            }
            other => panic!("expected Path, got {other:?}"),
        }
    }

    #[test]
    fn value_to_gremlin_path_single_node_no_edges() {
        let store = test_store("g");
        let nid = store
            .create_node("g", vec!["thing".to_string()], indexmap::IndexMap::new())
            .unwrap();
        let gv = value_to_gremlin(
            &Value::Path {
                nodes: vec![nid],
                edges: vec![],
            },
            &store,
            "g",
        );
        match gv {
            GremlinValue::Path { objects, .. } => {
                assert_eq!(objects.len(), 1, "single-node path has 1 object");
                match &objects[0] {
                    GremlinValue::Vertex { label, .. } => {
                        assert_eq!(label, "thing");
                    }
                    other => panic!("expected Vertex, got {other:?}"),
                }
            }
            other => panic!("expected Path, got {other:?}"),
        }
    }

    // ── value_to_gremlin: property conversion helper ────────────────────

    #[test]
    fn value_to_gremlin_node_bool_property() {
        let store = test_store("g");
        let mut props = indexmap::IndexMap::new();
        props.insert("active".to_string(), Value::Bool(true));
        let nid = store
            .create_node("g", vec!["flag".to_string()], props)
            .unwrap();
        let gv = value_to_gremlin(&Value::Node(nid), &store, "g");
        match gv {
            GremlinValue::Vertex { properties, .. } => {
                assert_eq!(
                    properties.get("active"),
                    Some(&GremlinValue::Bool(true)),
                    "bool property must be converted"
                );
            }
            other => panic!("expected Vertex, got {other:?}"),
        }
    }

    #[test]
    fn value_to_gremlin_node_null_property() {
        let store = test_store("g");
        let mut props = indexmap::IndexMap::new();
        props.insert("maybe".to_string(), Value::Null);
        let nid = store
            .create_node("g", vec!["test".to_string()], props)
            .unwrap();
        let gv = value_to_gremlin(&Value::Node(nid), &store, "g");
        match gv {
            GremlinValue::Vertex { properties, .. } => {
                assert_eq!(
                    properties.get("maybe"),
                    Some(&GremlinValue::Null),
                    "null property must be converted"
                );
            }
            other => panic!("expected Vertex, got {other:?}"),
        }
    }

    #[test]
    fn value_to_gremlin_node_list_property() {
        let store = test_store("g");
        let mut props = indexmap::IndexMap::new();
        props.insert(
            "tags".to_string(),
            Value::List(vec![Value::Str("a".into()), Value::Str("b".into())]),
        );
        let nid = store
            .create_node("g", vec!["tagged".to_string()], props)
            .unwrap();
        let gv = value_to_gremlin(&Value::Node(nid), &store, "g");
        match gv {
            GremlinValue::Vertex { properties, .. } => {
                assert_eq!(
                    properties.get("tags"),
                    Some(&GremlinValue::List(vec![
                        GremlinValue::String("a".to_string()),
                        GremlinValue::String("b".to_string()),
                    ])),
                    "list property must be recursively converted"
                );
            }
            other => panic!("expected Vertex, got {other:?}"),
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
    fn execute_gremlin_add_vertex_has_properties_in_graphson() {
        let store = InMemoryGraphStore::new();
        let data = execute_gremlin_text(
            &store,
            "default",
            r#"g.addV("person").property("name","alice").property("age",30)"#,
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(data.len(), 1);
        let vertex = &data[0];
        // GraphSON v3 wraps in {"@type":"g:Vertex","@value":{...}}
        let inner = vertex.get("@value").unwrap_or(vertex);
        let props = inner.get("properties");
        assert!(
            props.is_some(),
            "vertex GraphSON must include 'properties' key; got: {vertex}"
        );
        let props = props.unwrap();
        assert!(
            !props.as_object().map_or(true, |m| m.is_empty()),
            "vertex properties must not be empty; got: {props}"
        );
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
