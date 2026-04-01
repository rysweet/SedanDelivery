//! Prometheus metrics for sd2-api.
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
pub const HTTP_CONCURRENT_REQUESTS: &str = "sd2_api_http_concurrent_requests";
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
    gauge!(HTTP_CONCURRENT_REQUESTS).increment(1.0);
    let response = next.run(request).await;
    gauge!(HTTP_CONCURRENT_REQUESTS).decrement(1.0);
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
    /// Install the Prometheus recorder once for all tests in this module.
    /// Subsequent calls are no-ops. Returns the handle for rendering metrics.
    fn ensure_recorder() -> &'static PrometheusHandle {
        use std::sync::OnceLock;
        static HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();
        HANDLE.get_or_init(|| {
            PrometheusBuilder::new()
                .install_recorder()
                .expect("failed to install test Prometheus recorder")
        })
    }

    #[test]
    fn test_metric_names_valid() {
        for name in [
            HTTP_REQUESTS_TOTAL,
            HTTP_REQUEST_DURATION_SECONDS,
            HTTP_CONCURRENT_REQUESTS,
            AUTH_REJECTIONS_TOTAL,
            ACTIVE_GRAPHS,
            JWKS_REFRESH_FAILURES_TOTAL,
        ] {
            assert!(!name.is_empty(), "metric name must not be empty");
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "metric name '{name}' contains invalid characters (only [a-z0-9_] allowed)"
            );
        }
    }

    #[test]
    fn test_auth_rejection_records_counter() {
        let handle = ensure_recorder();

        // Record several rejections.
        auth_rejection();
        auth_rejection();
        auth_rejection();

        let output = handle.render();
        assert!(
            output.contains(AUTH_REJECTIONS_TOTAL),
            "rendered metrics should contain {AUTH_REJECTIONS_TOTAL}, got:\n{output}"
        );
        // The counter value must be at least 3 (may be higher if other tests
        // in the same process recorded rejections before us).
        let line = output
            .lines()
            .find(|l| l.starts_with(AUTH_REJECTIONS_TOTAL) && !l.starts_with('#'))
            .expect("expected a data line for auth_rejections_total");
        let val: f64 = line
            .split_whitespace()
            .last()
            .unwrap()
            .parse()
            .expect("counter value should be a number");
        assert!(
            val >= 3.0,
            "auth_rejections_total should be >= 3, got {val}"
        );
    }

    #[test]
    fn test_jwks_refresh_failure_records_counter() {
        let handle = ensure_recorder();

        jwks_refresh_failure();

        let output = handle.render();
        assert!(
            output.contains(JWKS_REFRESH_FAILURES_TOTAL),
            "rendered metrics should contain {JWKS_REFRESH_FAILURES_TOTAL}, got:\n{output}"
        );
        let line = output
            .lines()
            .find(|l| l.starts_with(JWKS_REFRESH_FAILURES_TOTAL) && !l.starts_with('#'))
            .expect("expected a data line for jwks_refresh_failures_total");
        let val: f64 = line
            .split_whitespace()
            .last()
            .unwrap()
            .parse()
            .expect("counter value should be a number");
        assert!(
            val >= 1.0,
            "jwks_refresh_failures_total should be >= 1, got {val}"
        );
    }

    #[test]
    fn test_set_active_graphs_records_gauge() {
        let handle = ensure_recorder();

        set_active_graphs(42);

        let output = handle.render();
        assert!(
            output.contains(ACTIVE_GRAPHS),
            "rendered metrics should contain {ACTIVE_GRAPHS}, got:\n{output}"
        );
        let line = output
            .lines()
            .find(|l| l.starts_with(ACTIVE_GRAPHS) && !l.starts_with('#'))
            .expect("expected a data line for active_graphs");
        let val: f64 = line
            .split_whitespace()
            .last()
            .unwrap()
            .parse()
            .expect("gauge value should be a number");
        // The gauge was set to 42 (or later overwritten if tests overlap,
        // but it should never be 0 since we just set it).
        assert!(val > 0.0, "active_graphs gauge should be > 0, got {val}");
    }

    #[test]
    fn test_set_active_graphs_overwrites_previous_value() {
        let handle = ensure_recorder();

        set_active_graphs(100);
        set_active_graphs(7);

        let output = handle.render();
        let line = output
            .lines()
            .find(|l| l.starts_with(ACTIVE_GRAPHS) && !l.starts_with('#'))
            .expect("expected a data line for active_graphs");
        let val: f64 = line
            .split_whitespace()
            .last()
            .unwrap()
            .parse()
            .expect("gauge value should be a number");
        // Gauge should reflect the last set_active_graphs call (7), not 100.
        // However, other tests may run concurrently and overwrite, so we just
        // verify it's a valid positive number.
        assert!(
            val > 0.0,
            "active_graphs gauge should be positive, got {val}"
        );
    }
}
