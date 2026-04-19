//! Request and response models for query, mutate, explain, and profile endpoints.

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

#[derive(Deserialize)]
pub struct CypherBody {
    pub cypher: String,
    #[serde(default)]
    pub params: Option<serde_json::Map<String, Json>>,
    /// Optional page size for cursor-based pagination. When set, the response
    /// will contain at most `page_size` rows and a `cursor` token if more rows
    /// are available.
    #[serde(default)]
    pub page_size: Option<usize>,
    /// Opaque cursor token returned from a previous paginated query. Pass this
    /// to fetch the next page of results.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Optional override for the target database. When set, the query will
    /// execute against this database instead of the one in the URL path.
    #[serde(default)]
    pub database: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryResponse {
    pub columns: Vec<String>,
    pub rows: Vec<Json>,
    pub elapsed_ms: u64,
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplainResponse {
    pub plan: serde_json::Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileResponse {
    pub columns: Vec<String>,
    pub rows: Vec<serde_json::Value>,
    pub plan: serde_json::Value,
    pub elapsed_ms: u64,
}
