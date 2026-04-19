//! Request and response models for neighbor traversal endpoints.

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

#[derive(Deserialize)]
pub struct NeighborParams {
    /// "in", "out", or "both" (default: "both")
    pub direction: Option<String>,
    /// Comma-separated relationship types to filter
    pub types: Option<String>,
    #[serde(default = "default_neighbor_limit")]
    pub limit: usize,
}

/// Return the default neighbor limit when the caller does not supply a `limit` parameter.
///
/// Reads `GQL_API_DEFAULT_NEIGHBOR_LIMIT` from the environment on first call (default `50`).
fn default_neighbor_limit() -> usize {
    use std::sync::OnceLock;
    static VAL: OnceLock<usize> = OnceLock::new();
    *VAL.get_or_init(|| {
        std::env::var("GQL_API_DEFAULT_NEIGHBOR_LIMIT")
            .ok()
            .and_then(|v| match v.parse() {
                Ok(val) => Some(val),
                Err(_) => {
                    tracing::warn!(
                        "invalid value for GQL_API_DEFAULT_NEIGHBOR_LIMIT: {v:?}, using default 50"
                    );
                    None
                }
            })
            .unwrap_or(50)
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NeighborResponse {
    pub nodes: Vec<Json>,
    pub edges: Vec<Json>,
}
