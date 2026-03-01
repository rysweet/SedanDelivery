use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::Semaphore;

use gql_core::storage::InMemoryGraphStore;

/// RAII guard that decrements the active-connection gauge on drop.
/// This ensures the counter is accurate even when a session task panics.
struct ConnectionGuard {
    active: Arc<AtomicU64>,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::Relaxed);
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let bind_addr = std::env::var("BOLT_BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:7687".to_string());

    // --- Connection lifecycle configuration ---

    let max_connections: usize = std::env::var("BOLT_MAX_CONNECTIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1024);

    let idle_timeout_secs: u64 = std::env::var("BOLT_IDLE_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300);
    let idle_timeout = Duration::from_secs(idle_timeout_secs);

    // Semaphore gates new connections once max_connections is reached.
    let semaphore = Arc::new(Semaphore::new(max_connections));

    // Active connection gauge (increment on connect, decrement on disconnect).
    let active_connections = Arc::new(AtomicU64::new(0));

    let store = Arc::new(InMemoryGraphStore::new());

    let listener = TcpListener::bind(&bind_addr)
        .await
        .expect("failed to bind Bolt TCP listener");
    tracing::info!(
        "sedan-bolt listening on {} (max_connections={}, idle_timeout={}s)",
        bind_addr,
        max_connections,
        idle_timeout_secs
    );

    // Monotonic connection ID (not the same as active count).
    let connection_counter = Arc::new(AtomicU64::new(0));
    // Single global bookmark counter shared across all sessions so that
    // bookmarks produced by one session are meaningful when passed to another.
    let global_bookmark = Arc::new(AtomicU64::new(0));
    // Global write lock for transaction timeout contention.
    // When a transaction performs a write, it acquires this lock. If another
    // transaction with a tx_timeout tries to write while the lock is held,
    // it will time out with Neo.TransientError.Transaction.LockClientStopped.
    let write_lock = Arc::new(tokio::sync::Mutex::new(()));

    // --- Graceful shutdown signal ---
    let mut shutdown_rx = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("failed to register SIGTERM handler");

    tracing::info!("accepting connections");

    loop {
        // Wait for either a new connection or a shutdown signal.
        let accept_result = tokio::select! {
            biased;

            _ = tokio::signal::ctrl_c() => {
                tracing::info!("received SIGINT, initiating graceful shutdown");
                break;
            }
            _ = shutdown_rx.recv() => {
                tracing::info!("received SIGTERM, initiating graceful shutdown");
                break;
            }
            result = listener.accept() => result,
        };

        let (stream, peer_addr) = match accept_result {
            Ok(pair) => pair,
            Err(e) => {
                tracing::error!("accept error (transient): {e}");
                continue;
            }
        };

        // Acquire a semaphore permit (blocks if at max_connections).
        let permit = match Arc::clone(&semaphore).acquire_owned().await {
            Ok(permit) => permit,
            Err(_) => {
                // Semaphore was closed — only happens during shutdown.
                tracing::info!("semaphore closed, rejecting connection");
                break;
            }
        };

        let store = Arc::clone(&store);
        let conn_id = connection_counter.fetch_add(1, Ordering::Relaxed);
        let bookmark = Arc::clone(&global_bookmark);
        let wl = Arc::clone(&write_lock);
        let active = Arc::clone(&active_connections);

        active.fetch_add(1, Ordering::Relaxed);
        let current = active.load(Ordering::Relaxed);
        tracing::info!(%peer_addr, conn_id, active_connections = current, "new connection");

        let handle = tokio::spawn(async move {
            // _permit is held for the lifetime of this task; when dropped it
            // returns the permit to the semaphore, allowing a new connection.
            let _permit = permit;

            // ConnectionGuard ensures active count is decremented even on panic.
            let _guard = ConnectionGuard { active };

            // Wrap session::run in a timeout so idle connections are reaped.
            let result = tokio::time::timeout(
                idle_timeout,
                sedan_bolt::connection::session::run(stream, store, conn_id, bookmark, wl),
            )
            .await;

            match result {
                Ok(Ok(())) => {
                    tracing::info!(%peer_addr, conn_id, "connection closed normally");
                }
                Ok(Err(e)) => {
                    tracing::warn!(%peer_addr, conn_id, "session error: {}", e);
                }
                Err(_elapsed) => {
                    tracing::info!(
                        %peer_addr,
                        conn_id,
                        timeout_secs = idle_timeout_secs,
                        "connection closed: idle timeout"
                    );
                }
            }

            // _guard and _permit drop here, decrementing active count and
            // releasing the semaphore permit.
        });

        // Spawn a lightweight watcher to detect and log panics without
        // blocking the accept loop.
        tokio::spawn(async move {
            if let Err(join_err) = handle.await {
                if join_err.is_panic() {
                    tracing::error!(
                        conn_id,
                        "session task panicked (connection resources cleaned up by guards)"
                    );
                } else {
                    tracing::warn!(conn_id, "session task cancelled: {}", join_err);
                }
            }
        });
    }

    // --- Graceful drain ---
    // Stop handing out new permits.
    semaphore.close();

    let drain_timeout = Duration::from_secs(30);
    tracing::info!(
        active_connections = active_connections.load(Ordering::Relaxed),
        drain_timeout_secs = 30,
        "draining active connections"
    );

    let drain_start = tokio::time::Instant::now();
    while active_connections.load(Ordering::Relaxed) > 0 {
        if drain_start.elapsed() >= drain_timeout {
            tracing::warn!(
                active_connections = active_connections.load(Ordering::Relaxed),
                "drain timeout exceeded, shutting down with active connections"
            );
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    tracing::info!("sedan-bolt shutdown complete");
}
