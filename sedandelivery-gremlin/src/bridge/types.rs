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
}
