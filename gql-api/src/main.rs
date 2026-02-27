//! gql-api: HTTP REST API for the SedanDelivery graph engine.
//!
//! Exposes all graph operations (CRUD for nodes/edges, Cypher query/mutate,
//! schema inspection, neighbor traversal, export/import) over a JSON API.
//! All graph data is held in `InMemoryGraphStore` instances — one per named graph.
//!
//! # CORS policy
//! By default only `http://localhost:5000` (the .NET BFF) and
//! `http://localhost:5173` (the React Vite dev server) are allowed.
//! Set `GQL_API_ALLOWED_ORIGINS` (comma-separated) to override in production.
//! The wildcard `Any` origin is intentionally **not** used because this service
//! accepts mutations.

mod error;
mod models;
mod routes;
mod serialize;
mod state;

use std::net::SocketAddr;

use axum::Router;
use axum::http::HeaderValue;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::state::AppState;

#[tokio::main]
async fn main() {
    // Initialise structured logging.  RUST_LOG=info is the default.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    // In production mode, require explicit values for security-sensitive variables.
    if std::env::var("ENVIRONMENT").as_deref() == Ok("production") {
        if std::env::var("GQL_REDIS_URL").is_err() {
            panic!("GQL_REDIS_URL is required when ENVIRONMENT=production");
        }
        if std::env::var("GQL_API_ALLOWED_ORIGINS").is_err() {
            panic!("GQL_API_ALLOWED_ORIGINS is required when ENVIRONMENT=production");
        }
    }

    let state = AppState::new();

    // Build the allowed-origins list from the environment, defaulting to
    // the .NET BFF and the Vite dev server only.
    let allowed_origins: Vec<HeaderValue> = std::env::var("GQL_API_ALLOWED_ORIGINS")
        .unwrap_or_else(|_| "http://localhost:5000,http://localhost:5173".to_string())
        .split(',')
        .filter_map(|o| o.trim().parse().ok())
        .collect();

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

    let app = Router::new()
        .merge(routes::health::router())
        .merge(routes::graphs::router())
        .merge(routes::nodes::router())
        .merge(routes::edges::router())
        .merge(routes::query::router())
        .merge(routes::schema::router())
        .merge(routes::neighbors::router())
        .merge(routes::export_import::router())
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    // Bind address: read from GQL_API_BIND_ADDR, default 0.0.0.0:3001.
    let addr: SocketAddr = std::env::var("GQL_API_BIND_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:3001".to_string())
        .parse()
        .expect("GQL_API_BIND_ADDR must be a valid socket address (e.g. 0.0.0.0:3001)");
    info!("gql-api listening on {addr}");

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("failed to bind TCP listener");

    axum::serve(listener, app)
        .await
        .expect("server error");
}
