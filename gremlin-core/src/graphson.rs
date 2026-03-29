//! GraphSON v3 request/response message types and serialization.

use crate::types::GremlinValue;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A Gremlin request message (GraphSON v3 format).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestMessage {
    pub request_id: Uuid,
    pub op: String,
    pub processor: String,
    pub args: serde_json::Value,
}

impl RequestMessage {
    pub fn new(
        op: impl Into<String>,
        processor: impl Into<String>,
        args: serde_json::Value,
    ) -> Self {
        Self {
            request_id: Uuid::new_v4(),
            op: op.into(),
            processor: processor.into(),
            args,
        }
    }
}

/// Status codes for Gremlin responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusCode(pub u16);

impl StatusCode {
    pub const SUCCESS: Self = Self(200);
    pub const NO_CONTENT: Self = Self(204);
    pub const PARTIAL_CONTENT: Self = Self(206);
    pub const UNAUTHORIZED: Self = Self(401);
    pub const FORBIDDEN: Self = Self(403);
    pub const AUTHENTICATE: Self = Self(407);
    pub const SERVER_ERROR: Self = Self(500);
    pub const SCRIPT_EVALUATION_ERROR: Self = Self(597);
}

/// The status portion of a Gremlin response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResponseStatus {
    pub code: StatusCode,
    pub message: String,
    #[serde(default)]
    pub attributes: IndexMap<String, serde_json::Value>,
}

/// The result portion of a Gremlin response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResponseResult {
    pub data: Vec<GremlinValue>,
    #[serde(default)]
    pub meta: IndexMap<String, serde_json::Value>,
}

/// A complete Gremlin response message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseMessage {
    pub request_id: Uuid,
    pub status: ResponseStatus,
    pub result: ResponseResult,
}

impl ResponseMessage {
    /// Create a success response with the given data.
    pub fn success(request_id: Uuid, data: Vec<GremlinValue>) -> Self {
        Self {
            request_id,
            status: ResponseStatus {
                code: StatusCode::SUCCESS,
                message: String::new(),
                attributes: IndexMap::new(),
            },
            result: ResponseResult {
                data,
                meta: IndexMap::new(),
            },
        }
    }

    /// Create an error response.
    pub fn error(request_id: Uuid, code: StatusCode, message: impl Into<String>) -> Self {
        Self {
            request_id,
            status: ResponseStatus {
                code,
                message: message.into(),
                attributes: IndexMap::new(),
            },
            result: ResponseResult {
                data: Vec::new(),
                meta: IndexMap::new(),
            },
        }
    }
}

// ── GraphSON v3 type wrappers ────────────────────────────────────────────────

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
        (Some("g:Map"), Some(v)) => {
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
        (Some("g:Vertex"), Some(v)) => {
            let id = v
                .get("id")
                .map(from_graphson)
                .and_then(|gv| match gv {
                    GremlinValue::Integer(i) => Some(i),
                    _ => None,
                })
                .unwrap_or(0);
            let label = v
                .get("label")
                .and_then(|l| l.as_str())
                .unwrap_or("")
                .to_string();
            let properties = v
                .get("properties")
                .and_then(|p| p.as_object())
                .map(|obj| {
                    obj.iter()
                        .map(|(k, v)| (k.clone(), from_graphson(v)))
                        .collect()
                })
                .unwrap_or_default();
            GremlinValue::Vertex {
                id,
                label,
                properties,
            }
        }
        (Some("g:Edge"), Some(v)) => {
            let id = v
                .get("id")
                .map(from_graphson)
                .and_then(|gv| match gv {
                    GremlinValue::Integer(i) => Some(i),
                    _ => None,
                })
                .unwrap_or(0);
            let label = v
                .get("label")
                .and_then(|l| l.as_str())
                .unwrap_or("")
                .to_string();
            let in_v = v
                .get("inV")
                .map(from_graphson)
                .and_then(|gv| match gv {
                    GremlinValue::Integer(i) => Some(i),
                    _ => None,
                })
                .unwrap_or(0);
            let out_v = v
                .get("outV")
                .map(from_graphson)
                .and_then(|gv| match gv {
                    GremlinValue::Integer(i) => Some(i),
                    _ => None,
                })
                .unwrap_or(0);
            let properties = v
                .get("properties")
                .and_then(|p| p.as_object())
                .map(|obj| {
                    obj.iter()
                        .map(|(k, v)| (k.clone(), from_graphson(v)))
                        .collect()
                })
                .unwrap_or_default();
            GremlinValue::Edge {
                id,
                label,
                in_v,
                out_v,
                properties,
            }
        }
        (Some("g:Path"), Some(v)) => {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_message_round_trip() {
        let req = RequestMessage::new(
            "bytecode",
            "traversal",
            serde_json::json!({"gremlin": "g.V()"}),
        );
        let json = serde_json::to_string(&req).unwrap();
        let decoded: RequestMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.op, "bytecode");
        assert_eq!(decoded.processor, "traversal");
        assert_eq!(decoded.request_id, req.request_id);
    }

    #[test]
    fn response_message_success() {
        let id = Uuid::new_v4();
        let resp = ResponseMessage::success(id, vec![GremlinValue::Integer(42)]);
        let json = serde_json::to_string(&resp).unwrap();
        let decoded: ResponseMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.status.code, StatusCode::SUCCESS);
        assert_eq!(decoded.result.data.len(), 1);
    }

    #[test]
    fn response_message_error() {
        let id = Uuid::new_v4();
        let resp = ResponseMessage::error(id, StatusCode::SERVER_ERROR, "something went wrong");
        assert_eq!(resp.status.code.0, 500);
        assert_eq!(resp.status.message, "something went wrong");
        assert!(resp.result.data.is_empty());
    }

    #[test]
    fn graphson_int64_round_trip() {
        let gv = GremlinValue::Integer(42);
        let json = to_graphson(&gv);
        assert_eq!(json["@type"], "g:Int64");
        assert_eq!(json["@value"], 42);
        let back = from_graphson(&json);
        assert_eq!(back, gv);
    }

    #[test]
    #[allow(clippy::approx_constant)]
    fn graphson_double_round_trip() {
        let gv = GremlinValue::Float(3.14);
        let json = to_graphson(&gv);
        assert_eq!(json["@type"], "g:Double");
        let back = from_graphson(&json);
        if let GremlinValue::Float(f) = back {
            assert!((f - 3.14).abs() < f64::EPSILON);
        } else {
            panic!("expected Float");
        }
    }

    #[test]
    fn graphson_vertex_round_trip() {
        let gv = GremlinValue::Vertex {
            id: 1,
            label: "person".into(),
            properties: IndexMap::new(),
        };
        let json = to_graphson(&gv);
        assert_eq!(json["@type"], "g:Vertex");
        let back = from_graphson(&json);
        assert_eq!(back, gv);
    }

    #[test]
    fn graphson_edge_round_trip() {
        let gv = GremlinValue::Edge {
            id: 7,
            label: "knows".into(),
            in_v: 2,
            out_v: 1,
            properties: IndexMap::new(),
        };
        let json = to_graphson(&gv);
        assert_eq!(json["@type"], "g:Edge");
        let back = from_graphson(&json);
        assert_eq!(back, gv);
    }

    #[test]
    fn graphson_list_round_trip() {
        let gv = GremlinValue::List(vec![
            GremlinValue::Integer(1),
            GremlinValue::String("hello".into()),
        ]);
        let json = to_graphson(&gv);
        assert_eq!(json["@type"], "g:List");
        let back = from_graphson(&json);
        assert_eq!(back, gv);
    }

    #[test]
    fn graphson_map_round_trip() {
        let mut map = IndexMap::new();
        map.insert("name".to_string(), GremlinValue::String("alice".into()));
        let gv = GremlinValue::Map(map);
        let json = to_graphson(&gv);
        assert_eq!(json["@type"], "g:Map");
        let back = from_graphson(&json);
        assert_eq!(back, gv);
    }

    #[test]
    fn graphson_null_and_bool() {
        assert_eq!(from_graphson(&serde_json::Value::Null), GremlinValue::Null);
        assert_eq!(
            from_graphson(&serde_json::Value::Bool(true)),
            GremlinValue::Bool(true)
        );
    }

    #[test]
    fn graphson_string_passthrough() {
        let json = serde_json::json!("hello");
        assert_eq!(from_graphson(&json), GremlinValue::String("hello".into()));
        assert_eq!(to_graphson(&GremlinValue::String("hello".into())), json);
    }
}
