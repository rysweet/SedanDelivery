//! Health HTTP endpoint for sedandelivery-gremlin.
//!
//! Spawns a lightweight axum server on a separate port (default 8183)
//! exposing `GET /health`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use gql_core::storage::GraphStore;
use serde::Serialize;

/// Shared state for the health server.
#[derive(Clone)]
pub struct HealthState {
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

/// Spawn the health HTTP server as a background tokio task.
pub fn spawn_health_server(state: HealthState) {
    let port: u16 = std::env::var("GREMLIN_HEALTH_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8183);

    let bind_addr = std::env::var("GREMLIN_HEALTH_BIND_ADDR")
        .unwrap_or_else(|_| "0.0.0.0".to_string());

    let sock_addr: std::net::SocketAddr = format!("{bind_addr}:{port}")
        .parse()
        .unwrap_or_else(|_| {
            tracing::warn!("invalid GREMLIN_HEALTH_BIND_ADDR, falling back to 0.0.0.0:{port}");
            format!("0.0.0.0:{port}").parse().expect("fallback must parse")
        });

    let app = Router::new()
        .route("/health", get(health_handler))
        .with_state(state);

    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(sock_addr)
            .await
            .expect("failed to bind health HTTP listener");
        tracing::info!(%sock_addr, "health server listening");
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
