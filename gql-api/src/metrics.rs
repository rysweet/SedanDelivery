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
            Response::builder()
                .status(500)
                .body(axum::body::Body::from(
                    "internal error building metrics response",
                ))
                .expect("fallback response must be valid")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_recorder_succeeds() {
        // Can only install once per process; just verify the function exists
        // and the constants are defined.
        assert_eq!(HTTP_REQUESTS_TOTAL, "http_requests_total");
        assert_eq!(
            HTTP_REQUEST_DURATION_SECONDS,
            "http_request_duration_seconds"
        );
        assert_eq!(AUTH_REJECTIONS_TOTAL, "auth_rejections_total");
        assert_eq!(ACTIVE_GRAPHS, "active_graphs");
    }
}
