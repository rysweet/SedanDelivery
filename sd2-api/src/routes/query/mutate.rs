//! POST /graphs/{name}/mutate — write queries.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::Json;

use cypher_core::executor::QueryExecutor;
use cypher_core::parser::parse_query;
use cypher_core::plan_cache;
use cypher_core::planner::QueryPlanner;
use cypher_core::procedure::ProcedureRegistry;
use cypher_core::storage::validate_graph_name;

use crate::error::ApiError;
use crate::models::{CypherBody, MutateResponse};
use crate::routes::helpers::require_graph;
use crate::state::AppState;

use super::common::{
    configured_budget, convert_params, current_request_id, reject_oversized_query,
};

#[tracing::instrument(name = "gql.mutate", skip_all, fields(graph = %name))]
pub(super) async fn mutate_handler(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<CypherBody>,
) -> Result<Json<MutateResponse>, ApiError> {
    let name = body.database.as_ref().unwrap_or(&name);
    validate_graph_name(name).map_err(ApiError::from)?;
    reject_oversized_query(&body.cypher)?;
    require_graph(&state, name).await?;

    let store = Arc::clone(&state.store);
    let graph_name = name.to_string();
    let cypher = body.cypher.clone();
    let params = convert_params(body.params.as_ref())?;

    let request_id = current_request_id();

    // Move the Arc and plan into spawn_blocking so CPU-bound execution
    // does not block the Tokio async thread pool.
    let store_for_exec = Arc::clone(&store);
    let name_for_exec = graph_name;
    let result = tokio::task::spawn_blocking(move || {
        static EMPTY_REG: std::sync::OnceLock<ProcedureRegistry> = std::sync::OnceLock::new();
        let registry = EMPTY_REG.get_or_init(ProcedureRegistry::new);
        let cache = plan_cache::global();
        let plan = if let Some((_cached_ast, cached_plan)) = cache.get(&cypher) {
            metrics::counter!("plan_cache_hits_total").increment(1);
            cached_plan
        } else {
            metrics::counter!("plan_cache_misses_total").increment(1);
            let ast = parse_query(&cypher).map_err(|e| ApiError::bad_request(e.to_string()))?;
            let planner = QueryPlanner::new(store_for_exec.as_ref(), &name_for_exec);
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
        QueryExecutor::with_context(store_for_exec.as_ref(), registry, params)
            .execute(&name_for_exec, &plan, &mut budget)
            .map_err(ApiError::from)
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
