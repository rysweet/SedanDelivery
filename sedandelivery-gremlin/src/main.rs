use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cypher_core::storage::GraphStore;
use sedandelivery_gremlin::{connection, health};
use tokio::net::TcpListener;
use tokio::sync::Semaphore;

/// Create the graph store based on environment configuration.
///
/// Delegates to [`sd2_redis_store::create_store::create_store`].
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
    let max_connections: usize =
        cypher_core::config::parse_env_range("GREMLIN_MAX_CONNECTIONS", 1024usize, 1, 65535);
    let idle_timeout_secs: u64 =
        cypher_core::config::parse_env_range("GREMLIN_IDLE_TIMEOUT_SECS", 300u64, 1, 86400);
    let max_lifetime_secs: u64 = cypher_core::config::parse_env_range(
        "GREMLIN_MAX_CONNECTION_LIFETIME_SECS",
        1800u64,
        0,
        86400,
    );
    let idle_timeout = Duration::from_secs(idle_timeout_secs);
    let max_lifetime = Duration::from_secs(max_lifetime_secs);

    // ── Authentication validation ────────────────────────────────────────
    // Fail fast if GREMLIN_PASSWORD is not set and no explicit opt-out is
    // provided.  Mirrors the VALKEY_PASSWORD pattern in entrypoint.sh.
    if std::env::var("GREMLIN_PASSWORD")
        .unwrap_or_default()
        .is_empty()
    {
        if std::env::var("GREMLIN_ALLOW_NO_AUTH").as_deref() == Ok("true") {
            tracing::warn!(
                "GREMLIN_PASSWORD not set — running WITHOUT authentication \
                 (GREMLIN_ALLOW_NO_AUTH=true). Do NOT use in production!"
            );
        } else {
            eprintln!(
                "ERROR: GREMLIN_PASSWORD is not set. Refusing to start without authentication.\n\
                 Set GREMLIN_PASSWORD to a strong secret, or set \
                 GREMLIN_ALLOW_NO_AUTH=true for local development only."
            );
            std::process::exit(1);
        }
    }

    // ── Store ────────────────────────────────────────────────────────────
    let store = create_store();
    tracing::info!("graph store initialised");

    // ── Shared state ─────────────────────────────────────────────────────
    let active_connections = Arc::new(AtomicU64::new(0));
    let connection_semaphore = Arc::new(Semaphore::new(max_connections));
    let start_time = Instant::now();

    // ── Health endpoint ──────────────────────────────────────────────────
    health::spawn_health_server(health::HealthState {
        active_connections: Arc::clone(&active_connections),
        start_time,
        store: Arc::clone(&store),
    });

    // ── WebSocket accept loop ────────────────────────────────────────────
    let listener = TcpListener::bind(&bind_addr).await.unwrap_or_else(|e| {
        eprintln!("failed to bind {bind_addr}: {e}");
        std::process::exit(1);
    });
    tracing::info!(%bind_addr, "gremlin WebSocket server listening");

    // Listen for both SIGINT (Ctrl+C) and SIGTERM (container orchestration).
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
            Err(_) => {
                tracing::info!("connection semaphore closed, shutting down");
                break;
            }
        };
        let store = Arc::clone(&store);
        let active = Arc::clone(&active_connections);
        active.fetch_add(1, Ordering::Relaxed);

        tokio::spawn(async move {
            let _permit = permit;
            let _guard = ConnectionGuard(Arc::clone(&active));
            tracing::debug!(%peer_addr, "new connection");

            if let Err(e) =
                connection::handle_connection(stream, store, idle_timeout, max_lifetime).await
            {
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
