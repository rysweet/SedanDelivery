//! POST /graphs/{name}/explain — plan inspection.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::Json;

use cypher_core::parser::parse_query;
use cypher_core::planner::QueryPlanner;
use cypher_core::storage::validate_graph_name;

use crate::error::ApiError;
use crate::models::{CypherBody, ExplainResponse};
use crate::routes::helpers::require_graph;
use crate::state::AppState;

use super::common::reject_oversized_query;

pub(super) async fn explain_handler(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<CypherBody>,
) -> Result<Json<ExplainResponse>, ApiError> {
    let name = body.database.as_ref().unwrap_or(&name);
    validate_graph_name(name).map_err(ApiError::from)?;
    reject_oversized_query(&body.cypher)?;
    require_graph(&state, name).await?;

    let store = Arc::clone(&state.store);
    let graph_name = name.to_string();
    let cypher = body.cypher.clone();
    let plan_json = tokio::task::spawn_blocking(move || {
        let ast = parse_query(&cypher).map_err(|e| ApiError::bad_request(e.to_string()))?;
        let planner = QueryPlanner::new(store.as_ref(), &graph_name);
        let plan = planner
            .plan(&ast)
            .map_err(|e| ApiError::bad_request(e.to_string()))?;

        let graph_stats =
            cypher_core::planner::collect_graph_statistics(store.as_ref(), &graph_name)
                .map_err(ApiError::from)?;
        let estimates = cypher_core::planner::estimate_cardinality(&plan, &graph_stats);

        Ok::<_, ApiError>(operator_to_json(&plan, &estimates))
    })
    .await
    .map_err(|_| ApiError::internal("explain executor panicked"))??;

    Ok(Json(ExplainResponse { plan: plan_json }))
}

/// Convert an operator tree to a JSON plan tree with estimated rows.
fn operator_to_json(
    op: &cypher_core::planner::Operator,
    estimates: &std::collections::HashMap<*const cypher_core::planner::Operator, f64>,
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
