//! Shared state and response types for the health server.

use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Instant;

use cypher_core::storage::GraphStore;
use metrics_exporter_prometheus::PrometheusHandle;
use serde::Serialize;

/// Shared state for the health server.
#[derive(Clone)]
pub struct HealthState {
    pub prometheus_handle: PrometheusHandle,
    pub active_connections: Arc<AtomicU64>,
    pub start_time: Instant,
    pub store: Arc<dyn GraphStore>,
}

#[derive(Serialize)]
pub(super) struct HealthResponse {
    pub(super) status: &'static str,
    pub(super) connections: u64,
    pub(super) uptime_seconds: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) error: Option<String>,
}
