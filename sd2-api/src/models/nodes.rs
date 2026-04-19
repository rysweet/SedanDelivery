//! Request and response models for node endpoints.

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

#[derive(Deserialize)]
pub struct CreateNodeBody {
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub properties: serde_json::Map<String, Json>,
}

#[derive(Deserialize)]
pub struct UpdateNodeBody {
    #[serde(default)]
    pub properties: serde_json::Map<String, Json>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeListResponse {
    pub nodes: Vec<Json>,
    pub total: usize,
}
