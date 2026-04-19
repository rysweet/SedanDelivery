//! Per-connection task body: TLS / WebSocket detection, session dispatch, and
//! result logging.

use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cypher_core::storage::GraphStore;
use tokio::net::TcpStream;

use sedandelivery_bolt::bookmark::BookmarkCounter;
use sedandelivery_bolt::connection::session::{self, SessionConfig, SessionError};

/// Inputs for a single accepted connection's session task.
pub(crate) struct ConnectionContext {
    pub stream: TcpStream,
    pub peer_addr: std::net::SocketAddr,
    pub conn_id: u64,
    pub store: Arc<dyn GraphStore>,
    pub bookmark: Arc<BookmarkCounter>,
    pub write_semaphore: Arc<tokio::sync::Semaphore>,
    pub active: Arc<AtomicU64>,
    pub tls: Option<tokio_rustls::TlsAcceptor>,
    pub idle_timeout: Duration,
    pub max_connection_lifetime: Option<Duration>,
    pub handshake_timeout: Duration,
    pub handshake_timeout_ms: u64,
}

/// RAII guard that decrements the active-connection gauge on drop.
/// This ensures the counter is accurate even when a session task panics.
pub(crate) struct ConnectionGuard {
    pub active: Arc<AtomicU64>,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::Relaxed);
        // Record disconnect metrics on drop (covers both normal close and panic).
        sedandelivery_bolt::metrics::connection_closed();
    }
}

/// Body of the per-connection task. Runs the appropriate session variant
/// (TLS, WebSocket, or raw Bolt TCP), then logs the result.
pub(crate) async fn run(ctx: ConnectionContext) {
    let ConnectionContext {
        stream,
        peer_addr,
        conn_id,
        store,
        bookmark,
        write_semaphore,
        active,
        tls,
        idle_timeout,
        max_connection_lifetime,
        handshake_timeout,
        handshake_timeout_ms,
    } = ctx;

    // ConnectionGuard ensures active count is decremented even on panic
    // and records disconnect metrics via connection_closed().
    let _guard = ConnectionGuard { active };

    // Idle timeout and max connection lifetime are enforced inside session::run
    // per-message, so we do not wrap the entire session in a single timeout.
    let result = if let Some(acceptor) = tls {
        run_tls(
            acceptor,
            stream,
            peer_addr,
            conn_id,
            store,
            bookmark,
            write_semaphore,
            idle_timeout,
            max_connection_lifetime,
            handshake_timeout,
            handshake_timeout_ms,
        )
        .await
    } else {
        run_plain_or_ws(
            stream,
            peer_addr,
            conn_id,
            store,
            bookmark,
            write_semaphore,
            idle_timeout,
            max_connection_lifetime,
        )
        .await
    };

    log_result(peer_addr, conn_id, result);
}

#[allow(clippy::too_many_arguments)]
async fn run_tls(
    acceptor: tokio_rustls::TlsAcceptor,
    stream: TcpStream,
    peer_addr: std::net::SocketAddr,
    conn_id: u64,
    store: Arc<dyn GraphStore>,
    bookmark: Arc<BookmarkCounter>,
    write_semaphore: Arc<tokio::sync::Semaphore>,
    idle_timeout: Duration,
    max_connection_lifetime: Option<Duration>,
    handshake_timeout: Duration,
    handshake_timeout_ms: u64,
) -> Result<(), SessionError> {
    match tokio::time::timeout(handshake_timeout, acceptor.accept(stream)).await {
        Ok(Ok(tls_stream)) => {
            tracing::debug!(conn_id, "TLS handshake complete");
            session::run(
                tls_stream,
                make_session_config(
                    store,
                    conn_id,
                    Some(peer_addr.ip()),
                    bookmark,
                    write_semaphore,
                    idle_timeout,
                    max_connection_lifetime,
                ),
            )
            .await
        }
        Ok(Err(e)) => {
            tracing::warn!(%peer_addr, conn_id, "TLS handshake failed: {e}");
            Ok(())
        }
        Err(_elapsed) => {
            tracing::warn!(
                %peer_addr,
                conn_id,
                handshake_timeout_ms,
                "pre-handshake timeout: slow client dropped"
            );
            Ok(())
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_plain_or_ws(
    stream: TcpStream,
    peer_addr: std::net::SocketAddr,
    conn_id: u64,
    store: Arc<dyn GraphStore>,
    bookmark: Arc<BookmarkCounter>,
    write_semaphore: Arc<tokio::sync::Semaphore>,
    idle_timeout: Duration,
    max_connection_lifetime: Option<Duration>,
) -> Result<(), SessionError> {
    let is_ws = sedandelivery_bolt::websocket::is_websocket_upgrade(&stream)
        .await
        .unwrap_or(false);

    if is_ws {
        tracing::info!(conn_id, "WebSocket upgrade detected");
        match sedandelivery_bolt::websocket::accept(stream).await {
            Ok(ws_stream) => {
                let adapter = sedandelivery_bolt::websocket::WsStreamAdapter::new(ws_stream);
                tracing::info!(conn_id, "WebSocket connection established");
                session::run(
                    adapter,
                    make_session_config(
                        store,
                        conn_id,
                        Some(peer_addr.ip()),
                        bookmark,
                        write_semaphore,
                        idle_timeout,
                        max_connection_lifetime,
                    ),
                )
                .await
            }
            Err(e) => {
                tracing::warn!(%peer_addr, conn_id, "WebSocket upgrade failed: {e}");
                Ok(())
            }
        }
    } else {
        session::run(
            stream,
            make_session_config(
                store,
                conn_id,
                Some(peer_addr.ip()),
                bookmark,
                write_semaphore,
                idle_timeout,
                max_connection_lifetime,
            ),
        )
        .await
    }
}

fn make_session_config(
    store: Arc<dyn GraphStore>,
    conn_id: u64,
    peer_ip: Option<IpAddr>,
    global_bookmark: Arc<BookmarkCounter>,
    write_semaphore: Arc<tokio::sync::Semaphore>,
    idle_timeout: Duration,
    max_connection_lifetime: Option<Duration>,
) -> SessionConfig {
    SessionConfig {
        store,
        conn_id,
        peer_ip,
        global_bookmark,
        write_semaphore,
        idle_timeout,
        max_connection_lifetime,
    }
}

fn log_result(peer_addr: std::net::SocketAddr, conn_id: u64, result: Result<(), SessionError>) {
    match result {
        Ok(()) => {
            tracing::info!(%peer_addr, conn_id, "connection closed normally");
        }
        Err(e) => match &e {
            SessionError::IdleTimeout(_) | SessionError::MaxLifetimeExceeded(_) => {
                tracing::info!(%peer_addr, conn_id, "{e}");
            }
            _ => {
                tracing::warn!(%peer_addr, conn_id, "session error: {e}");
            }
        },
    }
}
