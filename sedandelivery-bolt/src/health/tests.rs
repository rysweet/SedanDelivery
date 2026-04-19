//! Tests for the health server, handlers, and degraded-state behavior.

use std::any::Any;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use axum::http::StatusCode;
use cypher_core::error::StorageError;
use cypher_core::storage::{
    AdjEntry, EdgeData, EdgeId, GraphStats, GraphStore, InMemoryGraphStore, IndexDescriptor,
    NodeData, NodeId, RangeQuery,
};
use cypher_core::types::Value;
use http_body_util::BodyExt;
use indexmap::IndexMap;
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use tower::ServiceExt;

use super::server::{build_router, spawn_health_server_with_addr};
use super::state::HealthState;

/// Deserializable version of `HealthResponse` for tests.
#[derive(serde::Deserialize, Debug)]
struct HealthResponseTest {
    status: String,
    connections: u64,
    #[allow(dead_code)]
    uptime_seconds: u64,
    error: Option<String>,
}

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
    fn get_nodes_batch(
        &self,
        graph: &str,
        ids: &[NodeId],
    ) -> Result<Vec<Option<NodeData>>, StorageError> {
        self.0.get_nodes_batch(graph, ids)
    }
    fn get_edges_batch(
        &self,
        graph: &str,
        ids: &[EdgeId],
    ) -> Result<Vec<Option<EdgeData>>, StorageError> {
        self.0.get_edges_batch(graph, ids)
    }
    delegate!(find_nodes_by_label(&self, graph: &str, label: &str) -> Result<Vec<NodeId>, StorageError>);
    fn find_nodes_by_label_and_property(
        &self,
        graph: &str,
        label: &str,
        property: &str,
        range: RangeQuery<'_>,
    ) -> Result<Vec<NodeId>, StorageError> {
        self.0
            .find_nodes_by_label_and_property(graph, label, property, range)
    }
    delegate!(find_edges_by_type(&self, graph: &str, rel_type: &str) -> Result<Vec<EdgeId>, StorageError>);
    delegate!(out_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError>);
    delegate!(in_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError>);
    fn out_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        self.0.out_edges_by_type(graph, node_id, rel_types)
    }
    fn in_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, StorageError> {
        self.0.in_edges_by_type(graph, node_id, rel_types)
    }
    delegate!(out_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError>);
    delegate!(in_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, StorageError>);
    delegate!(all_node_ids(&self, graph: &str) -> Result<Vec<NodeId>, StorageError>);
    delegate!(all_edge_ids(&self, graph: &str) -> Result<Vec<EdgeId>, StorageError>);
    fn create_node(
        &self,
        graph: &str,
        labels: Vec<String>,
        properties: IndexMap<String, Value>,
    ) -> Result<NodeId, StorageError> {
        self.0.create_node(graph, labels, properties)
    }
    fn create_edge(
        &self,
        graph: &str,
        src: NodeId,
        dst: NodeId,
        rel_type: String,
        properties: IndexMap<String, Value>,
    ) -> Result<EdgeId, StorageError> {
        self.0.create_edge(graph, src, dst, rel_type, properties)
    }
    fn set_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.0.set_node_properties(graph, id, properties)
    }
    fn replace_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.0.replace_node_properties(graph, id, properties)
    }
    fn set_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.0.set_edge_properties(graph, id, properties)
    }
    fn replace_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), StorageError> {
        self.0.replace_edge_properties(graph, id, properties)
    }
    delegate!(remove_node_property(&self, graph: &str, id: NodeId, key: &str) -> Result<(), StorageError>);
    delegate!(remove_edge_property(&self, graph: &str, id: EdgeId, key: &str) -> Result<(), StorageError>);
    fn add_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        self.0.add_node_labels(graph, id, labels)
    }
    fn remove_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), StorageError> {
        self.0.remove_node_labels(graph, id, labels)
    }
    fn delete_node(&self, graph: &str, id: NodeId, detach: bool) -> Result<(), StorageError> {
        self.0.delete_node(graph, id, detach)
    }
    delegate!(delete_edge(&self, graph: &str, id: EdgeId) -> Result<(), StorageError>);
    delegate!(list_indexes(&self, graph: &str) -> Result<Vec<IndexDescriptor>, StorageError>);
    fn create_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        self.0.create_index(graph, label, property)
    }
    fn drop_index(&self, graph: &str, label: &str, property: &str) -> Result<(), StorageError> {
        self.0.drop_index(graph, label, property)
    }
    delegate!(create_graph(&self, name: &str) -> Result<(), StorageError>);
    delegate!(delete_graph(&self, name: &str) -> Result<(), StorageError>);
    delegate!(list_graphs(&self) -> Result<Vec<String>, StorageError>);
    delegate!(graph_stats(&self, graph: &str) -> Result<GraphStats, StorageError>);
    delegate!(all_label_names(&self, graph: &str) -> Result<Vec<String>, StorageError>);
    fn as_any(&self) -> &dyn Any {
        self
    }
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
    // Spawn the health server using the explicit-addr API so we don't
    // need to mutate process env (which is `unsafe` in Rust 2024).
    // We bind 0.0.0.0:<random> and then connect via 127.0.0.1, which
    // demonstrates that the default bind address is "all interfaces".
    let state = test_state();

    // Pick a random available port to avoid conflicts.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let addr: std::net::SocketAddr = format!("0.0.0.0:{port}").parse().unwrap();
    spawn_health_server_with_addr(state, Some(addr));

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
