//! Node CRUD endpoints.
//!
//! GET    /graphs/{name}/nodes?limit=100&offset=0
//! POST   /graphs/{name}/nodes
//! GET    /graphs/{name}/nodes/{id}
//! PUT    /graphs/{name}/nodes/{id}
//! DELETE /graphs/{name}/nodes/{id}

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};

use gql_core::storage::{validate_graph_name, NodeId};

use crate::error::ApiError;
use crate::models::{
    max_page_limit, CreateNodeBody, NodeListResponse, PaginationParams, UpdateNodeBody,
};
use crate::routes::helpers::{json_map_to_props, parse_node_id, require_graph};
use crate::serialize::node_to_json;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/graphs/{name}/nodes", get(list_nodes))
        .route("/graphs/{name}/nodes", post(create_node))
        .route("/graphs/{name}/nodes/{id}", get(get_node))
        .route("/graphs/{name}/nodes/{id}", put(update_node))
        .route("/graphs/{name}/nodes/{id}", delete(delete_node))
}

// ── GET /graphs/{name}/nodes ──────────────────────────────────────────────────

async fn list_nodes(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(params): Query<PaginationParams>,
) -> Result<Json<NodeListResponse>, ApiError> {
    if params.limit == 0 {
        return Err(ApiError::bad_request("limit must be >= 1"));
    }
    let limit_cap = max_page_limit();
    if params.limit > limit_cap {
        return Err(ApiError::bad_request(format!(
            "limit must be ≤ {limit_cap}, got {}",
            params.limit
        )));
    }
    validate_graph_name(&name).map_err(ApiError::from)?;
    require_graph(&state, &name)?;

    let store = &state.store;
    let all_ids = store.all_node_ids(&name).map_err(ApiError::from)?;
    let total = all_ids.len();

    let page_ids: Vec<NodeId> = all_ids
        .into_iter()
        .skip(params.offset)
        .take(params.limit)
        .collect();

    let batch = store
        .get_nodes_batch(&name, &page_ids)
        .map_err(ApiError::from)?;

    let nodes: Vec<serde_json::Value> = batch.iter().flatten().map(node_to_json).collect();

    Ok(Json(NodeListResponse { nodes, total }))
}

// ── POST /graphs/{name}/nodes ─────────────────────────────────────────────────

async fn create_node(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<CreateNodeBody>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    validate_graph_name(&name).map_err(ApiError::from)?;
    require_graph(&state, &name)?;

    let store = &state.store;
    let props = json_map_to_props(body.properties)?;
    let id = store
        .create_node(&name, body.labels, props)
        .map_err(ApiError::from)?;

    let node = store
        .get_node(&name, id)
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::internal("node created but cannot be retrieved"))?;

    Ok((StatusCode::CREATED, Json(node_to_json(&node))))
}

// ── GET /graphs/{name}/nodes/{id} ─────────────────────────────────────────────

async fn get_node(
    State(state): State<AppState>,
    Path((name, id_str)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    validate_graph_name(&name).map_err(ApiError::from)?;
    let id = parse_node_id(&id_str)?;
    require_graph(&state, &name)?;

    let node = state
        .store
        .get_node(&name, id)
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found(format!("node {id_str} not found")))?;

    Ok(Json(node_to_json(&node)))
}

// ── PUT /graphs/{name}/nodes/{id} ─────────────────────────────────────────────

async fn update_node(
    State(state): State<AppState>,
    Path((name, id_str)): Path<(String, String)>,
    Json(body): Json<UpdateNodeBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    validate_graph_name(&name).map_err(ApiError::from)?;
    let id = parse_node_id(&id_str)?;
    require_graph(&state, &name)?;

    let store = &state.store;

    let props = json_map_to_props(body.properties)?;
    store
        .set_node_properties(&name, id, props)
        .map_err(ApiError::from)?;

    let updated = store
        .get_node(&name, id)
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::internal("node vanished after update"))?;

    Ok(Json(node_to_json(&updated)))
}

// ── DELETE /graphs/{name}/nodes/{id} ──────────────────────────────────────────

async fn delete_node(
    State(state): State<AppState>,
    Path((name, id_str)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    validate_graph_name(&name).map_err(ApiError::from)?;
    let id = parse_node_id(&id_str)?;
    require_graph(&state, &name)?;

    state.store.delete_node(&name, id, true).map_err(|e| {
        use gql_core::error::StorageError;
        match &e {
            StorageError::NodeNotFound { .. } => {
                ApiError::not_found(format!("node {id_str} not found"))
            }
            _ => ApiError::from(e),
        }
    })?;

    Ok(StatusCode::NO_CONTENT)
}
