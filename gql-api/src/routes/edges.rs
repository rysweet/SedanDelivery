//! Edge CRUD endpoints.
//!
//! GET    /graphs/{name}/edges?limit=100&offset=0
//! POST   /graphs/{name}/edges
//! GET    /graphs/{name}/edges/{id}
//! PUT    /graphs/{name}/edges/{id}
//! DELETE /graphs/{name}/edges/{id}

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};

use gql_core::storage::{validate_graph_name, EdgeId, GraphStore};

use crate::error::ApiError;
use crate::models::{
    max_page_limit, CreateEdgeBody, EdgeListResponse, PaginationParams, UpdateEdgeBody,
};
use crate::routes::helpers::{json_map_to_props, parse_edge_id, parse_node_id};
use crate::serialize::edge_to_json;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/graphs/{name}/edges", get(list_edges))
        .route("/graphs/{name}/edges", post(create_edge))
        .route("/graphs/{name}/edges/{id}", get(get_edge))
        .route("/graphs/{name}/edges/{id}", put(update_edge))
        .route("/graphs/{name}/edges/{id}", delete(delete_edge))
}

// ── GET /graphs/{name}/edges ──────────────────────────────────────────────────

async fn list_edges(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(params): Query<PaginationParams>,
) -> Result<Json<EdgeListResponse>, ApiError> {
    let limit_cap = max_page_limit();
    if params.limit > limit_cap {
        return Err(ApiError::bad_request(format!(
            "limit must be ≤ {limit_cap}, got {}",
            params.limit
        )));
    }
    validate_graph_name(&name).map_err(ApiError::from)?;

    let store = state
        .get_store(&name)
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found(format!("graph '{name}' not found")))?;

    let all_ids = store.all_edge_ids(&name).map_err(ApiError::from)?;
    let total = all_ids.len();

    let page_ids: Vec<EdgeId> = all_ids
        .into_iter()
        .skip(params.offset)
        .take(params.limit)
        .collect();

    let batch = store
        .get_edges_batch(&name, &page_ids)
        .map_err(ApiError::from)?;

    let edges: Vec<serde_json::Value> = batch.iter().flatten().map(edge_to_json).collect();

    Ok(Json(EdgeListResponse { edges, total }))
}

// ── POST /graphs/{name}/edges ─────────────────────────────────────────────────

async fn create_edge(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<CreateEdgeBody>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    validate_graph_name(&name).map_err(ApiError::from)?;

    let src_id = parse_node_id(&body.source)?;
    let dst_id = parse_node_id(&body.target)?;

    if body.rel_type.is_empty() {
        return Err(ApiError::bad_request("relationship type must not be empty"));
    }

    let store = state
        .get_store(&name)
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found(format!("graph '{name}' not found")))?;

    let props = json_map_to_props(body.properties)?;
    let edge_id = store
        .create_edge(&name, src_id, dst_id, body.rel_type, props)
        .map_err(ApiError::from)?;

    let edge = store
        .get_edge(&name, edge_id)
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::internal("edge created but cannot be retrieved"))?;

    Ok((StatusCode::CREATED, Json(edge_to_json(&edge))))
}

// ── GET /graphs/{name}/edges/{id} ─────────────────────────────────────────────

async fn get_edge(
    State(state): State<AppState>,
    Path((name, id_str)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    validate_graph_name(&name).map_err(ApiError::from)?;
    let id = parse_edge_id(&id_str)?;

    let store = state
        .get_store(&name)
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found(format!("graph '{name}' not found")))?;

    let edge = store
        .get_edge(&name, id)
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found(format!("edge {id_str} not found")))?;

    Ok(Json(edge_to_json(&edge)))
}

// ── PUT /graphs/{name}/edges/{id} ─────────────────────────────────────────────

async fn update_edge(
    State(state): State<AppState>,
    Path((name, id_str)): Path<(String, String)>,
    Json(body): Json<UpdateEdgeBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    validate_graph_name(&name).map_err(ApiError::from)?;
    let id = parse_edge_id(&id_str)?;

    let store = state
        .get_store(&name)
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found(format!("graph '{name}' not found")))?;

    // Verify the edge exists.
    store
        .get_edge(&name, id)
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found(format!("edge {id_str} not found")))?;

    let props = json_map_to_props(body.properties)?;
    store
        .set_edge_properties(&name, id, props)
        .map_err(ApiError::from)?;

    let updated = store
        .get_edge(&name, id)
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::internal("edge vanished after update"))?;

    Ok(Json(edge_to_json(&updated)))
}

// ── DELETE /graphs/{name}/edges/{id} ──────────────────────────────────────────

async fn delete_edge(
    State(state): State<AppState>,
    Path((name, id_str)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    validate_graph_name(&name).map_err(ApiError::from)?;
    let id = parse_edge_id(&id_str)?;

    let store = state
        .get_store(&name)
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::not_found(format!("graph '{name}' not found")))?;

    store.delete_edge(&name, id).map_err(|e| {
        use gql_core::error::StorageError;
        match e {
            StorageError::EdgeNotFound { .. } => {
                ApiError::not_found(format!("edge {id_str} not found"))
            }
            other => ApiError::from(other),
        }
    })?;

    Ok(StatusCode::NO_CONTENT)
}
