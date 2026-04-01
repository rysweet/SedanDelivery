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

    #[test]
    fn value_to_gremlin_integer() {
        let gv = value_to_gremlin(&cypher_core::types::Value::Int(42));
        assert_eq!(gv, GremlinValue::Integer(42));
    }

    #[test]
    fn value_to_gremlin_string() {
        let gv = value_to_gremlin(&cypher_core::types::Value::Str("hello".into()));
        assert_eq!(gv, GremlinValue::String("hello".to_string()));
    }

    #[test]
    fn value_to_gremlin_null() {
        let gv = value_to_gremlin(&cypher_core::types::Value::Null);
        assert_eq!(gv, GremlinValue::Null);
    }
}
