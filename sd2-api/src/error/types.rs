//! Core error types: [`ApiError`], [`ErrorBody`], constructors, and the
//! Axum `IntoResponse` implementation.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

/// Wire format for all error responses: `{ "error": "..." }`.
#[derive(Debug, Serialize)]
pub struct ErrorBody {
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// A typed API error that knows its HTTP status code.
#[derive(Debug)]
pub struct ApiError {
    pub(super) status: StatusCode,
    pub(super) message: String,
    pub detail: Option<String>,
}

impl ApiError {
    pub fn new(status: StatusCode, msg: impl Into<String>) -> Self {
        Self {
            status,
            message: msg.into(),
            detail: None,
        }
    }

    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: msg.into(),
            detail: None,
        }
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: msg.into(),
            detail: None,
        }
    }

    pub fn conflict(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: msg.into(),
            detail: None,
        }
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: msg.into(),
            detail: None,
        }
    }

    pub fn unprocessable(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            message: msg.into(),
            detail: None,
        }
    }

    pub fn payload_too_large(code: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            message: code.into(),
            detail: Some(detail.into()),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorBody {
                error: self.message,
                message: self.detail,
            }),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_request_constructor() {
        let e = ApiError::bad_request("invalid input");
        assert_eq!(e.status, StatusCode::BAD_REQUEST);
        assert_eq!(e.message, "invalid input");
    }

    #[test]
    fn not_found_constructor() {
        let e = ApiError::not_found("not here");
        assert_eq!(e.status, StatusCode::NOT_FOUND);
        assert_eq!(e.message, "not here");
    }

    #[test]
    fn conflict_constructor() {
        let e = ApiError::conflict("already exists");
        assert_eq!(e.status, StatusCode::CONFLICT);
        assert_eq!(e.message, "already exists");
    }

    #[test]
    fn internal_constructor() {
        let e = ApiError::internal("boom");
        assert_eq!(e.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(e.message, "boom");
    }

    #[test]
    fn unprocessable_constructor() {
        let e = ApiError::unprocessable("bad entity");
        assert_eq!(e.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(e.message, "bad entity");
    }

    #[test]
    fn new_constructor_custom_status() {
        let e = ApiError::new(StatusCode::IM_A_TEAPOT, "teapot");
        assert_eq!(e.status, StatusCode::IM_A_TEAPOT);
        assert_eq!(e.message, "teapot");
    }
}
