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
use crate::routes::helpers::{json_map_to_props, parse_node_id, require_graph, run_store_blocking};
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
    require_graph(&state, &name).await?;
    let graph_name = name.clone();
    let limit = params.limit;
    let offset = params.offset;
    let (nodes, total) = run_store_blocking(&state, move |store| {
        let all_ids = store.all_node_ids(&graph_name).map_err(ApiError::from)?;
        let total = all_ids.len();
        let page_ids: Vec<NodeId> = all_ids.into_iter().skip(offset).take(limit).collect();
        let batch = store
            .get_nodes_batch(&graph_name, &page_ids)
            .map_err(ApiError::from)?;
        let nodes = batch.iter().flatten().map(node_to_json).collect();
        Ok((nodes, total))
    })
    .await?;

    Ok(Json(NodeListResponse { nodes, total }))
}

// ── POST /graphs/{name}/nodes ─────────────────────────────────────────────────

async fn create_node(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<CreateNodeBody>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    validate_graph_name(&name).map_err(ApiError::from)?;
    require_graph(&state, &name).await?;
    let props = json_map_to_props(body.properties)?;
    let graph_name = name.clone();
    let labels = body.labels;
    let node = run_store_blocking(&state, move |store| {
        let id = store
            .create_node(&graph_name, labels, props)
            .map_err(ApiError::from)?;
        store
            .get_node(&graph_name, id)
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::internal("node created but cannot be retrieved"))
    })
    .await?;

    Ok((StatusCode::CREATED, Json(node_to_json(&node))))
}

// ── GET /graphs/{name}/nodes/{id} ─────────────────────────────────────────────

async fn get_node(
    State(state): State<AppState>,
    Path((name, id_str)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    validate_graph_name(&name).map_err(ApiError::from)?;
    let id = parse_node_id(&id_str)?;
    require_graph(&state, &name).await?;
    let graph_name = name.clone();
    let id_lookup = id_str.clone();
    let node = run_store_blocking(&state, move |store| {
        store
            .get_node(&graph_name, id)
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::not_found(format!("node {id_lookup} not found")))
    })
    .await?;

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
    require_graph(&state, &name).await?;
    let props = json_map_to_props(body.properties)?;
    let graph_name = name.clone();
    let updated = run_store_blocking(&state, move |store| {
        store
            .set_node_properties(&graph_name, id, props)
            .map_err(ApiError::from)?;
        store
            .get_node(&graph_name, id)
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::internal("node vanished after update"))
    })
    .await?;

    Ok(Json(node_to_json(&updated)))
}

// ── DELETE /graphs/{name}/nodes/{id} ──────────────────────────────────────────

async fn delete_node(
    State(state): State<AppState>,
    Path((name, id_str)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    validate_graph_name(&name).map_err(ApiError::from)?;
    let id = parse_node_id(&id_str)?;
    require_graph(&state, &name).await?;
    let graph_name = name.clone();
    let id_lookup = id_str.clone();
    run_store_blocking(&state, move |store| {
        store.delete_node(&graph_name, id, true).map_err(|e| {
            use gql_core::error::StorageError;
            match &e {
                StorageError::NodeNotFound { .. } => {
                    ApiError::not_found(format!("node {id_lookup} not found"))
                }
                _ => ApiError::from(e),
            }
        })
    })
    .await?;

    Ok(StatusCode::NO_CONTENT)
}
