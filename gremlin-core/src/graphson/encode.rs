//! GraphSON v3 typed-JSON encoding (`GremlinValue` → `serde_json::Value`).

use crate::types::GremlinValue;
use indexmap::IndexMap;

/// Serialize a `GremlinValue` to GraphSON v3 typed JSON.
pub fn to_graphson(value: &GremlinValue) -> serde_json::Value {
    match value {
        GremlinValue::Null => serde_json::Value::Null,
        GremlinValue::Bool(b) => serde_json::Value::Bool(*b),
        GremlinValue::Integer(i) => serde_json::json!({
            "@type": "g:Int64",
            "@value": i
        }),
        GremlinValue::Float(f) => serde_json::json!({
            "@type": "g:Double",
            "@value": f
        }),
        GremlinValue::String(s) => serde_json::Value::String(s.clone()),
        GremlinValue::List(items) => {
            let typed_items: Vec<serde_json::Value> = items.iter().map(to_graphson).collect();
            serde_json::json!({
                "@type": "g:List",
                "@value": typed_items
            })
        }
        GremlinValue::Map(entries) => {
            let mut flat = Vec::with_capacity(entries.len() * 2);
            for (k, v) in entries {
                flat.push(serde_json::Value::String(k.clone()));
                flat.push(to_graphson(v));
            }
            serde_json::json!({
                "@type": "g:Map",
                "@value": flat
            })
        }
        GremlinValue::Vertex {
            id,
            label,
            properties,
        } => {
            let props: IndexMap<String, serde_json::Value> = properties
                .iter()
                .map(|(k, v)| (k.clone(), to_graphson(v)))
                .collect();
            serde_json::json!({
                "@type": "g:Vertex",
                "@value": {
                    "id": { "@type": "g:Int64", "@value": id },
                    "label": label,
                    "properties": props
                }
            })
        }
        GremlinValue::Edge {
            id,
            label,
            in_v,
            out_v,
            properties,
        } => {
            let props: IndexMap<String, serde_json::Value> = properties
                .iter()
                .map(|(k, v)| (k.clone(), to_graphson(v)))
                .collect();
            serde_json::json!({
                "@type": "g:Edge",
                "@value": {
                    "id": { "@type": "g:Int64", "@value": id },
                    "label": label,
                    "inV": { "@type": "g:Int64", "@value": in_v },
                    "outV": { "@type": "g:Int64", "@value": out_v },
                    "properties": props
                }
            })
        }
        GremlinValue::Path { labels, objects } => {
            let typed_objects: Vec<serde_json::Value> = objects.iter().map(to_graphson).collect();
            serde_json::json!({
                "@type": "g:Path",
                "@value": {
                    "labels": labels,
                    "objects": typed_objects
                }
            })
        }
    }
}
