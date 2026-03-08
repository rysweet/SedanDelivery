//! Cypher query and mutate endpoints.
//!
//! POST /graphs/{name}/query   — read queries
//! POST /graphs/{name}/mutate  — write queries

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};

use gql_core::executor::{ExecutionBudget, QueryExecutor};
use gql_core::parser::parse_query;
use gql_core::plan_cache;
use gql_core::planner::QueryPlanner;
use gql_core::procedure::ProcedureRegistry;
use gql_core::serde_value::json_to_value;
use gql_core::storage::validate_graph_name;
use gql_core::types::Value;

use crate::error::ApiError;
use crate::models::{CypherBody, ExplainResponse, MutateResponse, QueryResponse};
use crate::routes::helpers::require_graph;
use crate::serialize::row_to_json;
use crate::state::AppState;

/// Maximum Cypher query string length in bytes. Default 1 MiB (1_048_576).
/// Override via `GQL_API_MAX_QUERY_LENGTH`.
fn max_query_length() -> usize {
    static LIMIT: OnceLock<usize> = OnceLock::new();
    *LIMIT.get_or_init(|| {
        std::env::var("GQL_API_MAX_QUERY_LENGTH")
            .ok()
            .and_then(|v| match v.parse() {
                Ok(val) => Some(val),
                Err(_) => {
                    tracing::warn!(
                        "invalid value for GQL_API_MAX_QUERY_LENGTH: {v:?}, using default 1048576"
                    );
                    None
                }
            })
            .unwrap_or(1024 * 1024)
    })
}

/// Maximum query/mutate/explain body size in bytes. Default 1 MiB.
/// Override via `GQL_API_QUERY_BODY_LIMIT_BYTES`.
fn query_body_limit() -> usize {
    static LIMIT: OnceLock<usize> = OnceLock::new();
    *LIMIT.get_or_init(|| {
        std::env::var("GQL_API_QUERY_BODY_LIMIT_BYTES")
            .ok()
            .and_then(|v| match v.parse() {
                Ok(val) => Some(val),
                Err(_) => {
                    tracing::warn!(
                        "invalid value for GQL_API_QUERY_BODY_LIMIT_BYTES: {v:?}, using default 1048576"
                    );
                    None
                }
            })
            .unwrap_or(1024 * 1024)
    })
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/graphs/{name}/query", post(query_handler))
        .route("/graphs/{name}/mutate", post(mutate_handler))
        .route("/graphs/{name}/explain", post(explain_handler))
        .layer(DefaultBodyLimit::max(query_body_limit()))
}

/// Reject queries whose Cypher string exceeds `max_query_length()`. Returns 413.
fn reject_oversized_query(cypher: &str) -> Result<(), ApiError> {
    let max = max_query_length();
    if cypher.len() > max {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "query length {} bytes exceeds maximum allowed {} bytes",
                cypher.len(),
                max
            ),
        ));
    }
    Ok(())
}

/// Convert optional JSON params map to `HashMap<String, Value>` for the executor.
fn convert_params(
    params: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Result<HashMap<String, Value>, ApiError> {
    let Some(map) = params else {
        return Ok(HashMap::new());
    };
    let mut result = HashMap::with_capacity(map.len());
    for (k, v) in map {
        let val = json_to_value(v).ok_or_else(|| {
            ApiError::bad_request(format!("unsupported value for parameter '{k}'"))
        })?;
        result.insert(k.clone(), val);
    }
    Ok(result)
}

// ── POST /graphs/{name}/query ─────────────────────────────────────────────────

#[tracing::instrument(name = "gql.query", skip_all, fields(graph = %name))]
async fn query_handler(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<CypherBody>,
) -> Result<Json<QueryResponse>, ApiError> {
    validate_graph_name(&name).map_err(ApiError::from)?;
    reject_oversized_query(&body.cypher)?;
    require_graph(&state, &name)?;

    let store = Arc::clone(&state.store);
    let start = Instant::now();

    let cache = plan_cache::global();
    let plan = if let Some((_cached_ast, cached_plan)) = cache.get(&body.cypher) {
        metrics::counter!("plan_cache_hits_total").increment(1);
        cached_plan
    } else {
        metrics::counter!("plan_cache_misses_total").increment(1);
        let ast = parse_query(&body.cypher).map_err(|e| ApiError::bad_request(e.to_string()))?;
        let planner = QueryPlanner::new(store.as_ref(), &name);
        let plan = planner
            .plan(&ast)
            .map_err(|e| ApiError::bad_request(e.to_string()))?;
        cache.insert(&body.cypher, ast, plan.clone());
        plan
    };

    let params = convert_params(body.params.as_ref())?;

    // Propagate the current trace span ID as request_id into the executor budget.
    let request_id = tracing::Span::current()
        .id()
        .map(|id| format!("{:x}", id.into_u64()));

    // Move the Arc and the plan into spawn_blocking so CPU-bound execution
    // does not block the Tokio async thread pool.
    let store_for_exec = Arc::clone(&store);
    let name_for_exec = name.clone();
    let result = tokio::task::spawn_blocking(move || {
        static EMPTY_REG: std::sync::OnceLock<ProcedureRegistry> = std::sync::OnceLock::new();
        let registry = EMPTY_REG.get_or_init(ProcedureRegistry::new);
        let mut budget = ExecutionBudget::default_budget();
        if let Some(rid) = request_id {
            budget.set_request_id(rid);
        }
        QueryExecutor::with_context(store_for_exec.as_ref(), registry, params).execute(
            &name_for_exec,
            &plan,
            &mut budget,
        )
    })
    .await
    .map_err(|_| ApiError::internal("query executor panicked"))??;

    let elapsed_ms = start.elapsed().as_millis() as u64;

    let rows: Vec<serde_json::Value> = result
        .rows
        .iter()
        .map(|row| row_to_json(row, &result.columns, store.as_ref(), &name))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| ApiError::internal(format!("failed to serialize result: {e}")))?;

    Ok(Json(QueryResponse {
        columns: result.columns,
        rows,
        elapsed_ms,
    }))
}

// ── POST /graphs/{name}/explain ────────────────────────────────────────────────

async fn explain_handler(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<CypherBody>,
) -> Result<Json<ExplainResponse>, ApiError> {
    validate_graph_name(&name).map_err(ApiError::from)?;
    reject_oversized_query(&body.cypher)?;
    require_graph(&state, &name)?;

    let store = Arc::clone(&state.store);

    let ast = parse_query(&body.cypher).map_err(|e| ApiError::bad_request(e.to_string()))?;
    let planner = QueryPlanner::new(store.as_ref(), &name);
    let plan = planner
        .plan(&ast)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;

    let store_ref = store.as_ref();
    let graph_stats =
        gql_core::planner::collect_graph_statistics(store_ref, &name).map_err(ApiError::from)?;
    let estimates = gql_core::planner::estimate_cardinality(&plan, &graph_stats);

    let plan_json = operator_to_json(&plan, &estimates);

    Ok(Json(ExplainResponse { plan: plan_json }))
}

/// Convert an operator tree to a JSON plan tree with estimated rows.
fn operator_to_json(
    op: &gql_core::planner::Operator,
    estimates: &std::collections::HashMap<*const gql_core::planner::Operator, f64>,
) -> serde_json::Value {
    let op_type = format!("{:?}", op)
        .split(['(', '{'])
        .next()
        .unwrap_or("Unknown")
        .trim()
        .to_string();
    let estimated_rows = estimates
        .get(&(op as *const gql_core::planner::Operator))
        .copied()
        .unwrap_or(1.0);
    let children: Vec<serde_json::Value> = op
        .children()
        .into_iter()
        .map(|child| operator_to_json(child, estimates))
        .collect();
    serde_json::json!({
        "operatorType": op_type,
        "estimatedRows": estimated_rows,
        "children": children,
    })
}

// ── POST /graphs/{name}/mutate ────────────────────────────────────────────────

#[tracing::instrument(name = "gql.mutate", skip_all, fields(graph = %name))]
async fn mutate_handler(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<CypherBody>,
) -> Result<Json<MutateResponse>, ApiError> {
    validate_graph_name(&name).map_err(ApiError::from)?;
    reject_oversized_query(&body.cypher)?;
    require_graph(&state, &name)?;

    let store = Arc::clone(&state.store);

    let cache = plan_cache::global();
    let plan = if let Some((_cached_ast, cached_plan)) = cache.get(&body.cypher) {
        metrics::counter!("plan_cache_hits_total").increment(1);
        cached_plan
    } else {
        metrics::counter!("plan_cache_misses_total").increment(1);
        let ast = parse_query(&body.cypher).map_err(|e| ApiError::bad_request(e.to_string()))?;
        let planner = QueryPlanner::new(store.as_ref(), &name);
        let plan = planner
            .plan(&ast)
            .map_err(|e| ApiError::bad_request(e.to_string()))?;
        cache.insert(&body.cypher, ast, plan.clone());
        plan
    };

    let params = convert_params(body.params.as_ref())?;

    // Propagate the current trace span ID as request_id into the executor budget.
    let request_id = tracing::Span::current()
        .id()
        .map(|id| format!("{:x}", id.into_u64()));

    // Move the Arc and plan into spawn_blocking so CPU-bound execution
    // does not block the Tokio async thread pool.
    let store_for_exec = Arc::clone(&store);
    let name_for_exec = name.clone();
    let result = tokio::task::spawn_blocking(move || {
        static EMPTY_REG: std::sync::OnceLock<ProcedureRegistry> = std::sync::OnceLock::new();
        let registry = EMPTY_REG.get_or_init(ProcedureRegistry::new);
        let mut budget = ExecutionBudget::default_budget();
        if let Some(rid) = request_id {
            budget.set_request_id(rid);
        }
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
        let result = convert_params(None).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn convert_params_empty_map() {
        let map = serde_json::Map::new();
        let result = convert_params(Some(&map)).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn convert_params_string_value() {
        let mut map = serde_json::Map::new();
        map.insert("name".to_string(), json!("Alice"));
        let result = convert_params(Some(&map)).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result["name"], Value::Str("Alice".into()));
    }

    #[test]
    fn convert_params_int_value() {
        let mut map = serde_json::Map::new();
        map.insert("age".to_string(), json!(30));
        let result = convert_params(Some(&map)).unwrap();
        assert_eq!(result["age"], Value::Int(30));
    }

    #[test]
    fn convert_params_mixed_types() {
        let mut map = serde_json::Map::new();
        map.insert("name".to_string(), json!("Bob"));
        map.insert("age".to_string(), json!(25));
        map.insert("active".to_string(), json!(true));
        map.insert("score".to_string(), json!(3.14));
        let result = convert_params(Some(&map)).unwrap();
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
        let result = convert_params(Some(&map)).unwrap();
        assert_eq!(result["x"], Value::Null);
    }

    #[test]
    fn convert_params_list_value() {
        let mut map = serde_json::Map::new();
        map.insert("ids".to_string(), json!([1, 2, 3]));
        let result = convert_params(Some(&map)).unwrap();
        assert!(matches!(result["ids"], Value::List(_)));
    }
}
