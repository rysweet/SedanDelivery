//! Gremlin WebSocket server binary for SedanDelivery.
//!
//! Thin entry-point that configures logging, creates the graph store,
//! and accepts WebSocket connections — delegating to the library crate.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cypher_core::storage::GraphStore;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;

fn create_store() -> Arc<dyn GraphStore> {
    sd2_redis_store::create_store::create_store()
}

fn env_or(name: &str, default: &str) -> String {
    match std::env::var(name) {
        Ok(v) => v,
        Err(_) => {
            tracing::info!(%name, %default, "env var not set, using default");
            default.to_string()
        }
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(true)
        .init();

    let bind_addr = env_or("GREMLIN_SERVER_BIND_ADDR", "0.0.0.0:8182");
    let max_connections: usize =
        cypher_core::config::parse_env_range("GREMLIN_SERVER_MAX_CONNECTIONS", 1024usize, 1, 65535);
    let idle_timeout_secs: u64 =
        cypher_core::config::parse_env_range("GREMLIN_SERVER_IDLE_TIMEOUT_SECS", 300u64, 1, 86400);
    let idle_timeout = Duration::from_secs(idle_timeout_secs);

    let store = create_store();
    tracing::info!("graph store initialised");

    // Pre-create default graph
    match store.create_graph("default") {
        Ok(()) => {}
        Err(cypher_core::error::StorageError::GraphAlreadyExists { .. }) => {}
        Err(e) => panic!("failed to create default graph: {e}"),
    }

    let active_connections = Arc::new(AtomicU64::new(0));
    let connection_semaphore = Arc::new(Semaphore::new(max_connections));

    let listener = TcpListener::bind(&bind_addr).await.unwrap_or_else(|e| {
        eprintln!("failed to bind {bind_addr}: {e}");
        std::process::exit(1);
    });
    tracing::info!(%bind_addr, "gremlin-server WebSocket server listening");

    let auth_token: Option<Arc<str>> = match std::env::var("GREMLIN_AUTH_TOKEN") {
        Ok(v) if !v.is_empty() => {
            tracing::info!("Gremlin WebSocket auth enabled via GREMLIN_AUTH_TOKEN");
            Some(Arc::from(v.as_str()))
        }
        _ => {
            tracing::info!("GREMLIN_AUTH_TOKEN not set, auth disabled");
            None
        }
    };

    let graph = env_or("GREMLIN_SERVER_DEFAULT_GRAPH", "default");

    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("failed to register SIGTERM handler");

    loop {
        let accept_result = tokio::select! {
            biased;
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("received SIGINT, shutting down");
                break;
            }
            _ = sigterm.recv() => {
                tracing::info!("received SIGTERM, shutting down");
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

        let permit = match Arc::clone(&connection_semaphore).acquire_owned().await {
            Ok(permit) => permit,
            Err(_) => break,
        };
        let store = Arc::clone(&store);
        let active = Arc::clone(&active_connections);
        active.fetch_add(1, Ordering::Relaxed);
        let graph = graph.clone();
        let auth_token = auth_token.clone();

        tokio::spawn(async move {
            let _permit = permit;
            let _guard = ConnectionGuard(Arc::clone(&active));
            tracing::debug!(%peer_addr, "new connection");

            if let Err(e) = sedandelivery_gremlin_server::handle_connection(
                stream,
                store,
                idle_timeout,
                &graph,
                auth_token,
            )
            .await
            {
                tracing::warn!(%peer_addr, "connection error: {e}");
            }
        });
    }
}

struct ConnectionGuard(Arc<AtomicU64>);

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}
