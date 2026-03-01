//! Prometheus metrics for sedan-bolt.
//!
//! Uses the `metrics` crate's global API so any code can record metrics
//! without passing handles around. The PrometheusBuilder installs a global
//! recorder at startup; the health endpoint reads from its handle.

use metrics::{counter, gauge, histogram};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};

/// Metric name constants.
pub const CONNECTIONS_ACTIVE: &str = "sedan_bolt_connections_active";
pub const CONNECTIONS_TOTAL: &str = "sedan_bolt_connections_total";
pub const QUERIES_TOTAL: &str = "sedan_bolt_queries_total";
pub const QUERY_DURATION_SECONDS: &str = "sedan_bolt_query_duration_seconds";
pub const ERRORS_TOTAL: &str = "sedan_bolt_errors_total";

/// Install the Prometheus recorder and return a handle for scraping.
///
/// Must be called once at startup before any metrics are recorded.
pub fn init_recorder() -> PrometheusHandle {
    let builder = PrometheusBuilder::new();
    builder
        .install_recorder()
        .expect("failed to install Prometheus recorder")
}

/// Record a new connection (increment active gauge and total counter).
pub fn connection_accepted() {
    gauge!(CONNECTIONS_ACTIVE).increment(1.0);
    counter!(CONNECTIONS_TOTAL).increment(1);
}

/// Record a connection close (decrement active gauge).
pub fn connection_closed() {
    gauge!(CONNECTIONS_ACTIVE).decrement(1.0);
}

/// Record a completed query with its type label (`read` or `write`).
pub fn query_completed(query_type: &str) {
    let label = match query_type {
        "rw" | "w" => "write",
        _ => "read",
    };
    counter!(QUERIES_TOTAL, "type" => label.to_string()).increment(1);
}

/// Record query duration in seconds.
pub fn query_duration(seconds: f64) {
    histogram!(QUERY_DURATION_SECONDS).record(seconds);
}

/// Record an error with a Neo4j-style status code.
pub fn error_recorded(code: &str) {
    counter!(ERRORS_TOTAL, "code" => code.to_string()).increment(1);
}
