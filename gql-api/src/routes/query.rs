//! Cypher query and mutate endpoints.
//!
//! POST /graphs/{name}/query   — read queries
//! POST /graphs/{name}/mutate  — write queries
//!
//! # Lock discipline
//! The outer `Mutex` on `AppState.stores` guards only the graph-name→store
//! map.  We clone the `Arc<InMemoryGraphStore>` while holding the lock, then
//! release the lock **before** running any query/plan/execute work.  This
//! prevents slow CPU-bound executor work from serialising all other requests.

use std::time::Instant;

use axum::extract::{Path, State};
use axum::routing::post;
use axum::{Json, Router};

use gql_core::executor::{ExecutionBudget, QueryExecutor};
use gql_core::parser::parse_query;
use gql_core::planner::QueryPlanner;
use gql_core::storage::validate_graph_name;

use crate::error::ApiError;
use crate::models::{CypherBody, MutateResponse, QueryResponse};
use crate::serialize::row_to_json;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/graphs/{name}/query", post(query_handler))
        .route("/graphs/{name}/mutate", post(mutate_handler))
}

// ── POST /graphs/{name}/query ─────────────────────────────────────────────────

async fn query_handler(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<CypherBody>,
) -> Result<Json<QueryResponse>, ApiError> {
    validate_graph_name(&name).map_err(ApiError::from)?;

    // Acquire the lock only to retrieve the Arc — release immediately after.
    let store = state
        .get_store(&name)
        .ok_or_else(|| ApiError::not_found(format!("graph '{name}' not found")))?;

    let start = Instant::now();

    let ast = parse_query(&body.cypher).map_err(|e| ApiError::bad_request(e.to_string()))?;
    let planner = QueryPlanner::new(store.as_ref(), &name);
    let plan = planner.plan(&ast).map_err(|e| ApiError::bad_request(e.to_string()))?;
    let mut budget = ExecutionBudget::default_budget();
    let result = QueryExecutor::new(store.as_ref())
        .execute(&name, &plan, &mut budget)
        .map_err(ApiError::from)?;

    let elapsed_ms = start.elapsed().as_millis();

    let rows: Vec<serde_json::Value> = result
        .rows
        .iter()
        .map(|row| row_to_json(row, &result.columns, store.as_ref(), &name))
        .collect();

    Ok(Json(QueryResponse {
        columns: result.columns,
        rows,
        elapsed_ms,
    }))
}

// ── POST /graphs/{name}/mutate ────────────────────────────────────────────────

async fn mutate_handler(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<CypherBody>,
) -> Result<Json<MutateResponse>, ApiError> {
    validate_graph_name(&name).map_err(ApiError::from)?;

    // Acquire the lock only to retrieve the Arc — release immediately after.
    let store = state
        .get_store(&name)
        .ok_or_else(|| ApiError::not_found(format!("graph '{name}' not found")))?;

    let ast = parse_query(&body.cypher).map_err(|e| ApiError::bad_request(e.to_string()))?;
    let planner = QueryPlanner::new(store.as_ref(), &name);
    let plan = planner.plan(&ast).map_err(|e| ApiError::bad_request(e.to_string()))?;
    let mut budget = ExecutionBudget::default_budget();
    let result = QueryExecutor::new(store.as_ref())
        .execute(&name, &plan, &mut budget)
        .map_err(ApiError::from)?;

    Ok(Json(MutateResponse {
        nodes_created: result.stats.nodes_created,
        nodes_deleted: result.stats.nodes_deleted,
        edges_created: result.stats.edges_created,
        edges_deleted: result.stats.edges_deleted,
        properties_set: result.stats.properties_set,
    }))
}
