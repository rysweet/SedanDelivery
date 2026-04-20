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
use cypher_core::storage::GraphStore;
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

    let bind_addr =
        std::env::var("GREMLIN_HEALTH_BIND_ADDR").unwrap_or_else(|_| "0.0.0.0".to_string());

    let sock_addr: std::net::SocketAddr =
        format!("{bind_addr}:{port}").parse().unwrap_or_else(|_| {
            tracing::warn!("invalid GREMLIN_HEALTH_BIND_ADDR, falling back to 0.0.0.0:{port}");
            format!("0.0.0.0:{port}")
                .parse()
                .expect("fallback must parse")
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
    let store = Arc::clone(&state.store);

    match tokio::task::spawn_blocking(move || store.graph_exists("__health_probe")).await {
        Ok(Ok(_)) => (
            StatusCode::OK,
            axum::Json(HealthResponse {
                status: "ok",
                connections,
                uptime_seconds,
                error: None,
            }),
        ),
        Ok(Err(e)) => (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(HealthResponse {
                status: "error",
                connections,
                uptime_seconds,
                error: Some(e.to_string()),
            }),
        ),
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(HealthResponse {
                status: "error",
                connections,
                uptime_seconds,
                error: Some(format!("health probe task panicked: {e}")),
            }),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cypher_core::storage::InMemoryGraphStore;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::Mutex;

    // Health tests serialize on env vars + global port allocation.
    static ENV_LOCK: Mutex<()> = Mutex::const_new(());

    fn make_state() -> HealthState {
        HealthState {
            active_connections: Arc::new(AtomicU64::new(3)),
            start_time: Instant::now(),
            store: Arc::new(InMemoryGraphStore::new()),
        }
    }

    /// Acquire an unused TCP port by binding+dropping.
    fn pick_free_port() -> u16 {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let p = l.local_addr().unwrap().port();
        drop(l);
        p
    }

    async fn http_get(addr: &str, path: &str) -> (String, String) {
        // Retry briefly while the server is still binding.
        let mut last_err = None;
        for _ in 0..50 {
            match tokio::net::TcpStream::connect(addr).await {
                Ok(mut stream) => {
                    let req =
                        format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
                    stream.write_all(req.as_bytes()).await.unwrap();
                    let mut buf = Vec::new();
                    stream.read_to_end(&mut buf).await.unwrap();
                    let text = String::from_utf8_lossy(&buf).to_string();
                    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
                    return (head.to_string(), body.to_string());
                }
                Err(e) => {
                    last_err = Some(e);
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            }
        }
        panic!("could not connect to health server: {last_err:?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn health_endpoint_returns_ok_json() {
        let _g = ENV_LOCK.lock().await;
        let port = pick_free_port();
        std::env::set_var("GREMLIN_HEALTH_PORT", port.to_string());
        std::env::set_var("GREMLIN_HEALTH_BIND_ADDR", "127.0.0.1");

        spawn_health_server(make_state());

        let addr = format!("127.0.0.1:{port}");
        let (head, body) = http_get(&addr, "/health").await;

        assert!(head.starts_with("HTTP/1.1 200"), "head = {head}");
        let v: serde_json::Value = serde_json::from_str(&body).expect("body is JSON");
        assert_eq!(v["status"], "ok");
        assert_eq!(v["connections"], 3);
        assert!(v.get("uptime_seconds").is_some());

        std::env::remove_var("GREMLIN_HEALTH_PORT");
        std::env::remove_var("GREMLIN_HEALTH_BIND_ADDR");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn health_server_falls_back_when_bind_addr_invalid() {
        let _g = ENV_LOCK.lock().await;
        // We can't easily verify the *fallback bind* (it would conflict with the
        // 0.0.0.0 port), but we *can* verify spawn_health_server doesn't panic
        // when GREMLIN_HEALTH_BIND_ADDR is malformed: it should warn + use the
        // default address. Use a free high port.
        let port = pick_free_port();
        std::env::set_var("GREMLIN_HEALTH_PORT", port.to_string());
        std::env::set_var("GREMLIN_HEALTH_BIND_ADDR", "not-a-valid-addr!@#");

        // This must not panic.
        spawn_health_server(make_state());
        // Give the spawned task a chance to bind (or fail) before test exits.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        std::env::remove_var("GREMLIN_HEALTH_PORT");
        std::env::remove_var("GREMLIN_HEALTH_BIND_ADDR");
    }
}
