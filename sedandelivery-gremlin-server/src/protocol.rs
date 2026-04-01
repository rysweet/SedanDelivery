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
    fn unsupported_mime_rejected() {
        let mime = b"application/vnd.graphbinary-v1.0";
        let mut frame = vec![mime.len() as u8];
        frame.extend_from_slice(mime);
        frame.extend_from_slice(b"{}");
        assert!(parse_binary_frame(&frame).is_err());
    }

    #[test]
    fn success_response_serializes() {
        let resp = ResponseMessage::success(Uuid::nil(), serde_json::json!([42]));
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["status"]["code"], 200);
        assert_eq!(json["result"]["data"], serde_json::json!([42]));
    }

    #[test]
    fn error_response_includes_message() {
        let resp = ResponseMessage::error(Uuid::nil(), 500, "boom".into());
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["status"]["code"], 500);
        assert_eq!(json["status"]["message"], "boom");
    }
}
