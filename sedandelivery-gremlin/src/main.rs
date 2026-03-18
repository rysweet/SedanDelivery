//! sedandelivery-gremlin — Gremlin WebSocket server for SedanDelivery.
//!
//! Accepts Gremlin bytecode over WebSocket (port 8182) and translates
//! traversals into gql-core query execution. Health endpoint on port 8183.

mod bridge;
mod connection;
mod health;
mod protocol;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use gql_core::storage::GraphStore;
use tokio::net::TcpListener;

/// Create the graph store based on environment configuration.
///
/// Delegates to [`gql_redis_store::create_store::create_store`].
fn create_store() -> Arc<dyn GraphStore> {
    gql_redis_store::create_store::create_store()
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

#[tokio::main]
async fn main() {
    // ── Logging ──────────────────────────────────────────────────────────
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(true)
        .init();

    // ── Configuration ────────────────────────────────────────────────────
    let bind_addr = env_or("GREMLIN_BIND_ADDR", "0.0.0.0:8182");

    // ── Store ────────────────────────────────────────────────────────────
    let store = create_store();
    tracing::info!("graph store initialised");

    // ── Shared state ─────────────────────────────────────────────────────
    let active_connections = Arc::new(AtomicU64::new(0));
    let start_time = Instant::now();

    // ── Health endpoint ──────────────────────────────────────────────────
    health::spawn_health_server(health::HealthState {
        active_connections: Arc::clone(&active_connections),
        start_time,
        store: Arc::clone(&store),
    });

    // ── WebSocket accept loop ────────────────────────────────────────────
    let listener = TcpListener::bind(&bind_addr)
        .await
        .unwrap_or_else(|e| {
            eprintln!("failed to bind {bind_addr}: {e}");
            std::process::exit(1);
        });
    tracing::info!(%bind_addr, "gremlin WebSocket server listening");

    loop {
        let accept_result = tokio::select! {
            biased;
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("received SIGINT, shutting down");
                break;
            }
            result = listener.accept() => result,
        };

        let (stream, peer_addr) = match accept_result {
            Ok(pair) => pair,
            Err(e) => {
                tracing::error!("accept error: {e}");
                continue;
            }
        };

        let store = Arc::clone(&store);
        let active = Arc::clone(&active_connections);
        active.fetch_add(1, Ordering::Relaxed);

        tokio::spawn(async move {
            let _guard = ConnectionGuard(Arc::clone(&active));
            tracing::debug!(%peer_addr, "new connection");

            if let Err(e) = connection::handle_connection(stream, store).await {
                tracing::warn!(%peer_addr, "connection error: {e}");
            }
        });
    }
}

/// RAII guard that decrements the active-connection counter on drop.
struct ConnectionGuard(Arc<AtomicU64>);

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}
