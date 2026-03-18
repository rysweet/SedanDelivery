//! Gremlin ResponseMessage serialization (GraphSON v3 framing).
//!
//! Response format:
//! ```json
//! {
//!   "requestId": "...",
//!   "status": {"code": 200, "message": "", "attributes": {}},
//!   "result": {"data": [...], "meta": {}}
//! }
//! ```

use serde::Serialize;
use uuid::Uuid;

/// Gremlin status codes.
#[derive(Debug, Clone, Copy)]
pub enum StatusCode {
    Success = 200,
    NoContent = 204,
    Unauthorized = 401,
    #[allow(dead_code)]
    Forbidden = 403,
    Authenticate = 407,
    ServerError = 500,
    ScriptEvaluationError = 597,
}

impl StatusCode {
    fn as_u16(self) -> u16 {
        self as u16
    }
}

/// Outgoing Gremlin response message.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseMessage {
    pub request_id: Uuid,
    pub status: ResponseStatus,
    pub result: ResponseResult,
}

#[derive(Debug, Serialize)]
pub struct ResponseStatus {
    pub code: u16,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub message: String,
    pub attributes: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct ResponseResult {
    pub data: serde_json::Value,
    pub meta: serde_json::Map<String, serde_json::Value>,
}

impl ResponseMessage {
    /// Build a success response with data.
    pub fn success(request_id: Uuid, data: serde_json::Value) -> Self {
        Self {
            request_id,
            status: ResponseStatus {
                code: StatusCode::Success.as_u16(),
                message: String::new(),
                attributes: serde_json::Map::new(),
            },
            result: ResponseResult {
                data,
                meta: serde_json::Map::new(),
            },
        }
    }

    /// Build a no-content response.
    pub fn no_content(request_id: Uuid) -> Self {
        Self {
            request_id,
            status: ResponseStatus {
                code: StatusCode::NoContent.as_u16(),
                message: String::new(),
                attributes: serde_json::Map::new(),
            },
            result: ResponseResult {
                data: serde_json::Value::Null,
                meta: serde_json::Map::new(),
            },
        }
    }

    /// Build an error response.
    pub fn error(request_id: Uuid, code: StatusCode, message: String) -> Self {
        Self {
            request_id,
            status: ResponseStatus {
                code: code.as_u16(),
                message,
                attributes: serde_json::Map::new(),
            },
            result: ResponseResult {
                data: serde_json::Value::Null,
                meta: serde_json::Map::new(),
            },
        }
    }

    /// Build an authentication challenge (407).
    pub fn authenticate_challenge() -> Self {
        Self {
            request_id: Uuid::nil(),
            status: ResponseStatus {
                code: StatusCode::Authenticate.as_u16(),
                message: String::new(),
                attributes: serde_json::Map::new(),
            },
            result: ResponseResult {
                data: serde_json::Value::Null,
                meta: serde_json::Map::new(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_response_serialises() {
        let id = Uuid::nil();
        let resp = ResponseMessage::success(id, serde_json::json!([42]));
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["status"]["code"], 200);
        assert_eq!(json["result"]["data"], serde_json::json!([42]));
    }

    #[test]
    fn error_response_includes_message() {
        let id = Uuid::nil();
        let resp = ResponseMessage::error(id, StatusCode::ServerError, "boom".into());
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["status"]["code"], 500);
        assert_eq!(json["status"]["message"], "boom");
    }

    #[test]
    fn authenticate_challenge_is_407() {
        let resp = ResponseMessage::authenticate_challenge();
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["status"]["code"], 407);
    }
}
