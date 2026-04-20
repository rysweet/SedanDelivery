//! Type conversions between cypher-core `Value` / `QueryResult` and GraphSON v3 JSON.

use cypher_core::executor::QueryResult;
use cypher_core::types::Value;

/// Convert a full `QueryResult` into a vec of GraphSON JSON values.
///
/// Each row is emitted as a JSON object keyed by column name; single-column
/// results are unwrapped to bare values (matching TinkerPop convention).
pub fn query_result_to_graphson(result: &QueryResult) -> Vec<serde_json::Value> {
    let single_col = result.columns.len() == 1;

    result
        .rows
        .iter()
        .map(|row| {
            if single_col {
                let col = &result.columns[0];
                row.get(col.as_str())
                    .map(value_to_graphson)
                    .unwrap_or(serde_json::Value::Null)
            } else {
                let obj: serde_json::Map<String, serde_json::Value> = result
                    .columns
                    .iter()
                    .map(|col| {
                        let v = row
                            .get(col.as_str())
                            .map(value_to_graphson)
                            .unwrap_or(serde_json::Value::Null);
                        (col.clone(), v)
                    })
                    .collect();
                serde_json::Value::Object(obj)
            }
        })
        .collect()
}

/// Convert a cypher-core `Value` to its GraphSON v3 JSON representation.
pub fn value_to_graphson(val: &Value) -> serde_json::Value {
    match val {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Int(n) => serde_json::json!({
            "@type": "g:Int64",
            "@value": n
        }),
        Value::Float(f) => serde_json::json!({
            "@type": "g:Double",
            "@value": f
        }),
        Value::Str(s) => serde_json::Value::String(s.to_string()),
        Value::List(items) => {
            let arr: Vec<serde_json::Value> = items.iter().map(value_to_graphson).collect();
            serde_json::json!({
                "@type": "g:List",
                "@value": arr
            })
        }
        Value::Map(map) => {
            let entries: Vec<serde_json::Value> = map
                .iter()
                .flat_map(|(k, v)| {
                    vec![
                        serde_json::Value::String(k.to_string()),
                        value_to_graphson(v),
                    ]
                })
                .collect();
            serde_json::json!({
                "@type": "g:Map",
                "@value": entries
            })
        }
        Value::Node(node_id) => {
            serde_json::json!({
                "@type": "g:Vertex",
                "@value": {
                    "id": { "@type": "g:Int64", "@value": node_id.0 },
                    "label": "vertex"
                }
            })
        }
        Value::Edge(edge_id) => {
            serde_json::json!({
                "@type": "g:Edge",
                "@value": {
                    "id": { "@type": "g:Int64", "@value": edge_id.0 },
                    "label": "edge"
                }
            })
        }
        Value::Path { nodes, edges: _ } => {
            let labels: Vec<serde_json::Value> = Vec::new();
            let objects: Vec<serde_json::Value> = nodes
                .iter()
                .map(|nid| value_to_graphson(&Value::Node(*nid)))
                .collect();
            serde_json::json!({
                "@type": "g:Path",
                "@value": {
                    "labels": labels,
                    "objects": objects
                }
            })
        }
        // Fall back to string representation for types not yet mapped
        // (Date, LocalTime, Duration, Point, Bytes, etc.).
        other => serde_json::Value::String(format!("{other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_is_typed() {
        let v = value_to_graphson(&Value::Int(42));
        assert_eq!(v["@type"], "g:Int64");
        assert_eq!(v["@value"], 42);
    }

    #[test]
    fn string_is_plain() {
        let v = value_to_graphson(&Value::Str("hello".into()));
        assert_eq!(v, serde_json::Value::String("hello".into()));
    }

    #[test]
    fn null_is_null() {
        let v = value_to_graphson(&Value::Null);
        assert!(v.is_null());
    }

    #[test]
    fn list_is_typed() {
        let v = value_to_graphson(&Value::List(vec![Value::Int(1), Value::Int(2)]));
        assert_eq!(v["@type"], "g:List");
        assert_eq!(v["@value"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn bool_is_plain() {
        assert_eq!(
            value_to_graphson(&Value::Bool(true)),
            serde_json::json!(true)
        );
        assert_eq!(
            value_to_graphson(&Value::Bool(false)),
            serde_json::json!(false)
        );
    }

    #[test]
    fn float_is_typed() {
        let v = value_to_graphson(&Value::Float(3.5));
        assert_eq!(v["@type"], "g:Double");
        assert_eq!(v["@value"], 3.5);
    }

    #[test]
    fn map_is_typed_with_entries() {
        let mut m = indexmap::IndexMap::new();
        m.insert("a".to_string().into_boxed_str(), Value::Int(1));
        m.insert("b".to_string().into_boxed_str(), Value::Str("x".into()));
        let v = value_to_graphson(&Value::Map(m));
        assert_eq!(v["@type"], "g:Map");
        // Map encodes as flat [k1, v1, k2, v2, ...]
        let entries = v["@value"].as_array().unwrap();
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0], serde_json::Value::String("a".into()));
        assert_eq!(entries[2], serde_json::Value::String("b".into()));
    }

    #[test]
    fn node_and_edge_are_typed() {
        use cypher_core::storage::{EdgeId, NodeId};
        let n = value_to_graphson(&Value::Node(NodeId(7)));
        assert_eq!(n["@type"], "g:Vertex");
        assert_eq!(n["@value"]["id"]["@value"], 7);
        let e = value_to_graphson(&Value::Edge(EdgeId(11)));
        assert_eq!(e["@type"], "g:Edge");
        assert_eq!(e["@value"]["id"]["@value"], 11);
        assert_eq!(e["@value"]["label"], "edge");
    }

    #[test]
    fn path_is_typed() {
        use cypher_core::storage::{EdgeId, NodeId};
        let p = Value::Path {
            nodes: vec![NodeId(1), NodeId(2)],
            edges: vec![EdgeId(10)],
        };
        let v = value_to_graphson(&p);
        assert_eq!(v["@type"], "g:Path");
        let objects = v["@value"]["objects"].as_array().unwrap();
        assert_eq!(objects.len(), 2);
        assert_eq!(objects[0]["@type"], "g:Vertex");
        assert!(v["@value"]["labels"].as_array().unwrap().is_empty());
    }

    #[test]
    fn unmapped_variant_falls_back_to_debug_string() {
        // Bytes is not explicitly mapped — should serialise as a debug string.
        let v = value_to_graphson(&Value::Bytes(vec![1, 2, 3]));
        assert!(v.is_string(), "expected fallback string, got {v:?}");
        let s = v.as_str().unwrap();
        // Debug formatting should mention the variant name.
        assert!(s.contains("Bytes"), "fallback should embed Debug repr: {s}");
    }

    #[test]
    fn query_result_single_column_unwraps() {
        let mut row = std::collections::HashMap::new();
        row.insert("count".to_string(), Value::Int(5));
        let qr = QueryResult {
            columns: vec!["count".to_string()],
            rows: vec![row],
            stats: cypher_core::executor::MutationStats::default(),
            trailing_error: None,
        };
        let out = query_result_to_graphson(&qr);
        assert_eq!(out.len(), 1);
        // Single-column path returns bare typed value (not an object).
        assert_eq!(out[0]["@type"], "g:Int64");
        assert_eq!(out[0]["@value"], 5);
    }

    #[test]
    fn query_result_multi_column_emits_objects() {
        let mut row = std::collections::HashMap::new();
        row.insert("a".to_string(), Value::Int(1));
        row.insert("b".to_string(), Value::Str("x".into()));
        let qr = QueryResult {
            columns: vec!["a".to_string(), "b".to_string()],
            rows: vec![row],
            stats: cypher_core::executor::MutationStats::default(),
            trailing_error: None,
        };
        let out = query_result_to_graphson(&qr);
        assert_eq!(out.len(), 1);
        let obj = out[0].as_object().unwrap();
        assert_eq!(obj["a"]["@type"], "g:Int64");
        assert_eq!(obj["b"], serde_json::Value::String("x".into()));
    }

    #[test]
    fn query_result_missing_column_yields_null() {
        // Single column requested but not present in row -> Null.
        let row = std::collections::HashMap::new();
        let qr = QueryResult {
            columns: vec!["missing".to_string()],
            rows: vec![row],
            stats: cypher_core::executor::MutationStats::default(),
            trailing_error: None,
        };
        let out = query_result_to_graphson(&qr);
        assert_eq!(out.len(), 1);
        assert!(out[0].is_null());
    }

    #[test]
    fn query_result_empty_rows() {
        let qr = QueryResult {
            columns: vec!["x".to_string()],
            rows: vec![],
            stats: cypher_core::executor::MutationStats::default(),
            trailing_error: None,
        };
        assert!(query_result_to_graphson(&qr).is_empty());
    }
}
