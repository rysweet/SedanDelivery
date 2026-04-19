//! Request and response models for graph export and import endpoints.

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResponse {
    pub nodes: Vec<Json>,
    pub edges: Vec<Json>,
    pub schema: Json,
}

#[derive(Deserialize)]
pub struct ImportBody {
    #[serde(default)]
    pub nodes: Vec<ImportNode>,
    #[serde(default)]
    pub edges: Vec<ImportEdge>,
}

#[derive(Deserialize)]
pub struct ImportNode {
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub properties: serde_json::Map<String, Json>,
    /// Optional stable ID used by ImportEdge to refer to this node.
    pub id: Option<String>,
}

#[derive(Deserialize)]
pub struct ImportEdge {
    pub source: String,
    pub target: String,
    #[serde(rename = "type")]
    pub rel_type: String,
    #[serde(default)]
    pub properties: serde_json::Map<String, Json>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResponse {
    pub nodes_created: usize,
    pub edges_created: usize,
    pub warnings: Vec<String>,
}
