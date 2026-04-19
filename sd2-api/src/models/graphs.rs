//! Request and response models for graph-level endpoints.

use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct CreateGraphBody {
    pub name: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphListResponse {
    pub graphs: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphStatsResponse {
    pub node_count: u64,
    pub edge_count: u64,
    pub label_count: u32,
    pub rel_type_count: u32,
}
