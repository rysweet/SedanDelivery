//! Prometheus metrics for gql-api.
//!
//! Uses the `metrics` crate's global API (same pattern as sedandelivery-bolt).
//! The PrometheusBuilder installs a global recorder at startup; the `/metrics`
//! endpoint reads from its handle.

use std::time::Instant;

use axum::extract::{MatchedPath, Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use metrics::{counter, gauge, histogram};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};

use crate::state::AppState;

/// Metric name constants.
pub const HTTP_REQUESTS_TOTAL: &str = "http_requests_total";
pub const HTTP_REQUEST_DURATION_SECONDS: &str = "http_request_duration_seconds";
pub const AUTH_REJECTIONS_TOTAL: &str = "auth_rejections_total";
pub const ACTIVE_GRAPHS: &str = "active_graphs";
pub const JWKS_REFRESH_FAILURES_TOTAL: &str = "jwks_refresh_failures_total";

/// Install the Prometheus recorder and return a handle for scraping.
///
/// Must be called once at startup before any metrics are recorded.
pub fn init_recorder() -> PrometheusHandle {
    PrometheusBuilder::new()
        .install_recorder()
        .expect("failed to install Prometheus recorder")
}

/// Record an authentication rejection.
pub fn auth_rejection() {
    counter!(AUTH_REJECTIONS_TOTAL).increment(1);
}

/// Record a JWKS refresh failure.
pub fn jwks_refresh_failure() {
    counter!(JWKS_REFRESH_FAILURES_TOTAL).increment(1);
}

/// Set the active_graphs gauge to the current number of graphs.
pub fn set_active_graphs(count: usize) {
    gauge!(ACTIVE_GRAPHS).set(count as f64);
}

/// Axum middleware that records `http_requests_total` and
/// `http_request_duration_seconds` for every request.
pub async fn track_http_metrics(request: Request, next: Next) -> Response {
    let method = request.method().to_string();
    let path = request
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| request.uri().path().to_string());

    let start = Instant::now();
    let response = next.run(request).await;
    let duration = start.elapsed().as_secs_f64();

    let status = response.status().as_u16().to_string();

    counter!(
        HTTP_REQUESTS_TOTAL,
        "method" => method.clone(),
        "path" => path.clone(),
        "status" => status
    )
    .increment(1);

    histogram!(
        HTTP_REQUEST_DURATION_SECONDS,
        "method" => method,
        "path" => path
    )
    .record(duration);

    response
}

/// Router that exposes `GET /metrics` for Prometheus scraping.
pub fn router(handle: PrometheusHandle) -> Router<AppState> {
    Router::new().route("/metrics", get(metrics_handler).with_state(handle))
}

async fn metrics_handler(State(handle): State<PrometheusHandle>) -> impl IntoResponse {
    let body = handle.render();
    Response::builder()
        .header("content-type", "text/plain; version=0.0.4; charset=utf-8")
        .body(axum::body::Body::from(body))
        .unwrap_or_else(|_| {
            // If even the fallback Response::builder fails, return a minimal
            // hardcoded response. No panics in error paths.
            let mut resp = Response::new(axum::body::Body::from(
                "internal error building metrics response",
            ));
            *resp.status_mut() = axum::http::StatusCode::INTERNAL_SERVER_ERROR;
            resp
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_rejection_no_panic() {
        auth_rejection();
        auth_rejection();
        auth_rejection();
    }

    #[test]
    fn test_jwks_refresh_failure_no_panic() {
        jwks_refresh_failure();
    }

    #[test]
    fn test_set_active_graphs_no_panic() {
        set_active_graphs(10);
    }

    #[test]
    fn test_metric_names_valid() {
        for name in [
            HTTP_REQUESTS_TOTAL,
            HTTP_REQUEST_DURATION_SECONDS,
            AUTH_REJECTIONS_TOTAL,
            ACTIVE_GRAPHS,
            JWKS_REFRESH_FAILURES_TOTAL,
        ] {
            assert!(!name.is_empty(), "metric name must not be empty");
            assert!(
                name.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "metric name '{name}' contains invalid characters (only [a-z_] allowed)"
            );
        }
    }
}
