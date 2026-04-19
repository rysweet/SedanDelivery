//! POST /graphs/{name}/profile — execution profiling.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use axum::extract::{Path, State};
use axum::Json;

use cypher_core::executor::QueryExecutor;
use cypher_core::parser::parse_query;
use cypher_core::planner::QueryPlanner;
use cypher_core::procedure::ProcedureRegistry;
use cypher_core::storage::validate_graph_name;

use crate::error::ApiError;
use crate::models::{CypherBody, ProfileResponse};
use crate::routes::helpers::require_graph;
use crate::serialize::row_to_json;
use crate::state::AppState;

use super::common::{
    configured_budget, convert_params, current_request_id, reject_oversized_query,
};

#[tracing::instrument(name = "gql.profile", skip_all, fields(graph = %name))]
pub(super) async fn profile_handler(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<CypherBody>,
) -> Result<Json<ProfileResponse>, ApiError> {
    let name = body.database.as_ref().unwrap_or(&name);
    validate_graph_name(name).map_err(ApiError::from)?;
    reject_oversized_query(&body.cypher)?;
    require_graph(&state, name).await?;

    let store = Arc::clone(&state.store);
    let start = Instant::now();

    let params = convert_params(body.params.as_ref())?;

    let request_id = current_request_id();

    // Parse, plan, execute with profiling, and build the annotated plan JSON
    // all inside a single spawn_blocking call. This is critical because the
    // per-operator profile stats are keyed by operator pointer addresses, which
    // are only valid while the plan lives at the same memory location (i.e.,
    // before it is moved out of the closure).
    let store_for_exec = Arc::clone(&store);
    let name_for_exec = name.to_string();
    let cypher = body.cypher.clone();
    let (columns, rows, plan_json) = tokio::task::spawn_blocking(move || {
        static EMPTY_REG: std::sync::OnceLock<ProcedureRegistry> = std::sync::OnceLock::new();
        let registry = EMPTY_REG.get_or_init(ProcedureRegistry::new);

        let ast = parse_query(&cypher).map_err(|e| ApiError::bad_request(e.to_string()))?;
        let planner = QueryPlanner::new(store_for_exec.as_ref(), &name_for_exec);
        let plan = planner.plan(&ast).map_err(ApiError::from)?;

        let mut budget = configured_budget();
        budget.enable_profiling();
        if let Some(rid) = request_id {
            budget.set_request_id(rid);
        }
        let result = QueryExecutor::with_context(store_for_exec.as_ref(), registry, params)
            .execute(&name_for_exec, &plan, &mut budget)?;
        let profile_stats = budget.take_profile_stats();

        // Build the annotated plan JSON while the plan is still in place
        // (pointer addresses match the profile stats keys).
        let graph_stats = cypher_core::planner::collect_graph_statistics(
            store_for_exec.as_ref(),
            &name_for_exec,
        )?;
        let estimates = cypher_core::planner::estimate_cardinality(&plan, &graph_stats);
        let plan_json = operator_to_profile_json(&plan, &estimates, &profile_stats);

        let rows = result
            .rows
            .iter()
            .map(|row| {
                row_to_json(
                    row,
                    &result.columns,
                    store_for_exec.as_ref(),
                    &name_for_exec,
                )
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| ApiError::internal(format!("failed to serialize result: {e}")))?;

        Ok::<_, ApiError>((result.columns, rows, plan_json))
    })
    .await
    .map_err(|_| ApiError::internal("profile executor panicked"))??;

    let elapsed_ms = start.elapsed().as_millis() as u64;

    Ok(Json(ProfileResponse {
        columns,
        rows,
        plan: plan_json,
        elapsed_ms,
    }))
}

/// Convert an operator tree to a JSON plan tree with both estimated and actual
/// row counts. Used by the PROFILE endpoint.
fn operator_to_profile_json(
    op: &cypher_core::planner::Operator,
    estimates: &std::collections::HashMap<*const cypher_core::planner::Operator, f64>,
    profile_stats: &Option<HashMap<usize, cypher_core::executor::OperatorProfileStats>>,
) -> serde_json::Value {
    let op_type = format!("{:?}", op)
        .split(['(', '{'])
        .next()
        .unwrap_or("Unknown")
        .trim()
        .to_string();
    let estimated_rows = estimates
        .get(&(op as *const cypher_core::planner::Operator))
        .copied()
        .unwrap_or(1.0);

    let op_addr = op as *const cypher_core::planner::Operator as usize;
    let (actual_rows, db_hits) = profile_stats
        .as_ref()
        .and_then(|stats| stats.get(&op_addr))
        .map(|s| (s.rows as u64, s.db_hits as u64))
        .unwrap_or((0, 0));

    let children: Vec<serde_json::Value> = op
        .children()
        .into_iter()
        .map(|child| operator_to_profile_json(child, estimates, profile_stats))
        .collect();
    serde_json::json!({
        "operatorType": op_type,
        "estimatedRows": estimated_rows,
        "rows": actual_rows,
        "dbHits": db_hits,
        "children": children,
    })
}
