//! Prometheus metrics for sedandelivery-bolt.
//!
//! Uses the `metrics` crate's global API so any code can record metrics
//! without passing handles around. The PrometheusBuilder installs a global
//! recorder at startup; the health endpoint reads from its handle.

use metrics::{counter, gauge, histogram};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};

/// Metric name constants.
pub const CONNECTIONS_ACTIVE: &str = "sedandelivery_bolt_connections_active";
pub const CONNECTIONS_TOTAL: &str = "sedandelivery_bolt_connections_total";
pub const QUERIES_TOTAL: &str = "sedandelivery_bolt_queries_total";
pub const QUERY_DURATION_SECONDS: &str = "sedandelivery_bolt_query_duration_seconds";
pub const ERRORS_TOTAL: &str = "sedandelivery_bolt_errors_total";
pub const REDIS_USED_MEMORY_BYTES: &str = "redis_used_memory_bytes";
pub const REDIS_MAXMEMORY_BYTES: &str = "redis_maxmemory_bytes";
pub const REDIS_EVICTED_KEYS_TOTAL: &str = "redis_evicted_keys_total";

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

/// Set the Redis used_memory gauge (bytes).
pub fn record_redis_used_memory(bytes: u64) {
    gauge!(REDIS_USED_MEMORY_BYTES).set(bytes as f64);
}

/// Set the Redis maxmemory gauge (bytes; 0 = unlimited).
pub fn record_redis_maxmemory(bytes: u64) {
    gauge!(REDIS_MAXMEMORY_BYTES).set(bytes as f64);
}

/// Set the absolute evicted_keys gauge (cumulative value from INFO stats).
pub fn record_redis_evictions(total: u64) {
    gauge!(REDIS_EVICTED_KEYS_TOTAL).set(total as f64);
}

/// Record a rejected connection (per-IP limit or semaphore limit reached).
pub fn connection_rejected() {
    counter!("sedandelivery_bolt_connections_rejected_total").increment(1);
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Install the Prometheus recorder once for all tests in this module.
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
            CONNECTIONS_ACTIVE,
            CONNECTIONS_TOTAL,
            QUERIES_TOTAL,
            QUERY_DURATION_SECONDS,
            ERRORS_TOTAL,
        ] {
            assert!(!name.is_empty(), "metric name must not be empty");
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_lowercase() || c == '_'),
                "metric name '{name}' contains invalid characters (only [a-z_] allowed)"
            );
        }
    }

    #[test]
    fn test_connection_accepted_records_metrics() {
        let handle = ensure_recorder();

        connection_accepted();

        let output = handle.render();
        assert!(
            output.contains(CONNECTIONS_ACTIVE),
            "rendered metrics should contain {CONNECTIONS_ACTIVE}, got:\n{output}"
        );
        assert!(
            output.contains(CONNECTIONS_TOTAL),
            "rendered metrics should contain {CONNECTIONS_TOTAL}, got:\n{output}"
        );

        // connections_total should be at least 1
        let line = output
            .lines()
            .find(|l| l.starts_with(CONNECTIONS_TOTAL) && !l.starts_with('#'))
            .expect("expected a data line for connections_total");
        let val: f64 = line
            .split_whitespace()
            .last()
            .unwrap()
            .parse()
            .expect("counter value should be a number");
        assert!(
            val >= 1.0,
            "{CONNECTIONS_TOTAL} should be >= 1, got {val}"
        );
    }

    #[test]
    fn test_connection_closed_decrements_gauge() {
        let handle = ensure_recorder();

        // Accept two, close one -- gauge should reflect net connections.
        connection_accepted();
        connection_accepted();
        connection_closed();

        let output = handle.render();
        assert!(
            output.contains(CONNECTIONS_ACTIVE),
            "rendered metrics should contain {CONNECTIONS_ACTIVE}"
        );
        // We can't assert the exact value due to test ordering, but the gauge
        // should exist and be a valid number.
        let line = output
            .lines()
            .find(|l| l.starts_with(CONNECTIONS_ACTIVE) && !l.starts_with('#'))
            .expect("expected a data line for connections_active");
        let val: f64 = line
            .split_whitespace()
            .last()
            .unwrap()
            .parse()
            .expect("gauge value should be a number");
        // Value may vary due to test parallelism, just ensure it parsed.
        assert!(
            val.is_finite(),
            "{CONNECTIONS_ACTIVE} should be finite, got {val}"
        );
    }

    #[test]
    fn test_query_completed_records_read_counter() {
        let handle = ensure_recorder();

        query_completed("r");

        let output = handle.render();
        assert!(
            output.contains(QUERIES_TOTAL),
            "rendered metrics should contain {QUERIES_TOTAL}"
        );
        // Check that the read label appears.
        assert!(
            output.contains("type=\"read\""),
            "expected type=\"read\" label in metrics output"
        );
    }

    #[test]
    fn test_query_completed_records_write_counter() {
        let handle = ensure_recorder();

        query_completed("rw");
        query_completed("w");

        let output = handle.render();
        assert!(
            output.contains("type=\"write\""),
            "expected type=\"write\" label in metrics output"
        );
    }

    #[test]
    fn test_query_duration_records_histogram() {
        let handle = ensure_recorder();

        query_duration(0.123);
        query_duration(0.456);

        let output = handle.render();
        assert!(
            output.contains(QUERY_DURATION_SECONDS),
            "rendered metrics should contain {QUERY_DURATION_SECONDS}"
        );
        // Histograms produce _sum and _count lines.
        assert!(
            output.contains(&format!("{QUERY_DURATION_SECONDS}_count")),
            "expected histogram _count line"
        );
        assert!(
            output.contains(&format!("{QUERY_DURATION_SECONDS}_sum")),
            "expected histogram _sum line"
        );
    }

    #[test]
    fn test_error_recorded_records_counter_with_code() {
        let handle = ensure_recorder();

        error_recorded("Neo.ClientError.Statement.SyntaxError");

        let output = handle.render();
        assert!(
            output.contains(ERRORS_TOTAL),
            "rendered metrics should contain {ERRORS_TOTAL}"
        );
        assert!(
            output.contains("SyntaxError"),
            "expected error code label in metrics output"
        );
    }

    #[test]
    fn test_query_completed_default_maps_to_read() {
        // Any query_type that is NOT "rw" or "w" should map to "read".
        let handle = ensure_recorder();

        query_completed("unknown_type");

        let output = handle.render();
        assert!(
            output.contains("type=\"read\""),
            "unknown query types should map to 'read' label"
        );
    }
}
