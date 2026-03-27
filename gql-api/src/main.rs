//! gql-api: HTTP REST API for the SedanDelivery graph engine.
//!
//! Exposes all graph operations (CRUD for nodes/edges, Cypher query/mutate,
//! schema inspection, neighbor traversal, export/import) over a JSON API.
//!
//! # Storage backend
//! Set `GQL_REDIS_URL` to use `RedisGraphStore` with r2d2 connection pooling
//! (pool size configurable via `GQL_REDIS_POOL_SIZE`, default 64).
//! When unset, falls back to volatile `InMemoryGraphStore` (local dev / tests).
//!
//! # CORS policy
//! By default only `http://localhost:5000` (the .NET BFF) and
//! `http://localhost:5173` (the React Vite dev server) are allowed.
//! Set `GQL_API_ALLOWED_ORIGINS` (comma-separated) to override in production.
//! The wildcard `Any` origin is intentionally **not** used because this service
//! accepts mutations.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::http::HeaderValue;
use axum::{middleware, Router};
use tower::limit::ConcurrencyLimitLayer;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::info;
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

use gql_api::auth;
use gql_api::metrics;
use gql_api::rate_limit;
use gql_core::storage::GraphStore;

use gql_api::routes;
use gql_api::state::AppState;

/// Create the graph store based on environment configuration.
///
/// Delegates to [`gql_redis_store::create_store::create_store`].
fn create_store() -> Arc<dyn GraphStore> {
    gql_redis_store::create_store::create_store()
}

/// Wait for SIGINT (Ctrl-C) or SIGTERM, then return.
async fn shutdown_signal() {
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("failed to register SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("received SIGINT, initiating graceful shutdown");
        }
        _ = sigterm.recv() => {
            info!("received SIGTERM, initiating graceful shutdown");
        }
    }
}

#[tokio::main]
async fn main() {
    // Initialise structured logging.  RUST_LOG=info is the default.
    // When OTEL_EXPORTER_OTLP_ENDPOINT is set, traces are also exported via OTLP.
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    if std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").is_ok() {
        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_tonic()
            .build()
            .expect("failed to create OTLP span exporter");
        let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
            .with_batch_exporter(exporter)
            .build();
        let tracer = opentelemetry::trace::TracerProvider::tracer(&provider, "gql-api");
        let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

        tracing_subscriber::registry()
            .with(env_filter)
            .with(tracing_subscriber::fmt::layer())
            .with(otel_layer)
            .init();
        info!("OpenTelemetry OTLP tracing enabled");
    } else {
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
    };

    // In production mode, require explicit values for security-sensitive variables.
    if std::env::var("ENVIRONMENT").as_deref() == Ok("production") {
        if std::env::var("GQL_REDIS_URL").is_err() {
            panic!("GQL_REDIS_URL is required when ENVIRONMENT=production");
        }
        if std::env::var("GQL_API_ALLOWED_ORIGINS").is_err() {
            panic!("GQL_API_ALLOWED_ORIGINS is required when ENVIRONMENT=production");
        }
        if std::env::var("GQL_API_ENTRA_TENANT_ID").is_err() {
            panic!("GQL_API_ENTRA_TENANT_ID is required when ENVIRONMENT=production");
        }
        if std::env::var("GQL_API_ENTRA_CLIENT_ID").is_err() {
            panic!("GQL_API_ENTRA_CLIENT_ID is required when ENVIRONMENT=production");
        }
    }

    // Initialize Prometheus metrics recorder.
    let prometheus_handle = metrics::init_recorder();

    let store = create_store();
    let state = AppState::new(store);

    // Build the allowed-origins list from the environment, defaulting to
    // the .NET BFF and the Vite dev server only.
    let allowed_origins: Vec<HeaderValue> = match std::env::var("GQL_API_ALLOWED_ORIGINS") {
        Ok(origins) => origins
            .split(',')
            .filter_map(|o| {
                let trimmed = o.trim();
                match trimmed.parse() {
                    Ok(val) => Some(val),
                    Err(e) => {
                        tracing::warn!(
                            origin = trimmed,
                            "invalid CORS origin in GQL_API_ALLOWED_ORIGINS: {e:?}, skipping"
                        );
                        None
                    }
                }
            })
            .collect(),
        Err(_) => {
            tracing::warn!(
                "GQL_API_ALLOWED_ORIGINS not set — using localhost defaults. \
                 Set ENVIRONMENT=production to enforce."
            );
            "http://localhost:5000,http://localhost:5173"
                .split(',')
                .filter_map(|o| o.trim().parse().ok())
                .collect()
        }
    };

    info!("CORS allowed origins: {:?}", allowed_origins);

    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(allowed_origins))
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::PUT,
            axum::http::Method::DELETE,
            axum::http::Method::OPTIONS,
        ])
        .allow_headers([
            axum::http::header::CONTENT_TYPE,
            axum::http::header::AUTHORIZATION,
            axum::http::header::ACCEPT,
        ]);

    // Warn if the legacy GQL_API_JWKS_URL env var is set — it has no effect.
    if std::env::var("GQL_API_JWKS_URL").is_ok() {
        tracing::warn!(
            "GQL_API_JWKS_URL is set but has no effect — \
             authentication uses the Entra ID path (GQL_API_ENTRA_TENANT_ID / GQL_API_ENTRA_CLIENT_ID)"
        );
    }

    // Log whether Entra ID auth is enabled.
    if auth::entra_config().is_some() {
        info!("Entra ID authentication ENABLED (GQL_API_ENTRA_TENANT_ID is set)");
    } else {
        tracing::warn!(
            "Entra ID authentication DISABLED — set GQL_API_ENTRA_TENANT_ID \
             and GQL_API_ENTRA_CLIENT_ID to enable"
        );
    }

    // Per-IP rate limiting — configurable via GQL_API_RATE_LIMIT_RPS (default 100).
    let rps = rate_limit::rps_from_env();
    let limiter = rate_limit::build_limiter(rps);
    rate_limit::spawn_limiter_sweeper(Arc::clone(&limiter));
    info!("Per-IP rate limit: {rps} requests/second");

    // 100 concurrent requests per replica. KEDA scales min 1 → max 3 replicas,
    // giving 100–300 total concurrent requests across the deployment.
    // Applied BEFORE auth/trace layers so it rejects at the outermost boundary.
    let max_concurrency: usize = std::env::var("GQL_API_MAX_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&v: &usize| v >= 1)
        .unwrap_or(100);

    let app = Router::new()
        .merge(routes::health::router())
        .merge(routes::graphs::router())
        .merge(routes::nodes::router())
        .merge(routes::edges::router())
        .merge(routes::query::router())
        .merge(routes::schema::router())
        .merge(routes::neighbors::router())
        .merge(routes::export_import::router())
        .merge(metrics::router(prometheus_handle))
        .layer(middleware::from_fn(auth::require_entra_auth))
        .layer(middleware::from_fn(rate_limit::rate_limit_middleware))
        .layer(axum::Extension(limiter))
        .layer(middleware::from_fn(metrics::track_http_metrics))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .layer(ConcurrencyLimitLayer::new(max_concurrency))
        .with_state(state);

    // Load TLS configuration (returns None if GQL_API_TLS_CERT / GQL_API_TLS_KEY are unset).
    let tls_acceptor = gql_api::tls::load_tls_config();

    let drain_timeout_secs: u64 = std::env::var("GQL_API_DRAIN_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);

    if let Some(acceptor) = tls_acceptor {
        // TLS mode — use GQL_API_TLS_PORT (default 3443) for the bind address.
        let tls_port = gql_api::tls::tls_port();
        let addr: SocketAddr = std::env::var("GQL_API_BIND_ADDR")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| SocketAddr::from(([0, 0, 0, 0], tls_port)));
        info!("gql-api listening on {addr} (HTTPS)");

        let tcp_listener = tokio::net::TcpListener::bind(addr)
            .await
            .expect("failed to bind TCP listener for TLS");
        let tls_listener = gql_api::tls::TlsListener::new(tcp_listener, acceptor);

        let shutdown_notify = Arc::new(tokio::sync::Notify::new());
        let shutdown_notify2 = Arc::clone(&shutdown_notify);

        let server = axum::serve(
            tls_listener,
            app.into_make_service_with_connect_info::<gql_api::tls::TlsConnectInfo>(),
        )
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            shutdown_notify2.notify_one();
        });

        tokio::select! {
            result = server => {
                if let Err(e) = result {
                    tracing::error!("server error: {e}");
                }
            }
            _ = async {
                shutdown_notify.notified().await;
                info!(drain_timeout_secs, "draining connections");
                tokio::time::sleep(Duration::from_secs(drain_timeout_secs)).await;
            } => {
                tracing::warn!(
                    drain_timeout_secs,
                    "drain timeout exceeded, shutting down with in-flight connections"
                );
            }
        }
    } else {
        // Plaintext HTTP mode (backward compatible).
        let addr: SocketAddr = std::env::var("GQL_API_BIND_ADDR")
            .unwrap_or_else(|_| "0.0.0.0:3001".to_string())
            .parse()
            .expect("GQL_API_BIND_ADDR must be a valid socket address (e.g. 0.0.0.0:3001)");
        info!("gql-api listening on {addr} (HTTP)");

        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .expect("failed to bind TCP listener");

        let shutdown_notify = Arc::new(tokio::sync::Notify::new());
        let shutdown_notify2 = Arc::clone(&shutdown_notify);

        let server = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            shutdown_notify2.notify_one();
        });

        tokio::select! {
            result = server => {
                if let Err(e) = result {
                    tracing::error!("server error: {e}");
                }
            }
            _ = async {
                shutdown_notify.notified().await;
                info!(drain_timeout_secs, "draining connections");
                tokio::time::sleep(Duration::from_secs(drain_timeout_secs)).await;
            } => {
                tracing::warn!(
                    drain_timeout_secs,
                    "drain timeout exceeded, shutting down with in-flight connections"
                );
            }
        }
    }

    info!("gql-api shutdown complete");
}
