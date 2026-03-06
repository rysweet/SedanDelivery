//! Health and metrics HTTP endpoint for sedandelivery-bolt.
//!
//! Spawns a lightweight axum HTTP server on a separate port (default 7688)
//! that exposes:
//!   - `GET /health`  — JSON health status
//!   - `GET /metrics` — Prometheus text-format metrics

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use metrics_exporter_prometheus::PrometheusHandle;
use serde::Serialize;

/// Shared state for the health server.
#[derive(Clone)]
pub struct HealthState {
    pub prometheus_handle: PrometheusHandle,
    pub active_connections: Arc<AtomicU64>,
    pub start_time: Instant,
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    connections: u64,
    uptime_seconds: u64,
}

/// Spawn the health/metrics HTTP server as a background tokio task.
///
/// The server binds to `{bind_addr}:{port}` where bind_addr defaults to
/// `127.0.0.1` (override with `BOLT_HEALTH_BIND_ADDR`) and port defaults
/// to 7688 (override with `BOLT_HEALTH_PORT`).
pub fn spawn_health_server(state: HealthState) {
    let port = std::env::var("BOLT_HEALTH_PORT")
        .ok()
        .and_then(|v| match v.parse::<u16>() {
            Ok(val) => Some(val),
            Err(_) => {
                tracing::warn!("invalid value for BOLT_HEALTH_PORT: {v:?}, using default 7688");
                None
            }
        })
        .unwrap_or(7688);

    let bind_addr =
        std::env::var("BOLT_HEALTH_BIND_ADDR").unwrap_or_else(|_| "127.0.0.1".to_string());

    let app = Router::new()
        .route("/health", get(health_handler))
        .route("/metrics", get(metrics_handler))
        .with_state(state);

    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(format!("{bind_addr}:{port}"))
            .await
            .expect("failed to bind health HTTP listener");
        tracing::info!(%bind_addr, port, "health/metrics server listening");
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!("health server error: {e}");
        }
    });
}

async fn health_handler(State(state): State<HealthState>) -> impl IntoResponse {
    let resp = HealthResponse {
        status: "ok",
        connections: state.active_connections.load(Ordering::Relaxed),
        uptime_seconds: state.start_time.elapsed().as_secs(),
    };
    axum::Json(resp)
}

async fn metrics_handler(State(state): State<HealthState>) -> Response {
    let body = state.prometheus_handle.render();
    Response::builder()
        .header("content-type", "text/plain; version=0.0.4; charset=utf-8")
        .body(axum::body::Body::from(body))
        .unwrap()
}
