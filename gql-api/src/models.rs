//! Request and response body models for all API endpoints.

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

// ── Health ────────────────────────────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    pub status: &'static str,
    pub version: &'static str,
    pub demo_graph_exists: bool,
}

// ── Graphs ────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CreateGraphBody {
    pub name: String,
}

#[derive(Serialize)]
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

// ── Nodes ─────────────────────────────────────────────────────────────────────

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
pub struct NodeListResponse {
    pub nodes: Vec<Json>,
    pub total: usize,
}

// ── Edges ─────────────────────────────────────────────────────────────────────

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
pub struct EdgeListResponse {
    pub edges: Vec<Json>,
    pub total: usize,
}

// ── Query / Mutate ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CypherBody {
    pub cypher: String,
}

#[derive(Serialize)]
pub struct QueryResponse {
    pub columns: Vec<String>,
    pub rows: Vec<Json>,
    pub elapsed_ms: u128,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MutateResponse {
    pub nodes_created: u64,
    pub nodes_deleted: u64,
    pub edges_created: u64,
    pub edges_deleted: u64,
    pub properties_set: u64,
}

// ── Pagination query params ───────────────────────────────────────────────────

/// Return the maximum number of items that may be requested in a single page.
///
/// Reads `GQL_API_MAX_PAGE_LIMIT` from the environment on first call (default `10_000`).
pub fn max_page_limit() -> usize {
    use std::sync::OnceLock;
    static VAL: OnceLock<usize> = OnceLock::new();
    *VAL.get_or_init(|| {
        std::env::var("GQL_API_MAX_PAGE_LIMIT")
            .ok()
            .and_then(|v| match v.parse() {
                Ok(val) => Some(val),
                Err(_) => {
                    tracing::warn!(
                        "invalid value for GQL_API_MAX_PAGE_LIMIT: {v:?}, using default 10000"
                    );
                    None
                }
            })
            .unwrap_or(10_000)
    })
}

/// Return the default page size when the caller does not supply a `limit` parameter.
///
/// Reads `GQL_API_DEFAULT_PAGE_LIMIT` from the environment on first call (default `100`).
pub fn default_page_limit() -> usize {
    use std::sync::OnceLock;
    static VAL: OnceLock<usize> = OnceLock::new();
    *VAL.get_or_init(|| {
        std::env::var("GQL_API_DEFAULT_PAGE_LIMIT")
            .ok()
            .and_then(|v| match v.parse() {
                Ok(val) => Some(val),
                Err(_) => {
                    tracing::warn!(
                        "invalid value for GQL_API_DEFAULT_PAGE_LIMIT: {v:?}, using default 100"
                    );
                    None
                }
            })
            .unwrap_or(100)
    })
}

#[derive(Deserialize)]
pub struct PaginationParams {
    #[serde(default = "default_page_limit")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
}

// ── Schema ────────────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct LabelInfo {
    pub name: String,
    pub count: usize,
}

#[derive(Serialize)]
pub struct RelTypeInfo {
    pub name: String,
    pub count: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaResponse {
    pub labels: Vec<LabelInfo>,
    pub relationship_types: Vec<RelTypeInfo>,
}

// ── Neighbors ────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct NeighborParams {
    /// "in", "out", or "both" (default: "both")
    pub direction: Option<String>,
    /// Comma-separated relationship types to filter
    pub types: Option<String>,
    #[serde(default = "default_neighbor_limit")]
    pub limit: usize,
}

fn default_neighbor_limit() -> usize {
    50
}

#[derive(Serialize)]
pub struct NeighborResponse {
    pub nodes: Vec<Json>,
    pub edges: Vec<Json>,
}

// ── Export / Import ───────────────────────────────────────────────────────────

#[derive(Serialize)]
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
