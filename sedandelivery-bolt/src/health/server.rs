//! HTTP server setup: spawning, address resolution, and router construction.

use axum::routing::get;
use axum::Router;

use super::handlers::{health_handler, metrics_handler};
use super::state::HealthState;

/// Spawn the health/metrics HTTP server as a background tokio task.
///
/// The server binds to `{bind_addr}:{port}` where bind_addr defaults to
/// `0.0.0.0` (override with `BOLT_HEALTH_BIND_ADDR`) and port defaults
/// to 7688 (override with `BOLT_HEALTH_PORT`).
pub fn spawn_health_server(state: HealthState) {
    spawn_health_server_with_addr(state, None);
}

/// Same as [`spawn_health_server`] but lets the caller supply an explicit
/// `SocketAddr`, bypassing env-var lookup. Used by tests so they don't have
/// to mutate the process environment (`std::env::set_var` is `unsafe` in
/// Rust 2024).
pub fn spawn_health_server_with_addr(
    state: HealthState,
    addr_override: Option<std::net::SocketAddr>,
) {
    let sock_addr = addr_override.unwrap_or_else(resolve_health_addr_from_env);
    let bind_addr = sock_addr.ip().to_string();
    let port = sock_addr.port();

    let app = build_router(state);

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

/// Resolve the health server's listen address from `BOLT_HEALTH_PORT` and
/// `BOLT_HEALTH_BIND_ADDR`, with safe fallbacks. Defaults to `0.0.0.0:7688`.
fn resolve_health_addr_from_env() -> std::net::SocketAddr {
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
        std::env::var("BOLT_HEALTH_BIND_ADDR").unwrap_or_else(|_| "0.0.0.0".to_string());

    // Pre-validate the full socket address to catch typos early.
    let sock_addr_str = format!("{bind_addr}:{port}");
    match sock_addr_str.parse() {
        Ok(addr) => addr,
        Err(_) => {
            tracing::warn!(
                "BOLT_HEALTH_BIND_ADDR={bind_addr:?} with port={port} does not form a valid SocketAddr, \
                 falling back to 0.0.0.0:{port}"
            );
            format!("0.0.0.0:{port}")
                .parse()
                .expect("fallback addr must parse")
        }
    }
}

/// Build the axum router. Extracted for testability.
pub(super) fn build_router(state: HealthState) -> Router {
    Router::new()
        .route("/health", get(health_handler))
        .route("/metrics", get(metrics_handler))
        .with_state(state)
}
