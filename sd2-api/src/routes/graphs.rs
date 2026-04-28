//! Graph administration endpoints.
//!
//! GET    /graphs
//! POST   /graphs
//! DELETE /graphs/{name}
//! GET    /graphs/{name}/stats
//! POST   /admin/graphs/{name}/audit
//! POST   /admin/graphs/{name}/finalize-delete

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;

use cypher_core::storage::validate_graph_name;

use crate::error::ApiError;
use crate::models::{CreateGraphBody, GraphListResponse, GraphStatsResponse};
use crate::routes::helpers::run_store_blocking;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/graphs", get(list_graphs))
        .route("/graphs", post(create_graph))
        .route("/graphs/{name}", delete(delete_graph))
        .route("/graphs/{name}/stats", get(graph_stats))
        .route("/admin/graphs/{name}/audit", post(audit_graph_metadata))
        .route(
            "/admin/graphs/{name}/finalize-delete",
            post(finalize_delete_graph),
        )
}

#[derive(Debug, Deserialize, Default)]
struct AuditGraphQuery {
    #[serde(default)]
    repair: bool,
}

#[derive(Debug, Deserialize, Default)]
struct FinalizeDeleteQuery {
    #[serde(default)]
    confirm: bool,
}

// ── GET /graphs ───────────────────────────────────────────────────────────────

async fn list_graphs(State(state): State<AppState>) -> Result<Json<GraphListResponse>, ApiError> {
    let mut graphs = run_store_blocking(&state, move |store| {
        store.list_graphs().map_err(ApiError::from)
    })
    .await?;
    graphs.sort();
    Ok(Json(GraphListResponse { graphs }))
}

// ── POST /graphs ──────────────────────────────────────────────────────────────

async fn create_graph(
    State(state): State<AppState>,
    Json(body): Json<CreateGraphBody>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    validate_graph_name(&body.name).map_err(ApiError::from)?;
    let graph_name = body.name.clone();
    run_store_blocking(&state, move |store| {
        store.create_graph(&graph_name).map_err(ApiError::from)?;
        if let Ok(graphs) = store.list_graphs() {
            crate::metrics::set_active_graphs(graphs.len());
        }
        Ok(())
    })
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "name": body.name })),
    ))
}

// ── DELETE /graphs/{name} ─────────────────────────────────────────────────────

async fn delete_graph(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    validate_graph_name(&name).map_err(ApiError::from)?;
    let graph_name = name.clone();
    run_store_blocking(&state, move |store| {
        store.delete_graph(&graph_name).map_err(ApiError::from)?;
        if let Ok(graphs) = store.list_graphs() {
            crate::metrics::set_active_graphs(graphs.len());
        }
        Ok(())
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── GET /graphs/{name}/stats ──────────────────────────────────────────────────

async fn graph_stats(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<GraphStatsResponse>, ApiError> {
    validate_graph_name(&name).map_err(ApiError::from)?;
    let graph_name = name.clone();
    let stats = run_store_blocking(&state, move |store| {
        if !store.graph_exists(&graph_name).map_err(ApiError::from)? {
            return Err(ApiError::not_found(format!(
                "graph '{graph_name}' not found"
            )));
        }
        store.graph_stats(&graph_name).map_err(ApiError::from)
    })
    .await?;
    Ok(Json(GraphStatsResponse {
        node_count: stats.node_count,
        edge_count: stats.edge_count,
        label_count: stats.label_count,
        rel_type_count: stats.rel_type_count,
    }))
}

// ── POST /admin/graphs/{name}/audit ───────────────────────────────────────────

async fn audit_graph_metadata(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(query): Query<AuditGraphQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    validate_graph_name(&name).map_err(ApiError::from)?;
    let graph_name = name.clone();
    let repair = query.repair;
    let report = run_store_blocking(&state, move |store| {
        let Some(redis_store) = store
            .as_any()
            .downcast_ref::<sd2_redis_store::RedisGraphStore>()
        else {
            return Err(ApiError::new(
                StatusCode::NOT_IMPLEMENTED,
                "graph metadata audit is only implemented for Redis-backed stores",
            ));
        };

        redis_store
            .audit_graph_metadata(&graph_name, repair)
            .map_err(ApiError::from)
    })
    .await?;

    Ok(Json(serde_json::json!({
        "graph": report.graph,
        "repair": report.repair,
        "staleLabels": report.stale_labels,
        "staleReltypes": report.stale_reltypes,
        "stalePropidx": report.stale_propidx,
        "deletingOrphaned": report.deleting_orphaned,
    })))
}

// ── POST /admin/graphs/{name}/finalize-delete ─────────────────────────────────
//
// Operator-driven finalization of a stuck `_deleting` marker (F4 in the
// cluster consistency hardening package, issue #1769).
//
// Contract:
//   - Requires `?confirm=true` (operator confirmation). Missing/false → 400.
//   - Implemented for Redis-backed stores only; in-memory store → 501.
//   - Refuses to act unless the graph is in the `deletingOrphaned=true`
//     state per the audit report — returns 409 Conflict otherwise.
//   - On success: clears `G:{g}:_deleting` and SREMs the graph name from
//     the global registry, then returns the deleted keys plus the
//     registry membership cleared (if any).
//   - Emits a `tracing::warn!` on every invocation so operator action is
//     captured in the standard request-tracing pipeline (no separate
//     audit log table — the warn record is the audit trail).

async fn finalize_delete_graph(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(query): Query<FinalizeDeleteQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    validate_graph_name(&name).map_err(ApiError::from)?;
    if !query.confirm {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "finalize-delete requires explicit operator confirmation: append ?confirm=true",
        ));
    }
    tracing::warn!(
        target: "sd2_api::admin",
        graph = %name,
        action = "finalize_delete",
        "destructive admin endpoint invoked: operator-driven finalization of stuck _deleting marker"
    );
    let graph_name = name.clone();
    let report = run_store_blocking(&state, move |store| {
        let Some(redis_store) = store
            .as_any()
            .downcast_ref::<sd2_redis_store::RedisGraphStore>()
        else {
            return Err(ApiError::new(
                StatusCode::NOT_IMPLEMENTED,
                "finalize-delete is only implemented for Redis-backed stores",
            ));
        };

        redis_store
            .finalize_delete(&graph_name)
            .map_err(ApiError::from)
    })
    .await?;

    tracing::warn!(
        target: "sd2_api::admin",
        graph = %report.graph,
        action = "finalize_delete",
        finalized = report.finalized,
        deleted_keys = ?report.deleted_keys,
        registry_removed_from = ?report.registry_removed_from,
        "finalize-delete completed"
    );

    Ok(Json(serde_json::json!({
        "graph": report.graph,
        "finalized": report.finalized,
        "deletedKeys": report.deleted_keys,
        "registryRemovedFrom": report.registry_removed_from,
    })))
}
