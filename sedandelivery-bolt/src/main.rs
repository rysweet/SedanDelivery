use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use socket2::SockRef;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;

use gql_core::storage::GraphStore;

/// Create the graph store based on environment configuration.
///
/// Delegates to [`gql_redis_store::create_store::create_store`].
fn create_store() -> Arc<dyn GraphStore> {
    gql_redis_store::create_store::create_store()
}

/// RAII guard that decrements the active-connection gauge on drop.
/// This ensures the counter is accurate even when a session task panics.
struct ConnectionGuard {
    active: Arc<AtomicU64>,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::Relaxed);
        // Record disconnect metrics on drop (covers both normal close and panic).
        sedandelivery_bolt::metrics::connection_closed();
    }
}

#[tokio::main]
async fn main() {
    // Structured JSON logging when BOLT_JSON_LOGS=1, otherwise human-readable.
    // When OTEL_EXPORTER_OTLP_ENDPOINT is set, traces are also exported via OTLP.
    let json_logs = std::env::var("BOLT_JSON_LOGS")
        .map(|v| v == "1")
        .unwrap_or(false);

    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    if std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").is_ok() {
        use tracing_subscriber::prelude::*;

        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_tonic()
            .build()
            .expect("failed to create OTLP span exporter");
        let provider = opentelemetry_sdk::trace::TracerProvider::builder()
            .with_batch_exporter(exporter, opentelemetry_sdk::runtime::Tokio)
            .build();
        let tracer = opentelemetry::trace::TracerProvider::tracer(&provider, "sedandelivery-bolt");

        if json_logs {
            let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);
            tracing_subscriber::registry()
                .with(env_filter)
                .with(tracing_subscriber::fmt::layer().json())
                .with(otel_layer)
                .init();
        } else {
            let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);
            tracing_subscriber::registry()
                .with(env_filter)
                .with(tracing_subscriber::fmt::layer())
                .with(otel_layer)
                .init();
        }
        tracing::info!("OpenTelemetry OTLP tracing enabled");
    } else if json_logs {
        tracing_subscriber::fmt()
            .json()
            .with_env_filter(env_filter)
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
    }

    // In production mode, require security-sensitive variables.
    if std::env::var("ENVIRONMENT").as_deref() == Ok("production") {
        if std::env::var("BOLT_PASSWORD").is_err() {
            panic!("BOLT_PASSWORD is required when ENVIRONMENT=production");
        }
        if std::env::var("BOLT_TLS_CERT").is_err() || std::env::var("BOLT_TLS_KEY").is_err() {
            panic!("BOLT_TLS_CERT and BOLT_TLS_KEY are required when ENVIRONMENT=production");
        }
    }

    let bind_addr = std::env::var("BOLT_BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:7687".to_string());

    // --- Connection lifecycle configuration ---
    //
    // Short idle timeouts and bounded connection lifetimes are critical for
    // L4 (TCP-level) load balancing such as Envoy in Container Apps.  L4
    // balancers assign a connection to a backend at connect-time and never
    // move it.  Long-lived connections therefore "pin" to the original
    // replica, starving new replicas after a scale-out event.  By capping
    // idle time (300 s) and absolute lifetime (1800 s) we force clients to
    // reconnect periodically, giving the load balancer a chance to spread
    // traffic across all healthy replicas.

    let max_connections: usize =
        gql_core::config::parse_env_range("BOLT_MAX_CONNECTIONS", 1024, 1, 65535);

    let idle_timeout_secs: u64 =
        gql_core::config::parse_env_range("BOLT_IDLE_TIMEOUT_SECS", 300, 1, 86400);
    let idle_timeout = Duration::from_secs(idle_timeout_secs);

    let max_connection_lifetime_secs: u64 =
        gql_core::config::parse_env_range("BOLT_MAX_CONNECTION_LIFETIME_SECS", 1800, 0, 86400);
    let max_connection_lifetime = if max_connection_lifetime_secs == 0 {
        None
    } else {
        Some(Duration::from_secs(max_connection_lifetime_secs))
    };

    let write_concurrency: u32 =
        gql_core::config::parse_env_range("BOLT_WRITE_CONCURRENCY", 128, 1, 10000);

    // Pre-handshake timeout: max time to complete TLS/WebSocket handshake
    // before the slot is reclaimed. Prevents slot exhaustion from slow clients.
    let handshake_timeout_ms: u64 = std::env::var("BOLT_HANDSHAKE_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5_000); // 5 second default
    let handshake_timeout = Duration::from_millis(handshake_timeout_ms);

    // Per-IP connection limit (economic DoS protection).
    let max_conns_per_ip: usize = std::env::var("BOLT_MAX_CONN_PER_IP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50);

    let per_ip_conns: Arc<DashMap<IpAddr, usize>> = Arc::new(DashMap::new());

    // --- TCP keepalive configuration ---
    //
    // Azure Load Balancer has a 4-minute (240s) TCP idle timeout. Connections
    // that sit idle longer than this are silently dropped, causing clients to
    // see unexpected `ConnectionResetError` with no server-side indication.
    //
    // To prevent this, we enable TCP keepalive on every accepted connection
    // with a probe interval well under Azure's 240s threshold (default 120s).
    // The kernel sends probes at TCP_KEEPINTVL (30s) intervals up to
    // TCP_KEEPCNT (3) times before declaring the connection dead.
    let keepalive_secs: u64 =
        gql_core::config::parse_env_range("BOLT_TCP_KEEPALIVE_SECS", 120, 10, 300);
    let tcp_keepalive = socket2::TcpKeepalive::new()
        .with_time(Duration::from_secs(keepalive_secs))
        .with_interval(Duration::from_secs(30))
        .with_retries(3);
    tracing::info!(keepalive_secs, "TCP keepalive enabled (Azure LB idle timeout mitigation)");

    // Semaphore gates new connections once max_connections is reached.
    let semaphore = Arc::new(Semaphore::new(max_connections));

    // Active connection gauge (increment on connect, decrement on disconnect).
    let active_connections = Arc::new(AtomicU64::new(0));

    let store = create_store();
    // Pre-create the default "neo4j" database so it exists before any client
    // connects. With Redis-backed stores the graph may already exist from a
    // previous run (data is persistent), so check first to be idempotent.
    if !store
        .graph_exists("neo4j")
        .expect("failed to check default database")
    {
        // Handle race: two replicas may both see graph_exists==false and race to create.
        match store.create_graph("neo4j") {
            Ok(()) => {}
            Err(gql_core::error::StorageError::GraphAlreadyExists { .. }) => {
                tracing::info!("default 'neo4j' graph already created by peer replica");
            }
            Err(e) => panic!("failed to create default 'neo4j' database: {e}"),
        }
    }

    // Load TLS configuration (returns None if BOLT_TLS_CERT / BOLT_TLS_KEY are not set)
    let tls_acceptor = sedandelivery_bolt::tls::load_tls_config();
    let required_password = std::env::var("BOLT_PASSWORD").ok();
    if tls_acceptor.is_some() {
        tracing::info!("TLS enabled for Bolt connections");
    } else {
        tracing::info!("TLS not configured, using plaintext");
        if required_password.is_some() {
            tracing::warn!(
                "BOLT_PASSWORD is set but TLS is not configured \
                 — credentials will be transmitted in plaintext"
            );
        }
    }

    // If BOLT_REQUIRE_TLS is set to "1" or "true", refuse to start without TLS.
    let require_tls = std::env::var("BOLT_REQUIRE_TLS")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if require_tls && tls_acceptor.is_none() {
        panic!(
            "BOLT_REQUIRE_TLS is set but TLS is not configured \
             (set BOLT_TLS_CERT and BOLT_TLS_KEY)"
        );
    }

    // Spawn periodic auth rate limiter sweep (removes stale per-conn entries).
    sedandelivery_bolt::protocol::handler::spawn_auth_rate_limiter_sweep();

    // Initialize Prometheus metrics recorder.
    let prometheus_handle = sedandelivery_bolt::metrics::init_recorder();

    let start_time = Instant::now();

    // Spawn the health/metrics HTTP server.
    sedandelivery_bolt::health::spawn_health_server(sedandelivery_bolt::health::HealthState {
        prometheus_handle,
        active_connections: Arc::clone(&active_connections),
        start_time,
        store: Arc::clone(&store),
    });

    // Spawn background task that periodically sweeps stale per-IP auth rate-limit
    // entries and enforces the 100K entry cap to prevent unbounded memory growth.
    sedandelivery_bolt::protocol::handler::spawn_auth_rate_limit_sweep();

    let listener = TcpListener::bind(&bind_addr)
        .await
        .expect("failed to bind Bolt TCP listener");
    tracing::info!(
        "sedandelivery-bolt listening on {} (max_connections={}, idle_timeout={}s, max_connection_lifetime={}s, write_concurrency={}, handshake_timeout_ms={})",
        bind_addr,
        max_connections,
        idle_timeout_secs,
        max_connection_lifetime_secs,
        write_concurrency,
        handshake_timeout_ms
    );

    // Monotonic connection ID (not the same as active count).
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
    let write_semaphore = Arc::new(tokio::sync::Semaphore::new(write_concurrency as usize));

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

        // Set TCP keepalive on the accepted socket to prevent Azure Load
        // Balancer from silently dropping idle connections.
        if let Err(e) = SockRef::from(&stream).set_tcp_keepalive(&tcp_keepalive) {
            tracing::warn!(%peer_addr, "failed to set TCP keepalive: {e}");
        }

        // Per-IP connection limit check.
        let per_ip_guard = {
            let ip = peer_addr.ip();
            match sedandelivery_bolt::connection::per_ip_guard::PerIpGuard::try_new(
                Arc::clone(&per_ip_conns),
                ip,
                max_conns_per_ip,
            ) {
                Some(guard) => guard,
                None => {
                    tracing::warn!(
                        %peer_addr,
                        max_conns_per_ip,
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
                // Semaphore was closed — only happens during shutdown.
                tracing::info!("semaphore closed, rejecting connection");
                break;
            }
        };

        let store = Arc::clone(&store);
        let conn_id = connection_counter.fetch_add(1, Ordering::Relaxed);
        let bookmark = Arc::clone(&global_bookmark);
        let wl = Arc::clone(&write_semaphore);
        let active = Arc::clone(&active_connections);
        let tls = tls_acceptor.clone();

        // Record connection metrics.
        sedandelivery_bolt::metrics::connection_accepted();

        active.fetch_add(1, Ordering::Relaxed);
        let current = active.load(Ordering::Relaxed);
        tracing::info!(%peer_addr, conn_id, active_connections = current, "new connection");

        let handle = tokio::spawn(async move {
            // _permit is held for the lifetime of this task; when dropped it
            // returns the permit to the semaphore, allowing a new connection.
            let _permit = permit;
            // _per_ip_guard is held for the lifetime of this task; when dropped
            // it decrements the per-IP connection count.
            let _per_ip_guard = per_ip_guard;

            // ConnectionGuard ensures active count is decremented even on panic.
            // It also records disconnect metrics via connection_closed().
            let _guard = ConnectionGuard { active };

            // Idle timeout and max connection lifetime are now enforced
            // inside session::run per-message, so we no longer wrap the
            // entire session in a single tokio::time::timeout.
            let result = if let Some(acceptor) = tls {
                match tokio::time::timeout(handshake_timeout, acceptor.accept(stream)).await {
                    Ok(tls_result) => match tls_result {
                        Ok(tls_stream) => {
                            tracing::debug!(conn_id, "TLS handshake complete");
                            sedandelivery_bolt::connection::session::run(
                                tls_stream,
                                store,
                                conn_id,
                                bookmark,
                                wl,
                                Some(peer_addr.ip()),
                                idle_timeout,
                                max_connection_lifetime,
                            )
                            .await
                        }
                        Err(e) => {
                            tracing::warn!(%peer_addr, conn_id, "TLS handshake failed: {e}");
                            Ok(())
                        }
                    },
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
            } else {
                // Detect WebSocket upgrade vs raw Bolt TCP.
                let is_ws = sedandelivery_bolt::websocket::is_websocket_upgrade(&stream)
                    .await
                    .unwrap_or(false);

                if is_ws {
                    tracing::info!(conn_id, "WebSocket upgrade detected");
                    match sedandelivery_bolt::websocket::accept(stream).await {
                        Ok(ws_stream) => {
                            let adapter =
                                sedandelivery_bolt::websocket::WsStreamAdapter::new(ws_stream);
                            tracing::info!(conn_id, "WebSocket connection established");
                            sedandelivery_bolt::connection::session::run(
                                adapter,
                                store,
                                conn_id,
                                bookmark,
                                wl,
                                Some(peer_addr.ip()),
                                idle_timeout,
                                max_connection_lifetime,
                            )
                            .await
                        }
                        Err(e) => {
                            tracing::warn!(
                                %peer_addr, conn_id,
                                "WebSocket upgrade failed: {e}"
                            );
                            Ok(())
                        }
                    }
                } else {
                    sedandelivery_bolt::connection::session::run(
                        stream,
                        store,
                        conn_id,
                        bookmark,
                        wl,
                        Some(peer_addr.ip()),
                        idle_timeout,
                        max_connection_lifetime,
                    )
                    .await
                }
            };

            match result {
                Ok(()) => {
                    tracing::info!(%peer_addr, conn_id, "connection closed normally");
                }
                Err(e) => {
                    // Idle timeout and max lifetime are expected closures, not warnings.
                    use sedandelivery_bolt::connection::session::SessionError;
                    match &e {
                        SessionError::IdleTimeout(_) | SessionError::MaxLifetimeExceeded(_) => {
                            tracing::info!(%peer_addr, conn_id, "{e}");
                        }
                        _ => {
                            tracing::warn!(%peer_addr, conn_id, "session error: {e}");
                        }
                    }
                }
            }

            // _guard and _permit drop here, decrementing active count,
            // recording disconnect metrics, and releasing the semaphore permit.
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

    let drain_timeout_secs: u64 = std::env::var("BOLT_DRAIN_TIMEOUT_SECS")
        .ok()
        .and_then(|v| match v.parse::<u64>() {
            Ok(val) if val > 0 => Some(val),
            _ => {
                tracing::warn!("invalid BOLT_DRAIN_TIMEOUT_SECS value, using default 30");
                None
            }
        })
        .unwrap_or(30);
    let drain_timeout = Duration::from_secs(drain_timeout_secs);
    tracing::info!(
        active_connections = active_connections.load(Ordering::Relaxed),
        drain_timeout_secs,
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
