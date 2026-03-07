//! Cypher query and mutate endpoints.
//!
//! POST /graphs/{name}/query   — read queries
//! POST /graphs/{name}/mutate  — write queries

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use axum::extract::{Path, State};
use axum::routing::post;
use axum::{Json, Router};

use gql_core::executor::{ExecutionBudget, QueryExecutor};
use gql_core::parser::parse_query;
use gql_core::planner::QueryPlanner;
use gql_core::procedure::ProcedureRegistry;
use gql_core::serde_value::json_to_value;
use gql_core::storage::validate_graph_name;
use gql_core::types::Value;

use crate::error::ApiError;
use crate::models::{CypherBody, MutateResponse, QueryResponse};
use crate::routes::helpers::require_graph;
use crate::serialize::row_to_json;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/graphs/{name}/query", post(query_handler))
        .route("/graphs/{name}/mutate", post(mutate_handler))
}

/// Convert optional JSON params map to `HashMap<String, Value>` for the executor.
fn convert_params(
    params: Option<&serde_json::Map<String, serde_json::Value>>,
) -> HashMap<String, Value> {
    match params {
        Some(map) => map
            .iter()
            .filter_map(|(k, v)| json_to_value(v).map(|val| (k.clone(), val)))
            .collect(),
        None => HashMap::new(),
    }
}

// ── POST /graphs/{name}/query ─────────────────────────────────────────────────

async fn query_handler(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<CypherBody>,
) -> Result<Json<QueryResponse>, ApiError> {
    validate_graph_name(&name).map_err(ApiError::from)?;
    require_graph(&state, &name)?;

    let store = Arc::clone(&state.store);
    let start = Instant::now();

    let ast = parse_query(&body.cypher).map_err(|e| ApiError::bad_request(e.to_string()))?;
    let planner = QueryPlanner::new(store.as_ref(), &name);
    let plan = planner
        .plan(&ast)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;

    let params = convert_params(body.params.as_ref());

    // Move the Arc and the plan into spawn_blocking so CPU-bound execution
    // does not block the Tokio async thread pool.
    let store_for_exec = Arc::clone(&store);
    let name_for_exec = name.clone();
    let result = tokio::task::spawn_blocking(move || {
        static EMPTY_REG: std::sync::OnceLock<ProcedureRegistry> = std::sync::OnceLock::new();
        let registry = EMPTY_REG.get_or_init(ProcedureRegistry::new);
        let mut budget = ExecutionBudget::default_budget();
        QueryExecutor::with_context(store_for_exec.as_ref(), registry, params).execute(
            &name_for_exec,
            &plan,
            &mut budget,
        )
    })
    .await
    .map_err(|_| ApiError::internal("query executor panicked"))??;

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
    require_graph(&state, &name)?;

    let store = Arc::clone(&state.store);

    let ast = parse_query(&body.cypher).map_err(|e| ApiError::bad_request(e.to_string()))?;
    let planner = QueryPlanner::new(store.as_ref(), &name);
    let plan = planner
        .plan(&ast)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;

    let params = convert_params(body.params.as_ref());

    // Move the Arc and plan into spawn_blocking so CPU-bound execution
    // does not block the Tokio async thread pool.
    let store_for_exec = Arc::clone(&store);
    let name_for_exec = name.clone();
    let result = tokio::task::spawn_blocking(move || {
        static EMPTY_REG: std::sync::OnceLock<ProcedureRegistry> = std::sync::OnceLock::new();
        let registry = EMPTY_REG.get_or_init(ProcedureRegistry::new);
        let mut budget = ExecutionBudget::default_budget();
        QueryExecutor::with_context(store_for_exec.as_ref(), registry, params).execute(
            &name_for_exec,
            &plan,
            &mut budget,
        )
    })
    .await
    .map_err(|_| ApiError::internal("mutate executor panicked"))??;

    Ok(Json(MutateResponse {
        nodes_created: result.stats.nodes_created,
        nodes_deleted: result.stats.nodes_deleted,
        edges_created: result.stats.edges_created,
        edges_deleted: result.stats.edges_deleted,
        properties_set: result.stats.properties_set,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn convert_params_none_returns_empty() {
        let result = convert_params(None);
        assert!(result.is_empty());
    }

    #[test]
    fn convert_params_empty_map() {
        let map = serde_json::Map::new();
        let result = convert_params(Some(&map));
        assert!(result.is_empty());
    }

    #[test]
    fn convert_params_string_value() {
        let mut map = serde_json::Map::new();
        map.insert("name".to_string(), json!("Alice"));
        let result = convert_params(Some(&map));
        assert_eq!(result.len(), 1);
        assert_eq!(result["name"], Value::Str("Alice".into()));
    }

    #[test]
    fn convert_params_int_value() {
        let mut map = serde_json::Map::new();
        map.insert("age".to_string(), json!(30));
        let result = convert_params(Some(&map));
        assert_eq!(result["age"], Value::Int(30));
    }

    #[test]
    fn convert_params_mixed_types() {
        let mut map = serde_json::Map::new();
        map.insert("name".to_string(), json!("Bob"));
        map.insert("age".to_string(), json!(25));
        map.insert("active".to_string(), json!(true));
        map.insert("score".to_string(), json!(3.14));
        let result = convert_params(Some(&map));
        assert_eq!(result.len(), 4);
        assert_eq!(result["name"], Value::Str("Bob".into()));
        assert_eq!(result["age"], Value::Int(25));
        assert_eq!(result["active"], Value::Bool(true));
        assert_eq!(result["score"], Value::Float(3.14));
    }

    #[test]
    fn convert_params_null_value() {
        let mut map = serde_json::Map::new();
        map.insert("x".to_string(), json!(null));
        let result = convert_params(Some(&map));
        assert_eq!(result["x"], Value::Null);
    }

    #[test]
    fn convert_params_list_value() {
        let mut map = serde_json::Map::new();
        map.insert("ids".to_string(), json!([1, 2, 3]));
        let result = convert_params(Some(&map));
        assert!(matches!(result["ids"], Value::List(_)));
    }
}
