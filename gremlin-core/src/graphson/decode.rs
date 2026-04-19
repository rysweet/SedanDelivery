//! GraphSON v3 typed-JSON decoding (`serde_json::Value` → `GremlinValue`).

use crate::types::GremlinValue;
use indexmap::IndexMap;

/// Deserialize a GraphSON v3 typed JSON value into a `GremlinValue`.
pub fn from_graphson(json: &serde_json::Value) -> GremlinValue {
    if json.is_null() {
        return GremlinValue::Null;
    }
    if let Some(b) = json.as_bool() {
        return GremlinValue::Bool(b);
    }
    if let Some(s) = json.as_str() {
        return GremlinValue::String(s.to_string());
    }
    // Bare number (not wrapped)
    if let Some(n) = json.as_number() {
        if let Some(i) = n.as_i64() {
            return GremlinValue::Integer(i);
        }
        return GremlinValue::Float(n.as_f64().unwrap_or(0.0));
    }
    // Bare array
    if let Some(arr) = json.as_array() {
        return GremlinValue::List(arr.iter().map(from_graphson).collect());
    }

    let obj = match json.as_object() {
        Some(o) => o,
        None => return GremlinValue::Null,
    };

    let type_tag = obj.get("@type").and_then(|t| t.as_str());
    let inner = obj.get("@value");

    match (type_tag, inner) {
        (Some("g:Int32" | "g:Int64"), Some(v)) => GremlinValue::Integer(v.as_i64().unwrap_or(0)),
        (Some("g:Float" | "g:Double"), Some(v)) => GremlinValue::Float(v.as_f64().unwrap_or(0.0)),
        (Some("g:List"), Some(v)) => {
            let items = v
                .as_array()
                .map(|a| a.iter().map(from_graphson).collect())
                .unwrap_or_default();
            GremlinValue::List(items)
        }
        (Some("g:Map"), Some(v)) => decode_map(v),
        (Some("g:Vertex"), Some(v)) => decode_vertex(v),
        (Some("g:Edge"), Some(v)) => decode_edge(v),
        (Some("g:Path"), Some(v)) => decode_path(v),
        _ => {
            // Unknown typed value or plain object — treat as map
            let map = obj
                .iter()
                .map(|(k, v)| (k.clone(), from_graphson(v)))
                .collect();
            GremlinValue::Map(map)
        }
    }
}

fn decode_map(v: &serde_json::Value) -> GremlinValue {
    let flat = v.as_array().cloned().unwrap_or_default();
    let mut map = IndexMap::new();
    let mut i = 0;
    while i + 1 < flat.len() {
        let key = match from_graphson(&flat[i]) {
            GremlinValue::String(s) => s,
            other => format!("{other:?}"),
        };
        let val = from_graphson(&flat[i + 1]);
        map.insert(key, val);
        i += 2;
    }
    GremlinValue::Map(map)
}

fn decode_id(v: &serde_json::Value, key: &str) -> i64 {
    v.get(key)
        .map(from_graphson)
        .and_then(|gv| match gv {
            GremlinValue::Integer(i) => Some(i),
            _ => None,
        })
        .unwrap_or(0)
}

fn decode_label(v: &serde_json::Value) -> String {
    v.get("label")
        .and_then(|l| l.as_str())
        .unwrap_or("")
        .to_string()
}

fn decode_properties(v: &serde_json::Value) -> IndexMap<String, GremlinValue> {
    v.get("properties")
        .and_then(|p| p.as_object())
        .map(|obj| {
            obj.iter()
                .map(|(k, v)| (k.clone(), from_graphson(v)))
                .collect()
        })
        .unwrap_or_default()
}

fn decode_vertex(v: &serde_json::Value) -> GremlinValue {
    GremlinValue::Vertex {
        id: decode_id(v, "id"),
        label: decode_label(v),
        properties: decode_properties(v),
    }
}

fn decode_edge(v: &serde_json::Value) -> GremlinValue {
    GremlinValue::Edge {
        id: decode_id(v, "id"),
        label: decode_label(v),
        in_v: decode_id(v, "inV"),
        out_v: decode_id(v, "outV"),
        properties: decode_properties(v),
    }
}

fn decode_path(v: &serde_json::Value) -> GremlinValue {
    let labels = v
        .get("labels")
        .and_then(|l| serde_json::from_value(l.clone()).ok())
        .unwrap_or_default();
    let objects = v
        .get("objects")
        .and_then(|o| o.as_array())
        .map(|a| a.iter().map(from_graphson).collect())
        .unwrap_or_default();
    GremlinValue::Path { labels, objects }
}
