//! Request and response models for edge endpoints.

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

#[derive(Deserialize)]
pub struct CreateEdgeBody {
    pub source: String,
    pub target: String,
    #[serde(rename = "type")]
    pub rel_type: String,
    #[serde(default)]
    pub properties: serde_json::Map<String, Json>,
}

#[derive(Deserialize)]
pub struct UpdateEdgeBody {
    #[serde(default)]
    pub properties: serde_json::Map<String, Json>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EdgeListResponse {
    pub edges: Vec<Json>,
    pub total: usize,
}
