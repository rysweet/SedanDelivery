//! Health and metrics HTTP endpoint for sedandelivery-bolt.
//!
//! Spawns a lightweight axum HTTP server on a separate port (default 7688)
//! that exposes:
//!   - `GET /health`  -- JSON health status
//!   - `GET /metrics` -- Prometheus text-format metrics

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use gql_core::storage::GraphStore;
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
struct HealthResponse {
    status: &'static str,
    connections: u64,
    uptime_seconds: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// Spawn the health/metrics HTTP server as a background tokio task.
///
/// The server binds to `{bind_addr}:{port}` where bind_addr defaults to
/// `0.0.0.0` (override with `BOLT_HEALTH_BIND_ADDR`) and port defaults
/// to 7688 (override with `BOLT_HEALTH_PORT`).
pub fn spawn_health_server(state: HealthState) {
    let port = std::env::var("BOLT_HEALTH_PORT")
        .ok()
        .and_then(|v| match v.parse::<u16>() {
            Ok(val) => Some(val),
            Err(_) => {
                tracing::warn!("invalid value for BOLT_HEALTH_PORT: {v:?}, using default 7688");
                None
            }
        })
        .unwrap_or(7688);

    let bind_addr =
        std::env::var("BOLT_HEALTH_BIND_ADDR").unwrap_or_else(|_| "0.0.0.0".to_string());

    // Pre-validate the full socket address to catch typos early.
    let sock_addr_str = format!("{bind_addr}:{port}");
    let sock_addr: std::net::SocketAddr = match sock_addr_str.parse() {
        Ok(addr) => addr,
        Err(_) => {
            tracing::warn!(
                "BOLT_HEALTH_BIND_ADDR={bind_addr:?} with port={port} does not form a valid SocketAddr, \
                 falling back to 0.0.0.0:{port}"
            );
            format!("0.0.0.0:{port}")
                .parse()
                .expect("fallback addr must parse")
        }
    };

    let app = build_router(state);

    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(sock_addr)
            .await
            .expect("failed to bind health HTTP listener");
        tracing::info!(%bind_addr, port, "health/metrics server listening");
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!("health server error: {e}");
        }
    });
}

async fn health_handler(State(state): State<HealthState>) -> impl IntoResponse {
    let connections = state.active_connections.load(Ordering::Relaxed);
    let uptime_seconds = state.start_time.elapsed().as_secs();

    match state.store.graph_exists("__health_probe") {
        Ok(_) => (
            StatusCode::OK,
            axum::Json(HealthResponse {
                status: "ok",
                connections,
                uptime_seconds,
                error: None,
            }),
        ),
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(HealthResponse {
                status: "degraded",
                connections,
                uptime_seconds,
                error: Some(e.to_string()),
            }),
        ),
    }
}

/// Build the axum router. Extracted for testability.
fn build_router(state: HealthState) -> Router {
    Router::new()
        .route("/health", get(health_handler))
        .route("/metrics", get(metrics_handler))
        .with_state(state)
}

/// Deserializable version of [`HealthResponse`] for tests.
#[cfg(test)]
#[derive(serde::Deserialize, Debug)]
struct HealthResponseTest {
    status: String,
    connections: u64,
    uptime_seconds: u64,
    error: Option<String>,
}

async fn metrics_handler(State(state): State<HealthState>) -> Response {
    let body = state.prometheus_handle.render();
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
    use std::any::Any;
    use std::collections::HashMap;

    use gql_core::storage::{
        AdjEntry, EdgeData, EdgeId, GraphStats, IndexDescriptor, InMemoryGraphStore, NodeData,
        NodeId, RangeQuery,
    };
    use gql_core::error::StorageError;
    use gql_core::types::Value;
    use http_body_util::BodyExt;
    use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
    use tower::ServiceExt;

    /// Create a PrometheusHandle for tests without installing a global
    /// recorder. This avoids conflicts with the metrics module's tests
    /// which install their own global recorder.
    fn test_prometheus_handle() -> PrometheusHandle {
        let recorder = PrometheusBuilder::new().build_recorder();
        recorder.handle()
    }

    /// Build a [`HealthState`] backed by an [`InMemoryGraphStore`] for tests.
    fn test_state() -> HealthState {
        HealthState {
            prometheus_handle: test_prometheus_handle(),
            active_connections: Arc::new(AtomicU64::new(0)),
            start_time: Instant::now(),
            store: Arc::new(InMemoryGraphStore::new()),
        }
    }

    /// A store wrapper that delegates everything to [`InMemoryGraphStore`]
    /// except `graph_exists`, which always returns an error. Used to test
    /// the degraded health path.
    struct FailingStore(InMemoryGraphStore);

    impl FailingStore {
        fn new() -> Self {
            Self(InMemoryGraphStore::new())
        }
    }

    /// Forward a simple trait method to `self.0`.
    macro_rules! delegate {
        ($name:ident(&self) -> $ret:ty) => {
            fn $name(&self) -> $ret { self.0.$name() }
        };
        ($name:ident(&self, $($arg:ident : $ty:ty),+) -> $ret:ty) => {
            fn $name(&self, $($arg: $ty),+) -> $ret { self.0.$name($($arg),+) }
        };
    }

    impl GraphStore for FailingStore {
        fn graph_exists(&self, _name: &str) -> Result<bool, StorageError> {
            Err(StorageError::Backend {
                msg: "simulated storage failure".into(),
            })
        }

        delegate!(get_node(&self, graph: &str, id: NodeId) -> Result<Option<NodeData>, StorageError>);
        delegate!(get_edge(&self, graph: &str, id: EdgeId) -> Result<Option<EdgeData>, StorageError>);
        fn get_nodes_batch(&self, graph: &str, ids: &[NodeId]) -> Result<Vec<Option<NodeData>>, StorageError> { self.0.get_nodes_batch(graph, ids) }
        fn get_edges_batch(&self, graph: &str, ids: &[EdgeId]) -> Result<Vec<Option<EdgeData>>, StorageError> { self.0.get_edges_batch(graph, ids) }
        delegate!(find_nodes_by_label(&self, graph: &str, label: &str) -> Result<Vec<NodeId>, StorageError>);
        fn find_nodes_by_label_and_property(&self, graph: &str, label: &str, property: &str, range: RangeQuery<'_>) -> Result<Vec<NodeId>, StorageError> { self.0.find_nodes_by_label_and_property(graph, label, property, range) }
        delegate!(find_edges_by_type(&self, graph: &str, rel_type: &str) -> Result<Vec<EdgeId>, StorageError>);
        delegate!(out_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError>);
        delegate!(in_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError>);
        fn out_edges_by_type(&self, graph: &str, node_id: NodeId, rel_types: &[&str]) -> Result<Vec<AdjEntry>, StorageError> { self.0.out_edges_by_type(graph, node_id, rel_types) }
        fn in_edges_by_type(&self, graph: &str, node_id: NodeId, rel_types: &[&str]) -> Result<Vec<AdjEntry>, StorageError> { self.0.in_edges_by_type(graph, node_id, rel_types) }
        delegate!(out_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError>);
        delegate!(in_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError>);
        delegate!(all_node_ids(&self, graph: &str) -> Result<Vec<NodeId>, StorageError>);
        delegate!(all_edge_ids(&self, graph: &str) -> Result<Vec<EdgeId>, StorageError>);
        fn create_node(&self, graph: &str, labels: Vec<String>, properties: HashMap<String, Value>) -> Result<NodeId, StorageError> { self.0.create_node(graph, labels, properties) }
        fn create_edge(&self, graph: &str, src: NodeId, dst: NodeId, rel_type: String, properties: HashMap<String, Value>) -> Result<EdgeId, StorageError> { self.0.create_edge(graph, src, dst, rel_type, properties) }
        fn set_node_properties(&self, graph: &str, id: NodeId, properties: HashMap<String, Value>) -> Result<(), StorageError> { self.0.set_node_properties(graph, id, properties) }
        fn replace_node_properties(&self, graph: &str, id: NodeId, properties: HashMap<String, Value>) -> Result<(), StorageError> { self.0.replace_node_properties(graph, id, properties) }
        fn set_edge_properties(&self, graph: &str, id: EdgeId, properties: HashMap<String, Value>) -> Result<(), StorageError> { self.0.set_edge_properties(graph, id, properties) }
        fn replace_edge_properties(&self, graph: &str, id: EdgeId, properties: HashMap<String, Value>) -> Result<(), StorageError> { self.0.replace_edge_properties(graph, id, properties) }
        delegate!(remove_node_property(&self, graph: &str, id: NodeId, key: &str) -> Result<(), StorageError>);
        delegate!(remove_edge_property(&self, graph: &str, id: EdgeId, key: &str) -> Result<(), StorageError>);
        fn add_node_labels(&self, graph: &str, id: NodeId, labels: &[String]) -> Result<(), StorageError> { self.0.add_node_labels(graph, id, labels) }
        fn remove_node_labels(&self, graph: &str, id: NodeId, labels: &[String]) -> Result<(), StorageError> { self.0.remove_node_labels(graph, id, labels) }
        fn delete_node(&self, graph: &str, id: NodeId, detach: bool) -> Result<(), StorageError> { self.0.delete_node(graph, id, detach) }
        delegate!(delete_edge(&self, graph: &str, id: EdgeId) -> Result<(), StorageError>);
        delegate!(list_indexes(&self, graph: &str) -> Result<Vec<IndexDescriptor>, StorageError>);
        fn create_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> { self.0.create_index(graph, label, property) }
        fn drop_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> { self.0.drop_index(graph, label, property) }
        delegate!(create_graph(&self, name: &str) -> Result<(), StorageError>);
        delegate!(delete_graph(&self, name: &str) -> Result<(), StorageError>);
        delegate!(list_graphs(&self) -> Result<Vec<String>, StorageError>);
        delegate!(graph_stats(&self, graph: &str) -> Result<GraphStats, StorageError>);
        delegate!(all_label_names(&self, graph: &str) -> Result<Vec<String>, StorageError>);
        fn as_any(&self) -> &dyn Any { self }
    }

    fn failing_state() -> HealthState {
        HealthState {
            prometheus_handle: test_prometheus_handle(),
            active_connections: Arc::new(AtomicU64::new(0)),
            start_time: Instant::now(),
            store: Arc::new(FailingStore::new()),
        }
    }

    #[tokio::test]
    async fn health_handler_returns_ok_with_healthy_store() {
        let state = test_state();
        let app = build_router(state);

        let req = http::Request::builder()
            .uri("/health")
            .body(axum::body::Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let parsed: HealthResponseTest = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed.status, "ok");
        assert!(parsed.error.is_none());
    }

    #[tokio::test]
    async fn health_handler_returns_503_when_store_fails() {
        let state = failing_state();
        let app = build_router(state);

        let req = http::Request::builder()
            .uri("/health")
            .body(axum::body::Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let parsed: HealthResponseTest = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed.status, "degraded");
        assert!(parsed.error.is_some());
        assert!(
            parsed.error.as_ref().unwrap().contains("simulated"),
            "error message should include the underlying cause"
        );
    }

    #[tokio::test]
    async fn health_handler_reports_active_connections() {
        let state = test_state();
        state.active_connections.store(42, Ordering::Relaxed);
        let app = build_router(state);

        let req = http::Request::builder()
            .uri("/health")
            .body(axum::body::Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let parsed: HealthResponseTest = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed.connections, 42);
    }

    #[tokio::test]
    async fn health_handler_reports_uptime() {
        let state = test_state();
        let app = build_router(state);

        let req = http::Request::builder()
            .uri("/health")
            .body(axum::body::Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let parsed: HealthResponseTest = serde_json::from_slice(&body).unwrap();
        // Uptime should be 0 or very small since we just created the state.
        assert!(parsed.uptime_seconds <= 2);
    }

    #[tokio::test]
    async fn health_server_binds_to_all_interfaces_by_default() {
        // Spawn the health server without setting BOLT_HEALTH_BIND_ADDR.
        // It should bind to 0.0.0.0 (all interfaces), which means we can
        // connect via 127.0.0.1.
        let state = test_state();

        // Pick a random available port to avoid conflicts.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        // Temporarily set the env vars for this test.
        // SAFETY: test runs are single-threaded per test function; no other
        // thread is reading these env vars concurrently within this test.
        unsafe {
            std::env::set_var("BOLT_HEALTH_PORT", port.to_string());
            std::env::remove_var("BOLT_HEALTH_BIND_ADDR");
        }
        spawn_health_server(state);
        unsafe {
            std::env::remove_var("BOLT_HEALTH_PORT");
        }

        // Give the server a moment to start.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Connect via 127.0.0.1 -- this works because the server binds 0.0.0.0.
        let stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .expect("should connect to health server bound on 0.0.0.0 via 127.0.0.1");
        drop(stream);
    }

    #[tokio::test]
    async fn metrics_endpoint_returns_prometheus_format() {
        let state = test_state();
        let app = build_router(state);

        let req = http::Request::builder()
            .uri("/metrics")
            .body(axum::body::Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let ct = resp
            .headers()
            .get("content-type")
            .expect("metrics response must have content-type header")
            .to_str()
            .unwrap();
        assert!(
            ct.contains("text/plain"),
            "metrics content-type should be text/plain, got: {ct}"
        );
    }
}
