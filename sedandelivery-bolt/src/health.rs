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
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use gql_core::storage::GraphStore;
use metrics_exporter_prometheus::PrometheusHandle;
use serde::Serialize;

/// Shared state for the health server.
#[derive(Clone)]
pub struct HealthState {
    pub prometheus_handle: PrometheusHandle,
    pub active_connections: Arc<AtomicU64>,
    pub start_time: Instant,
    pub store: Arc<dyn GraphStore>,
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    connections: u64,
    uptime_seconds: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
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

    // Pre-validate the full socket address to catch typos early.
    let sock_addr_str = format!("{bind_addr}:{port}");
    let sock_addr: std::net::SocketAddr = match sock_addr_str.parse() {
        Ok(addr) => addr,
        Err(_) => {
            tracing::warn!(
                "BOLT_HEALTH_BIND_ADDR={bind_addr:?} with port={port} does not form a valid SocketAddr, \
                 falling back to 127.0.0.1:{port}"
            );
            format!("127.0.0.1:{port}")
                .parse()
                .expect("fallback addr must parse")
        }
    };

    let app = Router::new()
        .route("/health", get(health_handler))
        .route("/metrics", get(metrics_handler))
        .with_state(state);

    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(sock_addr)
            .await
            .expect("failed to bind health HTTP listener");
        tracing::info!(%bind_addr, port, "health/metrics server listening");
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!("health server error: {e}");
        }
    });
}

async fn health_handler(State(state): State<HealthState>) -> impl IntoResponse {
    let connections = state.active_connections.load(Ordering::Relaxed);
    let uptime_seconds = state.start_time.elapsed().as_secs();

    match state.store.graph_exists("__health_probe") {
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

async fn metrics_handler(State(state): State<HealthState>) -> Response {
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
