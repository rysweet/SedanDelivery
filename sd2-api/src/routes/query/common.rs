//! Shared helpers for the query/mutate/explain/profile endpoints.

use std::collections::HashMap;
use std::sync::OnceLock;

use axum::http::StatusCode;

use cypher_core::executor::ExecutionBudget;
use cypher_core::serde_value::json_to_value;
use cypher_core::types::Value;

use crate::error::ApiError;

/// Maximum Cypher query string length in bytes. Default 1 MiB (1_048_576).
/// Override via `GQL_API_MAX_QUERY_LENGTH`.
pub(super) fn max_query_length() -> usize {
    static LIMIT: OnceLock<usize> = OnceLock::new();
    *LIMIT.get_or_init(|| {
        std::env::var("GQL_API_MAX_QUERY_LENGTH")
            .ok()
            .and_then(|v| match v.parse() {
                Ok(val) => Some(val),
                Err(_) => {
                    tracing::warn!(
                        "invalid value for GQL_API_MAX_QUERY_LENGTH: {v:?}, using default 1048576"
                    );
                    None
                }
            })
            .unwrap_or(1024 * 1024)
    })
}

/// Maximum query/mutate/explain body size in bytes. Default 1 MiB.
/// Override via `GQL_API_QUERY_BODY_LIMIT_BYTES`.
pub(super) fn query_body_limit() -> usize {
    static LIMIT: OnceLock<usize> = OnceLock::new();
    *LIMIT.get_or_init(|| {
        std::env::var("GQL_API_QUERY_BODY_LIMIT_BYTES")
            .ok()
            .and_then(|v| match v.parse() {
                Ok(val) => Some(val),
                Err(_) => {
                    tracing::warn!(
                        "invalid value for GQL_API_QUERY_BODY_LIMIT_BYTES: {v:?}, using default 1048576"
                    );
                    None
                }
            })
            .unwrap_or(1024 * 1024)
    })
}

/// Reject queries whose Cypher string exceeds `max_query_length()`. Returns 413.
pub(super) fn reject_oversized_query(cypher: &str) -> Result<(), ApiError> {
    let max = max_query_length();
    if cypher.len() > max {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "query length {} bytes exceeds maximum allowed {} bytes",
                cypher.len(),
                max
            ),
        ));
    }
    Ok(())
}

/// Convert optional JSON params map to `HashMap<String, Value>` for the executor.
pub(super) fn convert_params(
    params: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Result<HashMap<String, Value>, ApiError> {
    let Some(map) = params else {
        return Ok(HashMap::new());
    };
    let mut result = HashMap::with_capacity(map.len());
    for (k, v) in map {
        let val = json_to_value(v).ok_or_else(|| {
            ApiError::bad_request(format!("unsupported value for parameter '{k}'"))
        })?;
        result.insert(k.clone(), val);
    }
    Ok(result)
}

pub(super) fn configured_budget() -> ExecutionBudget {
    ExecutionBudget::from_config(cypher_core::config::global())
}

/// Capture the current trace span ID for propagation as a request_id.
pub(super) fn current_request_id() -> Option<String> {
    tracing::Span::current()
        .id()
        .map(|id| format!("{:x}", id.into_u64()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn convert_params_none_returns_empty() {
        let result = convert_params(None).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn convert_params_empty_map() {
        let map = serde_json::Map::new();
        let result = convert_params(Some(&map)).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn convert_params_string_value() {
        let mut map = serde_json::Map::new();
        map.insert("name".to_string(), json!("Alice"));
        let result = convert_params(Some(&map)).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result["name"], Value::Str("Alice".into()));
    }

    #[test]
    fn convert_params_int_value() {
        let mut map = serde_json::Map::new();
        map.insert("age".to_string(), json!(30));
        let result = convert_params(Some(&map)).unwrap();
        assert_eq!(result["age"], Value::Int(30));
    }

    #[test]
    fn convert_params_mixed_types() {
        let mut map = serde_json::Map::new();
        map.insert("name".to_string(), json!("Bob"));
        map.insert("age".to_string(), json!(25));
        map.insert("active".to_string(), json!(true));
        map.insert("score".to_string(), json!(std::f64::consts::PI));
        let result = convert_params(Some(&map)).unwrap();
        assert_eq!(result.len(), 4);
        assert_eq!(result["name"], Value::Str("Bob".into()));
        assert_eq!(result["age"], Value::Int(25));
        assert_eq!(result["active"], Value::Bool(true));
        assert_eq!(result["score"], Value::Float(std::f64::consts::PI));
    }

    #[test]
    fn convert_params_null_value() {
        let mut map = serde_json::Map::new();
        map.insert("x".to_string(), json!(null));
        let result = convert_params(Some(&map)).unwrap();
        assert_eq!(result["x"], Value::Null);
    }

    #[test]
    fn convert_params_list_value() {
        let mut map = serde_json::Map::new();
        map.insert("ids".to_string(), json!([1, 2, 3]));
        let result = convert_params(Some(&map)).unwrap();
        assert!(matches!(result["ids"], Value::List(_)));
    }
}
