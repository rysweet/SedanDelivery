//! Endpoint handlers for `/health` and `/metrics`.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use super::state::{HealthResponse, HealthState};

pub(super) async fn health_handler(State(state): State<HealthState>) -> impl IntoResponse {
    let connections = state.active_connections.load(Ordering::Relaxed);
    let uptime_seconds = state.start_time.elapsed().as_secs();

    let store = Arc::clone(&state.store);
    let probe_result = tokio::task::spawn_blocking(move || store.graph_exists("__health_probe"))
        .await
        .unwrap_or_else(|e| Err(cypher_core::error::StorageError::Backend { msg: e.to_string() }));

    match probe_result {
        Ok(_) => (
            StatusCode::OK,
            axum::Json(HealthResponse {
                status: "ok",
                connections,
                uptime_seconds,
                error: None,
            }),
        ),
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(HealthResponse {
                status: "degraded",
                connections,
                uptime_seconds,
                error: Some(e.to_string()),
            }),
        ),
    }
}

pub(super) async fn metrics_handler(State(state): State<HealthState>) -> Response {
    // Sync the backend error counter from sd2-redis-store into Prometheus
    // so it appears in the /metrics output on every scrape.
    crate::metrics::sync_backend_error_gauge();
    let body = state.prometheus_handle.render();
    Response::builder()
        .header("content-type", "text/plain; version=0.0.4; charset=utf-8")
        .body(axum::body::Body::from(body))
        .unwrap_or_else(|_| {
            Response::builder()
                .status(500)
                .body(axum::body::Body::from(
                    "internal error building metrics response",
                ))
                .expect("fallback response must be valid")
        })
}
