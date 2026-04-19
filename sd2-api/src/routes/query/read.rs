//! POST /graphs/{name}/query — read queries.

use std::sync::Arc;
use std::time::Instant;

use axum::extract::{Path, State};
use axum::Json;

use cypher_core::executor::QueryExecutor;
use cypher_core::parser::parse_query;
use cypher_core::plan_cache;
use cypher_core::planner::QueryPlanner;
use cypher_core::procedure::ProcedureRegistry;
use cypher_core::storage::validate_graph_name;

use crate::error::ApiError;
use crate::models::{CypherBody, QueryResponse};
use crate::routes::helpers::require_graph;
use crate::serialize::row_to_json;
use crate::state::AppState;

use super::common::{
    configured_budget, convert_params, current_request_id, reject_oversized_query,
};

#[tracing::instrument(name = "gql.query", skip_all, fields(graph = %name))]
pub(super) async fn query_handler(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<CypherBody>,
) -> Result<Json<QueryResponse>, ApiError> {
    let name = body.database.as_ref().unwrap_or(&name);
    validate_graph_name(name).map_err(ApiError::from)?;
    reject_oversized_query(&body.cypher)?;
    require_graph(&state, name).await?;

    let store = Arc::clone(&state.store);
    let start = Instant::now();
    let graph_name = name.to_string();
    let cypher = body.cypher.clone();
    let params = convert_params(body.params.as_ref())?;

    let request_id = current_request_id();

    let store_for_exec = Arc::clone(&store);
    let (columns, rows) = tokio::task::spawn_blocking(move || {
        static EMPTY_REG: std::sync::OnceLock<ProcedureRegistry> = std::sync::OnceLock::new();
        let registry = EMPTY_REG.get_or_init(ProcedureRegistry::new);
        let cache = plan_cache::global();
        let plan = if let Some((_cached_ast, cached_plan)) = cache.get(&cypher) {
            metrics::counter!("plan_cache_hits_total").increment(1);
            cached_plan
        } else {
            metrics::counter!("plan_cache_misses_total").increment(1);
            let ast = parse_query(&cypher).map_err(|e| ApiError::bad_request(e.to_string()))?;
            let planner = QueryPlanner::new(store_for_exec.as_ref(), &graph_name);
            let plan = planner
                .plan(&ast)
                .map_err(|e| ApiError::bad_request(e.to_string()))?;
            cache.insert(&cypher, ast, plan.clone());
            plan
        };
        let mut budget = configured_budget();
        if let Some(rid) = request_id {
            budget.set_request_id(rid);
        }
        let result = QueryExecutor::with_context(store_for_exec.as_ref(), registry, params)
            .execute(&graph_name, &plan, &mut budget)
            .map_err(ApiError::from)?;
        let rows = result
            .rows
            .iter()
            .map(|row| row_to_json(row, &result.columns, store_for_exec.as_ref(), &graph_name))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| ApiError::internal(format!("failed to serialize result: {e}")))?;
        Ok::<_, ApiError>((result.columns, rows))
    })
    .await
    .map_err(|_| ApiError::internal("query executor panicked"))??;

    let elapsed_ms = start.elapsed().as_millis() as u64;

    Ok(Json(QueryResponse {
        columns,
        rows,
        elapsed_ms,
    }))
}
