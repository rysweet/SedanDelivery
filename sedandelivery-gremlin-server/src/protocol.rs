//! Gremlin WebSocket protocol handling.
//!
//! Handles binary frame MIME prefix parsing (TinkerPop 3.x wire format),
//! request deserialization, and response serialization in GraphSON v3.

use serde::Deserialize;
use uuid::Uuid;

/// Supported MIME types for Gremlin serialization.
const GRAPHSON_V3_MIME: &str = "application/vnd.gremlin-v3.0+json";

/// Incoming Gremlin request message.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestMessage {
    pub request_id: Uuid,
    pub op: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub processor: String,
    #[serde(default)]
    pub args: serde_json::Map<String, serde_json::Value>,
}

/// Outgoing Gremlin response message.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseMessage {
    pub request_id: Uuid,
    pub status: ResponseStatus,
    pub result: ResponseResult,
}

#[derive(Debug, serde::Serialize)]
pub struct ResponseStatus {
    pub code: u16,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub message: String,
    pub attributes: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, serde::Serialize)]
pub struct ResponseResult {
    pub data: serde_json::Value,
    pub meta: serde_json::Map<String, serde_json::Value>,
}

impl ResponseMessage {
    pub fn success(request_id: Uuid, data: serde_json::Value) -> Self {
        Self {
            request_id,
            status: ResponseStatus {
                code: 200,
                message: String::new(),
                attributes: serde_json::Map::new(),
            },
            result: ResponseResult {
                data,
                meta: serde_json::Map::new(),
            },
        }
    }

    pub fn no_content(request_id: Uuid) -> Self {
        Self {
            request_id,
            status: ResponseStatus {
                code: 204,
                message: String::new(),
                attributes: serde_json::Map::new(),
            },
            result: ResponseResult {
                data: serde_json::Value::Null,
                meta: serde_json::Map::new(),
            },
        }
    }

    pub fn error(request_id: Uuid, code: u16, message: String) -> Self {
        Self {
            request_id,
            status: ResponseStatus {
                code,
                message,
                attributes: serde_json::Map::new(),
            },
            result: ResponseResult {
                data: serde_json::Value::Null,
                meta: serde_json::Map::new(),
            },
        }
    }
}

/// Extract JSON payload from a binary WebSocket frame.
///
/// TinkerPop 3.x binary frames start with:
///   [1 byte: MIME type length] [N bytes: MIME string] [remaining: JSON payload]
///
/// Returns the JSON payload as a string slice, or an error if the MIME type
/// is not supported.
pub fn parse_binary_frame(data: &[u8]) -> Result<&str, String> {
    if data.is_empty() {
        return Err("empty binary frame".to_string());
    }

    let mime_len = data[0] as usize;
    if data.len() < 1 + mime_len {
        return Err("binary frame too short for MIME header".to_string());
    }

    let mime = std::str::from_utf8(&data[1..1 + mime_len])
        .map_err(|e| format!("invalid MIME string: {e}"))?;

    // Accept GraphSON v3 and untyped JSON
    match mime {
        GRAPHSON_V3_MIME | "application/json" => {}
        other => return Err(format!("unsupported MIME type: {other}")),
    }

    let payload = &data[1 + mime_len..];
    std::str::from_utf8(payload).map_err(|e| format!("invalid UTF-8 in payload: {e}"))
}

/// Parse a JSON string into a `RequestMessage`.
pub fn parse_request(json: &str) -> Result<RequestMessage, String> {
    serde_json::from_str(json).map_err(|e| format!("malformed request: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Binary frame parsing ────────────────────────────────────────────

    #[test]
    fn parse_binary_graphson_v3() {
        let mime = GRAPHSON_V3_MIME.as_bytes();
        let payload = b"{\"requestId\":\"00000000-0000-0000-0000-000000000001\",\"op\":\"eval\",\"args\":{\"gremlin\":\"g.V()\"}}";
        let mut frame = vec![mime.len() as u8];
        frame.extend_from_slice(mime);
        frame.extend_from_slice(payload);

        let result = parse_binary_frame(&frame).unwrap();
        let req = parse_request(result).unwrap();
        assert_eq!(req.op, "eval");
    }

    #[test]
    fn parse_binary_application_json() {
        let mime = b"application/json";
        let payload =
            b"{\"requestId\":\"00000000-0000-0000-0000-000000000002\",\"op\":\"eval\",\"args\":{}}";
        let mut frame = vec![mime.len() as u8];
        frame.extend_from_slice(mime);
        frame.extend_from_slice(payload);

        let result = parse_binary_frame(&frame).unwrap();
        assert!(result.contains("requestId"));
    }

    #[test]
    fn unsupported_mime_rejected() {
        let mime = b"application/vnd.graphbinary-v1.0";
        let mut frame = vec![mime.len() as u8];
        frame.extend_from_slice(mime);
        frame.extend_from_slice(b"{}");
        let err = parse_binary_frame(&frame).unwrap_err();
        assert!(err.contains("unsupported MIME type"));
    }

    #[test]
    fn empty_binary_frame_rejected() {
        let err = parse_binary_frame(&[]).unwrap_err();
        assert!(err.contains("empty binary frame"));
    }

    #[test]
    fn truncated_binary_frame_rejected() {
        // MIME length says 50 bytes but frame is only 3 bytes total
        let frame = vec![50, b'a', b'b'];
        let err = parse_binary_frame(&frame).unwrap_err();
        assert!(err.contains("too short"));
    }

    // ── Request parsing ─────────────────────────────────────────────────

    #[test]
    fn parse_text_request() {
        let json = r#"{"requestId":"00000000-0000-0000-0000-000000000001","op":"eval","args":{"gremlin":"g.V().count()"}}"#;
        let req = parse_request(json).unwrap();
        assert_eq!(req.op, "eval");
        assert_eq!(
            req.args.get("gremlin").and_then(|v| v.as_str()),
            Some("g.V().count()")
        );
    }

    #[test]
    fn parse_request_with_bindings() {
        let json = r#"{
            "requestId": "00000000-0000-0000-0000-000000000003",
            "op": "eval",
            "args": {
                "gremlin": "g.V(x)",
                "bindings": {"x": 1}
            }
        }"#;
        let req = parse_request(json).unwrap();
        assert_eq!(req.op, "eval");
        let bindings = req.args.get("bindings").unwrap().as_object().unwrap();
        assert_eq!(bindings.get("x").unwrap(), &serde_json::json!(1));
    }

    #[test]
    fn parse_request_with_aliases() {
        let json = r#"{
            "requestId": "00000000-0000-0000-0000-000000000004",
            "op": "eval",
            "args": {
                "gremlin": "g.V()",
                "aliases": {"g": "mygraph"}
            }
        }"#;
        let req = parse_request(json).unwrap();
        let aliases = req.args.get("aliases").unwrap().as_object().unwrap();
        assert_eq!(aliases.get("g").unwrap(), "mygraph");
    }

    #[test]
    fn parse_request_close_op() {
        let json = r#"{"requestId":"00000000-0000-0000-0000-000000000005","op":"close","args":{}}"#;
        let req = parse_request(json).unwrap();
        assert_eq!(req.op, "close");
    }

    #[test]
    fn parse_request_missing_request_id_fails() {
        let json = r#"{"op":"eval","args":{"gremlin":"g.V()"}}"#;
        assert!(parse_request(json).is_err());
    }

    #[test]
    fn parse_request_malformed_json_fails() {
        assert!(parse_request("{not json}").is_err());
    }

    #[test]
    fn parse_request_defaults_empty_processor() {
        let json = r#"{"requestId":"00000000-0000-0000-0000-000000000001","op":"eval","args":{}}"#;
        let req = parse_request(json).unwrap();
        assert_eq!(req.processor, "");
    }

    #[test]
    fn parse_request_defaults_empty_args() {
        let json = r#"{"requestId":"00000000-0000-0000-0000-000000000001","op":"eval"}"#;
        let req = parse_request(json).unwrap();
        assert!(req.args.is_empty());
    }

    // ── Response serialization ──────────────────────────────────────────

    #[test]
    fn success_response_serializes() {
        let resp = ResponseMessage::success(Uuid::nil(), serde_json::json!([42]));
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["status"]["code"], 200);
        assert_eq!(json["result"]["data"], serde_json::json!([42]));
    }

    #[test]
    fn success_response_omits_empty_message() {
        let resp = ResponseMessage::success(Uuid::nil(), serde_json::json!([]));
        let json_str = serde_json::to_string(&resp).unwrap();
        // "message" should be absent because of skip_serializing_if
        assert!(!json_str.contains("\"message\""));
    }

    #[test]
    fn error_response_includes_message() {
        let resp = ResponseMessage::error(Uuid::nil(), 500, "boom".into());
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["status"]["code"], 500);
        assert_eq!(json["status"]["message"], "boom");
    }

    #[test]
    fn no_content_response_has_204() {
        let id = Uuid::parse_str("12345678-1234-1234-1234-123456789012").unwrap();
        let resp = ResponseMessage::no_content(id);
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["status"]["code"], 204);
        assert_eq!(json["result"]["data"], serde_json::Value::Null);
        assert_eq!(json["requestId"], id.to_string());
    }

    #[test]
    fn error_response_preserves_status_codes() {
        for code in [401, 403, 497, 498, 499, 500, 597, 598] {
            let resp = ResponseMessage::error(Uuid::nil(), code, "test".into());
            let json = serde_json::to_value(&resp).unwrap();
            assert_eq!(json["status"]["code"], code);
        }
    }

    #[test]
    fn response_request_id_roundtrips() {
        let id = Uuid::new_v4();
        let resp = ResponseMessage::success(id, serde_json::json!(null));
        let json = serde_json::to_value(&resp).unwrap();
        let parsed_id: Uuid = serde_json::from_value(json["requestId"].clone()).unwrap();
        assert_eq!(parsed_id, id);
    }

    #[test]
    fn success_with_vertex_data() {
        let data = serde_json::json!([{
            "id": {"@type": "g:Int64", "@value": 1},
            "label": "person",
            "type": "vertex"
        }]);
        let resp = ResponseMessage::success(Uuid::nil(), data.clone());
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["result"]["data"], data);
    }
}
