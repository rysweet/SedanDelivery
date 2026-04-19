//! Gremlin request/response message types (GraphSON v3 envelope).

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
