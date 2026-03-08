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

use gql_core::storage::validate_graph_name;

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
    let mut graphs = state.store.list_graphs().map_err(ApiError::from)?;
    graphs.sort();
    Ok(Json(GraphListResponse { graphs }))
}

// ── POST /graphs ──────────────────────────────────────────────────────────────

async fn create_graph(
    State(state): State<AppState>,
    Json(body): Json<CreateGraphBody>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    validate_graph_name(&body.name).map_err(ApiError::from)?;

    state
        .store
        .create_graph(&body.name)
        .map_err(ApiError::from)?;
    if let Ok(graphs) = state.store.list_graphs() {
        crate::metrics::set_active_graphs(graphs.len());
    }

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

    state.store.delete_graph(&name).map_err(ApiError::from)?;
    if let Ok(graphs) = state.store.list_graphs() {
        crate::metrics::set_active_graphs(graphs.len());
    }
    Ok(StatusCode::NO_CONTENT)
}

// ── GET /graphs/{name}/stats ──────────────────────────────────────────────────

async fn graph_stats(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<GraphStatsResponse>, ApiError> {
    validate_graph_name(&name).map_err(ApiError::from)?;

    if !state.store.graph_exists(&name).map_err(ApiError::from)? {
        return Err(ApiError::not_found(format!("graph '{name}' not found")));
    }

    let stats = state.store.graph_stats(&name).map_err(ApiError::from)?;
    Ok(Json(GraphStatsResponse {
        node_count: stats.node_count,
        edge_count: stats.edge_count,
        label_count: stats.label_count,
        rel_type_count: stats.rel_type_count,
    }))
}
