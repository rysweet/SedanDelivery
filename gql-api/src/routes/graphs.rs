//! Graph administration endpoints.
//!
//! GET    /graphs
//! POST   /graphs
//! DELETE /graphs/{name}
//! GET    /graphs/{name}/stats

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};

use std::sync::Arc;

use gql_core::storage::{validate_graph_name, GraphStore, InMemoryGraphStore};

use crate::error::ApiError;
use crate::models::{CreateGraphBody, GraphListResponse, GraphStatsResponse};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/graphs", get(list_graphs))
        .route("/graphs", post(create_graph))
        .route("/graphs/{name}", delete(delete_graph))
        .route("/graphs/{name}/stats", get(graph_stats))
}

// ── GET /graphs ───────────────────────────────────────────────────────────────

async fn list_graphs(State(state): State<AppState>) -> Result<Json<GraphListResponse>, ApiError> {
    let map = state
        .stores
        .lock()
        .map_err(|_| ApiError::internal("lock poisoned"))?;
    let mut graphs: Vec<String> = map.keys().cloned().collect();
    graphs.sort();
    Ok(Json(GraphListResponse { graphs }))
}

// ── POST /graphs ──────────────────────────────────────────────────────────────

async fn create_graph(
    State(state): State<AppState>,
    Json(body): Json<CreateGraphBody>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    validate_graph_name(&body.name).map_err(ApiError::from)?;

    let mut map = state
        .stores
        .lock()
        .map_err(|_| ApiError::internal("lock poisoned"))?;

    if map.contains_key(&body.name) {
        return Err(ApiError::conflict(format!(
            "graph '{}' already exists",
            body.name
        )));
    }

    let store = Arc::new(InMemoryGraphStore::default());
    store.create_graph(&body.name).map_err(ApiError::from)?;

    map.insert(body.name.clone(), store);

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "name": body.name })),
    ))
}

// ── DELETE /graphs/{name} ─────────────────────────────────────────────────────

async fn delete_graph(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    validate_graph_name(&name).map_err(ApiError::from)?;

    let mut map = state
        .stores
        .lock()
        .map_err(|_| ApiError::internal("lock poisoned"))?;

    if !map.contains_key(&name) {
        return Err(ApiError::not_found(format!("graph '{name}' not found")));
    }

    map.remove(&name);
    Ok(StatusCode::NO_CONTENT)
}

// ── GET /graphs/{name}/stats ──────────────────────────────────────────────────

async fn graph_stats(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<GraphStatsResponse>, ApiError> {
    validate_graph_name(&name).map_err(ApiError::from)?;

    let store = state
        .get_store(&name)
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found(format!("graph '{name}' not found")))?;

    let stats = store.graph_stats(&name).map_err(ApiError::from)?;
    Ok(Json(GraphStatsResponse {
        node_count: stats.node_count,
        edge_count: stats.edge_count,
        label_count: stats.label_count,
        rel_type_count: stats.rel_type_count,
    }))
}
