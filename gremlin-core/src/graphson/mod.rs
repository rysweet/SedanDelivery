//! GraphSON v3 request/response message types and serialization.
//!
//! This module is split into focused submodules:
//! - [`messages`] -- request/response envelope structs.
//! - [`encode`] -- `to_graphson` typed-JSON encoder.
//! - [`decode`] -- `from_graphson` typed-JSON decoder.

mod decode;
mod encode;
mod messages;

pub use decode::from_graphson;
pub use encode::to_graphson;
pub use messages::{RequestMessage, ResponseMessage, ResponseResult, ResponseStatus, StatusCode};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::GremlinValue;
    use indexmap::IndexMap;
    use uuid::Uuid;

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

    // ── Path round-trip ─────────────────────────────────────────────────────

    #[test]
    fn graphson_path_round_trip() {
        let gv = GremlinValue::Path {
            labels: vec![vec!["a".into()], vec![]],
            objects: vec![
                GremlinValue::Vertex {
                    id: 1,
                    label: "person".into(),
                    properties: IndexMap::new(),
                },
                GremlinValue::Integer(42),
            ],
        };
        let json = to_graphson(&gv);
        assert_eq!(json["@type"], "g:Path");
        let back = from_graphson(&json);
        assert_eq!(back, gv);
    }

    // ── Bare number deserialization ─────────────────────────────────────────

    #[test]
    fn from_graphson_bare_integer() {
        let json = serde_json::json!(99);
        assert_eq!(from_graphson(&json), GremlinValue::Integer(99));
    }

    #[test]
    fn from_graphson_bare_float() {
        let json = serde_json::json!(1.25);
        if let GremlinValue::Float(f) = from_graphson(&json) {
            assert!((f - 1.25).abs() < f64::EPSILON);
        } else {
            panic!("expected Float");
        }
    }

    #[test]
    fn from_graphson_bare_array() {
        let json = serde_json::json!([1, "two", true]);
        let gv = from_graphson(&json);
        if let GremlinValue::List(items) = gv {
            assert_eq!(items.len(), 3);
            assert_eq!(items[0], GremlinValue::Integer(1));
            assert_eq!(items[1], GremlinValue::String("two".into()));
            assert_eq!(items[2], GremlinValue::Bool(true));
        } else {
            panic!("expected List");
        }
    }

    // ── Unknown typed value → Map fallback ──────────────────────────────────

    #[test]
    fn from_graphson_unknown_type_becomes_map() {
        let json = serde_json::json!({"@type": "g:Unknown", "@value": {"foo": 1}});
        let gv = from_graphson(&json);
        // Should fall through to generic map handling
        if let GremlinValue::Map(m) = &gv {
            assert!(m.contains_key("@type"));
            assert!(m.contains_key("@value"));
        } else {
            panic!("expected Map fallback, got {gv:?}");
        }
    }

    #[test]
    fn from_graphson_plain_object_becomes_map() {
        let json = serde_json::json!({"name": "alice", "age": 30});
        let gv = from_graphson(&json);
        if let GremlinValue::Map(m) = gv {
            assert_eq!(m.get("name"), Some(&GremlinValue::String("alice".into())));
            assert_eq!(m.get("age"), Some(&GremlinValue::Integer(30)));
        } else {
            panic!("expected Map");
        }
    }

    // ── Int32 type tag ──────────────────────────────────────────────────────

    #[test]
    fn from_graphson_int32() {
        let json = serde_json::json!({"@type": "g:Int32", "@value": 7});
        assert_eq!(from_graphson(&json), GremlinValue::Integer(7));
    }

    #[test]
    fn from_graphson_float_type() {
        let json = serde_json::json!({"@type": "g:Float", "@value": 1.5});
        if let GremlinValue::Float(f) = from_graphson(&json) {
            assert!((f - 1.5).abs() < f64::EPSILON);
        } else {
            panic!("expected Float");
        }
    }

    // ── Edge with properties ────────────────────────────────────────────────

    #[test]
    fn graphson_edge_with_properties_round_trip() {
        let mut props = IndexMap::new();
        props.insert("weight".to_string(), GremlinValue::Float(0.5));
        let gv = GremlinValue::Edge {
            id: 10,
            label: "knows".into(),
            in_v: 2,
            out_v: 1,
            properties: props,
        };
        let json = to_graphson(&gv);
        let back = from_graphson(&json);
        assert_eq!(back, gv);
    }

    // ── Vertex with properties ──────────────────────────────────────────────

    #[test]
    fn graphson_vertex_with_properties_round_trip() {
        let mut props = IndexMap::new();
        props.insert("name".to_string(), GremlinValue::String("bob".into()));
        props.insert("age".to_string(), GremlinValue::Integer(25));
        let gv = GremlinValue::Vertex {
            id: 5,
            label: "person".into(),
            properties: props,
        };
        let json = to_graphson(&gv);
        let back = from_graphson(&json);
        assert_eq!(back, gv);
    }

    // ── Map with non-string keys ────────────────────────────────────────────

    #[test]
    fn graphson_map_nested_round_trip() {
        let mut inner = IndexMap::new();
        inner.insert("x".to_string(), GremlinValue::Integer(1));
        let mut outer = IndexMap::new();
        outer.insert("nested".to_string(), GremlinValue::Map(inner));
        let gv = GremlinValue::Map(outer);
        let json = to_graphson(&gv);
        let back = from_graphson(&json);
        assert_eq!(back, gv);
    }

    // ── Status code constants ───────────────────────────────────────────────

    #[test]
    fn status_code_constants() {
        assert_eq!(StatusCode::SUCCESS.0, 200);
        assert_eq!(StatusCode::NO_CONTENT.0, 204);
        assert_eq!(StatusCode::PARTIAL_CONTENT.0, 206);
        assert_eq!(StatusCode::UNAUTHORIZED.0, 401);
        assert_eq!(StatusCode::FORBIDDEN.0, 403);
        assert_eq!(StatusCode::AUTHENTICATE.0, 407);
        assert_eq!(StatusCode::SERVER_ERROR.0, 500);
        assert_eq!(StatusCode::SCRIPT_EVALUATION_ERROR.0, 597);
    }

    // ── ResponseMessage serde ───────────────────────────────────────────────

    #[test]
    fn response_message_round_trip_serde() {
        let id = Uuid::new_v4();
        let resp = ResponseMessage::success(
            id,
            vec![
                GremlinValue::String("hello".into()),
                GremlinValue::Integer(42),
            ],
        );
        let json = serde_json::to_string(&resp).unwrap();
        let decoded: ResponseMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.request_id, id);
        assert_eq!(decoded.result.data.len(), 2);
        assert_eq!(decoded.status.code, StatusCode::SUCCESS);
    }

    #[test]
    fn response_message_error_round_trip() {
        let id = Uuid::new_v4();
        let resp = ResponseMessage::error(id, StatusCode::SCRIPT_EVALUATION_ERROR, "syntax error");
        let json = serde_json::to_string(&resp).unwrap();
        let decoded: ResponseMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.status.code.0, 597);
        assert_eq!(decoded.status.message, "syntax error");
        assert!(decoded.result.data.is_empty());
    }

    // ── RequestMessage fields ───────────────────────────────────────────────

    #[test]
    fn request_message_new_generates_unique_ids() {
        let r1 = RequestMessage::new("eval", "", serde_json::json!({}));
        let r2 = RequestMessage::new("eval", "", serde_json::json!({}));
        assert_ne!(r1.request_id, r2.request_id);
    }
}
