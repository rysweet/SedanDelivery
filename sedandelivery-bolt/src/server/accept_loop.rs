//! TCP listener bind, accept loop, and per-connection task spawning,
//! plus graceful drain on shutdown.

use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use socket2::SockRef;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;

use cypher_core::storage::GraphStore;

use super::config::{self, ServerConfig};
use super::connection_task::{self, ConnectionContext};

/// Bind the listener, run the accept loop until SIGINT/SIGTERM, then drain
/// active connections.
pub(crate) async fn serve(
    cfg: ServerConfig,
    store: Arc<dyn GraphStore>,
    tls_acceptor: Option<tokio_rustls::TlsAcceptor>,
    active_connections: Arc<AtomicU64>,
) {
    let tcp_keepalive = socket2::TcpKeepalive::new()
        .with_time(Duration::from_secs(cfg.keepalive_secs))
        .with_interval(Duration::from_secs(30))
        .with_retries(3);
    tracing::info!(
        keepalive_secs = cfg.keepalive_secs,
        "TCP keepalive enabled (Azure LB idle timeout mitigation)"
    );

    let per_ip_conns: Arc<DashMap<IpAddr, usize>> = Arc::new(DashMap::new());
    let semaphore = Arc::new(Semaphore::new(cfg.max_connections));

    let listener = TcpListener::bind(&cfg.bind_addr)
        .await
        .expect("failed to bind Bolt TCP listener");
    tracing::info!(
        "sedandelivery-bolt listening on {} (max_connections={}, idle_timeout={}s, max_connection_lifetime={}s, write_concurrency={}, handshake_timeout_ms={})",
        cfg.bind_addr,
        cfg.max_connections,
        cfg.idle_timeout_secs,
        cfg.max_connection_lifetime_secs,
        cfg.write_concurrency,
        cfg.handshake_timeout_ms,
    );

    let connection_counter = Arc::new(AtomicU64::new(0));
    // Single global bookmark counter shared across all sessions so that
    // bookmarks produced by one session are meaningful when passed to another.
    let global_bookmark = Arc::new(sedandelivery_bolt::bookmark::BookmarkCounter::from_env());
    // Write concurrency semaphore for transaction timeout contention.
    // Controls how many write queries can execute concurrently. Autocommit
    // writes without tx_timeout bypass this semaphore entirely (safe because
    // Redis operations are atomic and single-statement queries need no
    // cross-session isolation). When tx_timeout is specified, the semaphore
    // detects contention and returns LockClientStopped on timeout.
    let write_semaphore = Arc::new(tokio::sync::Semaphore::new(cfg.write_concurrency as usize));

    // --- Graceful shutdown signal ---
    let mut shutdown_rx = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("failed to register SIGTERM handler");

    tracing::info!("accepting connections");

    loop {
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

        if let Err(e) = SockRef::from(&stream).set_tcp_keepalive(&tcp_keepalive) {
            tracing::warn!(%peer_addr, "failed to set TCP keepalive: {e}");
        }

        // Per-IP connection limit check.
        let per_ip_guard = {
            let ip = peer_addr.ip();
            match sedandelivery_bolt::connection::per_ip_guard::PerIpGuard::try_new(
                Arc::clone(&per_ip_conns),
                ip,
                cfg.max_conns_per_ip,
            ) {
                Some(guard) => guard,
                None => {
                    tracing::warn!(
                        %peer_addr,
                        max_conns_per_ip = cfg.max_conns_per_ip,
                        "per-IP connection limit reached, dropping connection"
                    );
                    sedandelivery_bolt::metrics::connection_rejected();
                    continue;
                }
            }
        };

        // Acquire a semaphore permit (blocks if at max_connections).
        let permit = match Arc::clone(&semaphore).acquire_owned().await {
            Ok(permit) => permit,
            Err(_) => {
                tracing::info!("semaphore closed, rejecting connection");
                break;
            }
        };

        let conn_id = connection_counter.fetch_add(1, Ordering::Relaxed);
        let ctx = ConnectionContext {
            stream,
            peer_addr,
            conn_id,
            store: Arc::clone(&store),
            bookmark: Arc::clone(&global_bookmark),
            write_semaphore: Arc::clone(&write_semaphore),
            active: Arc::clone(&active_connections),
            tls: tls_acceptor.clone(),
            idle_timeout: cfg.idle_timeout,
            max_connection_lifetime: cfg.max_connection_lifetime,
            handshake_timeout: cfg.handshake_timeout,
            handshake_timeout_ms: cfg.handshake_timeout_ms,
        };

        sedandelivery_bolt::metrics::connection_accepted();

        ctx.active.fetch_add(1, Ordering::Relaxed);
        let current = ctx.active.load(Ordering::Relaxed);
        tracing::info!(%peer_addr, conn_id, active_connections = current, "new connection");

        let handle = tokio::spawn(async move {
            // _permit returns to the semaphore when dropped, allowing a new connection.
            let _permit = permit;
            // _per_ip_guard decrements the per-IP connection count when dropped.
            let _per_ip_guard = per_ip_guard;
            connection_task::run(ctx).await;
        });

        // Lightweight watcher: detect and log panics without blocking the accept loop.
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

    drain(&semaphore, &active_connections).await;
}

/// Stop accepting new permits and wait up to `BOLT_DRAIN_TIMEOUT_SECS` for
/// active sessions to finish.
async fn drain(semaphore: &Semaphore, active_connections: &AtomicU64) {
    semaphore.close();

    let drain_timeout = config::drain_timeout();
    tracing::info!(
        active_connections = active_connections.load(Ordering::Relaxed),
        drain_timeout_secs = drain_timeout.as_secs(),
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

    tracing::info!("sedandelivery-bolt shutdown complete");
}
