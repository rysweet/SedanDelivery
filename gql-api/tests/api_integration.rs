//! Integration tests for gql-api route handlers.
//!
//! Each test builds a full Axum `Router` backed by an `InMemoryGraphStore`,
//! sends a real HTTP request via `tower::ServiceExt::oneshot`, and asserts on
//! the response status and body.  Every test uses a unique graph name derived
//! from the test function name to avoid cross-test interference.

use std::sync::Arc;

use axum::body::Body;
use axum::Router;
use gql_core::storage::{GraphStore, InMemoryGraphStore};
use http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use gql_api::routes;
use gql_api::state::AppState;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a fresh router with an in-memory store.
fn app() -> Router {
    let store = Arc::new(InMemoryGraphStore::new());
    let state = AppState::new(store);
    Router::new()
        .merge(routes::health::router())
        .merge(routes::graphs::router())
        .merge(routes::nodes::router())
        .merge(routes::edges::router())
        .merge(routes::query::router())
        .merge(routes::schema::router())
        .merge(routes::neighbors::router())
        .merge(routes::export_import::router())
        .with_state(state)
}

/// Build a router whose store already contains a graph named `graph_name`.
fn app_with_graph(graph_name: &str) -> Router {
    let store = Arc::new(InMemoryGraphStore::new());
    store.create_graph(graph_name).expect("seed graph");
    let state = AppState::new(store);
    Router::new()
        .merge(routes::health::router())
        .merge(routes::graphs::router())
        .merge(routes::nodes::router())
        .merge(routes::edges::router())
        .merge(routes::query::router())
        .merge(routes::schema::router())
        .merge(routes::neighbors::router())
        .merge(routes::export_import::router())
        .with_state(state)
}

/// Collect the response body bytes into a `serde_json::Value`.
async fn body_json(body: Body) -> Value {
    let bytes = body.collect().await.expect("collect body").to_bytes();
    serde_json::from_slice(&bytes).expect("parse JSON body")
}

/// Build a JSON POST request for the given URI.
fn json_post(uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

/// Build a JSON PUT request for the given URI.
fn json_put(uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(Method::PUT)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

// ===========================================================================
// 1. Health
// ===========================================================================

#[tokio::test]
async fn health_returns_200() {
    let resp = app()
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["status"], "ok");
}

// ===========================================================================
// 2-5. Graphs CRUD
// ===========================================================================

#[tokio::test]
async fn create_graph_returns_201() {
    let resp = app()
        .oneshot(json_post(
            "/graphs",
            json!({"name": "create_graph_returns_201"}),
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["name"], "create_graph_returns_201");
}

#[tokio::test]
async fn create_duplicate_graph_returns_409() {
    let router = app();

    // First creation succeeds.
    let resp = router
        .clone()
        .oneshot(json_post(
            "/graphs",
            json!({"name": "create_duplicate_graph_returns_409"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Second creation conflicts.
    let resp = router
        .oneshot(json_post(
            "/graphs",
            json!({"name": "create_duplicate_graph_returns_409"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn list_graphs_contains_created_graph() {
    let router = app();
    let name = "list_graphs_contains_created_graph";

    // Create a graph first.
    let _ = router
        .clone()
        .oneshot(json_post("/graphs", json!({"name": name})))
        .await
        .unwrap();

    // List graphs.
    let resp = router
        .oneshot(
            Request::builder()
                .uri("/graphs")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = body_json(resp.into_body()).await;
    let graphs = body["graphs"].as_array().expect("graphs is array");
    assert!(
        graphs.iter().any(|g| g.as_str() == Some(name)),
        "expected graph list to contain '{name}', got {graphs:?}"
    );
}

#[tokio::test]
async fn delete_graph_returns_204() {
    let router = app_with_graph("delete_graph_returns_204");

    let resp = router
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri("/graphs/delete_graph_returns_204")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn delete_nonexistent_graph_returns_404() {
    let resp = app()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri("/graphs/delete_nonexistent_graph_404")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ===========================================================================
// 6. Graph stats for nonexistent graph
// ===========================================================================

#[tokio::test]
async fn stats_nonexistent_graph_returns_404() {
    let resp = app()
        .oneshot(
            Request::builder()
                .uri("/graphs/nonexistent_stats_test/stats")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn stats_existing_graph_returns_200() {
    let router = app_with_graph("stats_existing_graph");

    let resp = router
        .oneshot(
            Request::builder()
                .uri("/graphs/stats_existing_graph/stats")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = body_json(resp.into_body()).await;
    assert_eq!(body["nodeCount"], 0);
    assert_eq!(body["edgeCount"], 0);
}

// ===========================================================================
// 7-8. Nodes CRUD
// ===========================================================================

#[tokio::test]
async fn create_node_returns_201() {
    let graph = "create_node_returns_201";
    let router = app_with_graph(graph);

    let resp = router
        .oneshot(json_post(
            &format!("/graphs/{graph}/nodes"),
            json!({"labels": ["Person"], "properties": {"name": "Alice"}}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let body = body_json(resp.into_body()).await;
    assert!(body.get("id").is_some(), "response should contain node id");
    let labels = body["labels"].as_array().expect("labels array");
    assert!(labels.iter().any(|l| l.as_str() == Some("Person")));
}

#[tokio::test]
async fn list_nodes_returns_created_node() {
    let graph = "list_nodes_returns_created";
    let router = app_with_graph(graph);

    // Create a node.
    let _ = router
        .clone()
        .oneshot(json_post(
            &format!("/graphs/{graph}/nodes"),
            json!({"labels": ["Person"], "properties": {"name": "Bob"}}),
        ))
        .await
        .unwrap();

    // List nodes.
    let resp = router
        .oneshot(
            Request::builder()
                .uri(&format!("/graphs/{graph}/nodes"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = body_json(resp.into_body()).await;
    assert_eq!(body["total"], 1);
    let nodes = body["nodes"].as_array().expect("nodes array");
    assert_eq!(nodes.len(), 1);
}

// ===========================================================================
// 9. Nodes pagination limit exceeded
// ===========================================================================

#[tokio::test]
async fn list_nodes_excessive_limit_returns_400() {
    let graph = "list_nodes_excessive_limit";
    let router = app_with_graph(graph);

    let resp = router
        .oneshot(
            Request::builder()
                .uri(&format!("/graphs/{graph}/nodes?limit=99999"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let body = body_json(resp.into_body()).await;
    assert!(
        body["error"].as_str().unwrap_or("").contains("limit must"),
        "error message should mention limit: {body}"
    );
}

#[tokio::test]
async fn list_nodes_zero_limit_returns_400() {
    let graph = "list_nodes_zero_limit";
    let router = app_with_graph(graph);

    let resp = router
        .oneshot(
            Request::builder()
                .uri(&format!("/graphs/{graph}/nodes?limit=0"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

// ===========================================================================
// 10-11. Cypher query
// ===========================================================================

#[tokio::test]
async fn query_count_returns_200() {
    let graph = "query_count_returns_200";
    let router = app_with_graph(graph);

    let resp = router
        .oneshot(json_post(
            &format!("/graphs/{graph}/query"),
            json!({"cypher": "MATCH (n) RETURN count(n)"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = body_json(resp.into_body()).await;
    assert!(body.get("columns").is_some());
    assert!(body.get("rows").is_some());
    assert!(body.get("elapsed_ms").is_some());
}

#[tokio::test]
async fn query_invalid_cypher_returns_400() {
    let graph = "query_invalid_cypher";
    let router = app_with_graph(graph);

    let resp = router
        .oneshot(json_post(
            &format!("/graphs/{graph}/query"),
            json!({"cypher": "THIS IS NOT VALID CYPHER !!!"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn query_nonexistent_graph_returns_404() {
    let resp = app()
        .oneshot(json_post(
            "/graphs/no_such_graph_query/query",
            json!({"cypher": "MATCH (n) RETURN n"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ===========================================================================
// 12. Cypher mutate
// ===========================================================================

#[tokio::test]
async fn mutate_create_node_returns_200() {
    let graph = "mutate_create_node";
    let router = app_with_graph(graph);

    let resp = router
        .oneshot(json_post(
            &format!("/graphs/{graph}/mutate"),
            json!({"cypher": "CREATE (:Company {name: 'Acme'})"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = body_json(resp.into_body()).await;
    assert_eq!(body["nodesCreated"], 1);
}

// ===========================================================================
// 13. Schema
// ===========================================================================

#[tokio::test]
async fn schema_contains_labels() {
    let graph = "schema_contains_labels";
    let router = app_with_graph(graph);

    // Create a Person node first.
    let _ = router
        .clone()
        .oneshot(json_post(
            &format!("/graphs/{graph}/nodes"),
            json!({"labels": ["Person"], "properties": {"name": "Alice"}}),
        ))
        .await
        .unwrap();

    // Fetch schema.
    let resp = router
        .oneshot(
            Request::builder()
                .uri(&format!("/graphs/{graph}/schema"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = body_json(resp.into_body()).await;
    let labels = body["labels"].as_array().expect("labels array");
    assert!(
        labels.iter().any(|l| l["name"] == "Person"),
        "expected schema to contain Person label, got {labels:?}"
    );
}

// ===========================================================================
// 14. Export
// ===========================================================================

#[tokio::test]
async fn export_contains_nodes_and_edges() {
    let graph = "export_contains_nodes";
    let router = app_with_graph(graph);

    // Seed a node.
    let _ = router
        .clone()
        .oneshot(json_post(
            &format!("/graphs/{graph}/nodes"),
            json!({"labels": ["Thing"], "properties": {"x": 1}}),
        ))
        .await
        .unwrap();

    // Export.
    let resp = router
        .oneshot(
            Request::builder()
                .uri(&format!("/graphs/{graph}/export"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = body_json(resp.into_body()).await;
    assert!(body.get("nodes").is_some(), "export should contain 'nodes'");
    assert!(body.get("edges").is_some(), "export should contain 'edges'");
    let nodes = body["nodes"].as_array().expect("nodes array");
    assert_eq!(nodes.len(), 1);
}

// ===========================================================================
// 15. Import
// ===========================================================================

#[tokio::test]
async fn import_creates_nodes() {
    let graph = "import_creates_nodes";
    let router = app_with_graph(graph);

    let resp = router
        .oneshot(json_post(
            &format!("/graphs/{graph}/import"),
            json!({
                "nodes": [
                    {"labels": ["Person"], "properties": {"name": "Alice"}, "id": "n1"},
                    {"labels": ["Person"], "properties": {"name": "Bob"}, "id": "n2"}
                ],
                "edges": [
                    {"source": "n1", "target": "n2", "type": "KNOWS"}
                ]
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = body_json(resp.into_body()).await;
    assert_eq!(body["nodesCreated"], 2);
    assert_eq!(body["edgesCreated"], 1);
}

// ===========================================================================
// Additional edge cases and coverage
// ===========================================================================

#[tokio::test]
async fn get_node_by_id_returns_200() {
    let graph = "get_node_by_id";
    let router = app_with_graph(graph);

    // Create a node and extract its ID.
    let create_resp = router
        .clone()
        .oneshot(json_post(
            &format!("/graphs/{graph}/nodes"),
            json!({"labels": ["Person"], "properties": {"name": "Carol"}}),
        ))
        .await
        .unwrap();
    assert_eq!(create_resp.status(), StatusCode::CREATED);
    let created = body_json(create_resp.into_body()).await;
    let node_id = created["id"].as_str().expect("id should be a string");

    // Fetch by ID.
    let resp = router
        .oneshot(
            Request::builder()
                .uri(&format!("/graphs/{graph}/nodes/{node_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["id"].as_str().unwrap(), node_id);
}

#[tokio::test]
async fn get_nonexistent_node_returns_404() {
    let graph = "get_nonexistent_node";
    let router = app_with_graph(graph);

    let resp = router
        .oneshot(
            Request::builder()
                .uri(&format!("/graphs/{graph}/nodes/999999"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn update_node_returns_updated_properties() {
    let graph = "update_node_props";
    let router = app_with_graph(graph);

    // Create.
    let create_resp = router
        .clone()
        .oneshot(json_post(
            &format!("/graphs/{graph}/nodes"),
            json!({"labels": ["Person"], "properties": {"name": "Dan"}}),
        ))
        .await
        .unwrap();
    let created = body_json(create_resp.into_body()).await;
    let node_id = created["id"].as_str().expect("id is string");

    // Update.
    let resp = router
        .oneshot(json_put(
            &format!("/graphs/{graph}/nodes/{node_id}"),
            json!({"properties": {"name": "Daniel", "age": 40}}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = body_json(resp.into_body()).await;
    assert_eq!(body["properties"]["name"], "Daniel");
    assert_eq!(body["properties"]["age"], 40);
}

#[tokio::test]
async fn delete_node_returns_204() {
    let graph = "delete_node_returns_204";
    let router = app_with_graph(graph);

    // Create.
    let create_resp = router
        .clone()
        .oneshot(json_post(
            &format!("/graphs/{graph}/nodes"),
            json!({"labels": ["Tmp"]}),
        ))
        .await
        .unwrap();
    let created = body_json(create_resp.into_body()).await;
    let node_id = created["id"].as_str().expect("id is string");

    // Delete.
    let resp = router
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(&format!("/graphs/{graph}/nodes/{node_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn create_edge_returns_201() {
    let graph = "create_edge_returns_201";
    let router = app_with_graph(graph);

    // Create two nodes.
    let r1 = router
        .clone()
        .oneshot(json_post(
            &format!("/graphs/{graph}/nodes"),
            json!({"labels": ["A"]}),
        ))
        .await
        .unwrap();
    let n1 = body_json(r1.into_body()).await;

    let r2 = router
        .clone()
        .oneshot(json_post(
            &format!("/graphs/{graph}/nodes"),
            json!({"labels": ["B"]}),
        ))
        .await
        .unwrap();
    let n2 = body_json(r2.into_body()).await;

    // Create edge.
    let resp = router
        .oneshot(json_post(
            &format!("/graphs/{graph}/edges"),
            json!({
                "source": n1["id"].to_string().trim_matches('"'),
                "target": n2["id"].to_string().trim_matches('"'),
                "type": "LINKED",
                "properties": {"weight": 1.0}
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let body = body_json(resp.into_body()).await;
    assert!(body.get("id").is_some());
    assert_eq!(body["type"], "LINKED");
}

#[tokio::test]
async fn list_edges_returns_200() {
    let graph = "list_edges_returns_200";
    let router = app_with_graph(graph);

    // Create two nodes and an edge between them.
    let r1 = router
        .clone()
        .oneshot(json_post(
            &format!("/graphs/{graph}/nodes"),
            json!({"labels": ["X"]}),
        ))
        .await
        .unwrap();
    let n1 = body_json(r1.into_body()).await;

    let r2 = router
        .clone()
        .oneshot(json_post(
            &format!("/graphs/{graph}/nodes"),
            json!({"labels": ["Y"]}),
        ))
        .await
        .unwrap();
    let n2 = body_json(r2.into_body()).await;

    let _ = router
        .clone()
        .oneshot(json_post(
            &format!("/graphs/{graph}/edges"),
            json!({
                "source": n1["id"].to_string().trim_matches('"'),
                "target": n2["id"].to_string().trim_matches('"'),
                "type": "REL"
            }),
        ))
        .await
        .unwrap();

    // List edges.
    let resp = router
        .oneshot(
            Request::builder()
                .uri(&format!("/graphs/{graph}/edges"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = body_json(resp.into_body()).await;
    assert_eq!(body["total"], 1);
}

#[tokio::test]
async fn list_edges_excessive_limit_returns_400() {
    let graph = "list_edges_excessive_limit";
    let router = app_with_graph(graph);

    let resp = router
        .oneshot(
            Request::builder()
                .uri(&format!("/graphs/{graph}/edges?limit=99999"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn neighbors_returns_adjacent_nodes() {
    let graph = "neighbors_returns_adjacent";
    let router = app_with_graph(graph);

    // Create two nodes linked by an edge.
    let r1 = router
        .clone()
        .oneshot(json_post(
            &format!("/graphs/{graph}/nodes"),
            json!({"labels": ["Person"], "properties": {"name": "Eve"}}),
        ))
        .await
        .unwrap();
    let n1 = body_json(r1.into_body()).await;

    let r2 = router
        .clone()
        .oneshot(json_post(
            &format!("/graphs/{graph}/nodes"),
            json!({"labels": ["Person"], "properties": {"name": "Frank"}}),
        ))
        .await
        .unwrap();
    let n2 = body_json(r2.into_body()).await;

    let n1_id = n1["id"].to_string().trim_matches('"').to_string();
    let n2_id = n2["id"].to_string().trim_matches('"').to_string();

    let _ = router
        .clone()
        .oneshot(json_post(
            &format!("/graphs/{graph}/edges"),
            json!({"source": n1_id, "target": n2_id, "type": "KNOWS"}),
        ))
        .await
        .unwrap();

    // Get neighbors of n1.
    let resp = router
        .oneshot(
            Request::builder()
                .uri(&format!("/graphs/{graph}/neighbors/{n1_id}?direction=out"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = body_json(resp.into_body()).await;
    let nodes = body["nodes"].as_array().expect("nodes array");
    assert_eq!(nodes.len(), 1, "expected exactly one neighbor");
    let edges = body["edges"].as_array().expect("edges array");
    assert_eq!(edges.len(), 1);
}

#[tokio::test]
async fn neighbors_invalid_direction_returns_400() {
    let graph = "neighbors_invalid_dir";
    let router = app_with_graph(graph);

    // Create a node to query against.
    let create_resp = router
        .clone()
        .oneshot(json_post(
            &format!("/graphs/{graph}/nodes"),
            json!({"labels": ["N"]}),
        ))
        .await
        .unwrap();
    let created = body_json(create_resp.into_body()).await;
    let node_id = created["id"].to_string().trim_matches('"').to_string();

    let resp = router
        .oneshot(
            Request::builder()
                .uri(&format!(
                    "/graphs/{graph}/neighbors/{node_id}?direction=sideways"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn create_graph_invalid_name_returns_400() {
    let resp = app()
        .oneshot(json_post(
            "/graphs",
            json!({"name": "bad name with spaces!"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn import_empty_payload_returns_200() {
    let graph = "import_empty_payload";
    let router = app_with_graph(graph);

    let resp = router
        .oneshot(json_post(
            &format!("/graphs/{graph}/import"),
            json!({"nodes": [], "edges": []}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = body_json(resp.into_body()).await;
    assert_eq!(body["nodesCreated"], 0);
    assert_eq!(body["edgesCreated"], 0);
}

#[tokio::test]
async fn export_empty_graph_returns_empty_arrays() {
    let graph = "export_empty_graph";
    let router = app_with_graph(graph);

    let resp = router
        .oneshot(
            Request::builder()
                .uri(&format!("/graphs/{graph}/export"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = body_json(resp.into_body()).await;
    let nodes = body["nodes"].as_array().expect("nodes array");
    assert!(nodes.is_empty());
    let edges = body["edges"].as_array().expect("edges array");
    assert!(edges.is_empty());
}

#[tokio::test]
async fn schema_empty_graph_returns_empty_labels() {
    let graph = "schema_empty_graph";
    let router = app_with_graph(graph);

    let resp = router
        .oneshot(
            Request::builder()
                .uri(&format!("/graphs/{graph}/schema"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = body_json(resp.into_body()).await;
    let labels = body["labels"].as_array().expect("labels array");
    assert!(labels.is_empty());
    let rels = body["relationshipTypes"].as_array().expect("rels array");
    assert!(rels.is_empty());
}

#[tokio::test]
async fn mutate_nonexistent_graph_returns_404() {
    let resp = app()
        .oneshot(json_post(
            "/graphs/no_such_graph_mutate/mutate",
            json!({"cypher": "CREATE (:X)"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn create_node_on_nonexistent_graph_returns_404() {
    let resp = app()
        .oneshot(json_post(
            "/graphs/no_such_graph_nodes/nodes",
            json!({"labels": ["X"]}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn invalid_node_id_in_path_returns_400() {
    let graph = "invalid_node_id_path";
    let router = app_with_graph(graph);

    let resp = router
        .oneshot(
            Request::builder()
                .uri(&format!("/graphs/{graph}/nodes/not-a-number"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn create_edge_empty_rel_type_returns_400() {
    let graph = "create_edge_empty_reltype";
    let router = app_with_graph(graph);

    // Create two nodes.
    let r1 = router
        .clone()
        .oneshot(json_post(
            &format!("/graphs/{graph}/nodes"),
            json!({"labels": ["A"]}),
        ))
        .await
        .unwrap();
    let n1 = body_json(r1.into_body()).await;
    let n1_id = n1["id"].to_string().trim_matches('"').to_string();

    let r2 = router
        .clone()
        .oneshot(json_post(
            &format!("/graphs/{graph}/nodes"),
            json!({"labels": ["B"]}),
        ))
        .await
        .unwrap();
    let n2 = body_json(r2.into_body()).await;
    let n2_id = n2["id"].to_string().trim_matches('"').to_string();

    // Try to create edge with empty relationship type.
    let resp = router
        .oneshot(json_post(
            &format!("/graphs/{graph}/edges"),
            json!({"source": n1_id, "target": n2_id, "type": ""}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

// ===========================================================================
// Quality Audit Fixes — Outside-In Regression Tests
// ===========================================================================

#[tokio::test]
async fn c2_duplicate_graph_create_returns_409() {
    let graph = "c2_dup_409";
    let router = app();
    let r1 = router.clone().oneshot(json_post("/graphs", json!({"name": graph}))).await.unwrap();
    assert_eq!(r1.status(), StatusCode::CREATED);
    let r2 = router.oneshot(json_post("/graphs", json!({"name": graph}))).await.unwrap();
    assert_eq!(r2.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn c2_delete_nonexistent_graph_returns_404() {
    let router = app();
    let resp = router.oneshot(
        Request::builder().method(Method::DELETE).uri("/graphs/no_such_c2").body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn c6_update_nonexistent_node_returns_404() {
    let graph = "c6_noexist_node";
    let router = app_with_graph(graph);
    let resp = router.oneshot(json_put(
        &format!("/graphs/{graph}/nodes/999999"),
        json!({"properties": {"x": 1}}),
    )).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn c6_update_nonexistent_edge_returns_404() {
    let graph = "c6_noexist_edge";
    let router = app_with_graph(graph);
    let resp = router.oneshot(json_put(
        &format!("/graphs/{graph}/edges/999999"),
        json!({"properties": {"w": 1.0}}),
    )).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn m4_neighbor_limit_over_max_returns_400() {
    let graph = "m4_neighbor_limit";
    let router = app_with_graph(graph);
    let nr = router.clone().oneshot(json_post(
        &format!("/graphs/{graph}/nodes"), json!({"labels": ["A"]}),
    )).await.unwrap();
    let node = body_json(nr.into_body()).await;
    let nid = node["id"].to_string().trim_matches('"').to_string();
    let resp = router.oneshot(
        Request::builder().uri(&format!("/graphs/{graph}/neighbors/{nid}?limit=999999"))
            .body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn h4_order_by_mixed_types_is_deterministic() {
    let graph = "h4_mixed_order";
    let router = app_with_graph(graph);
    router.clone().oneshot(json_post(
        &format!("/graphs/{graph}/mutate"),
        json!({"cypher": "CREATE (:T {v: 42}), (:T {v: 'hello'}), (:T {v: true})"}),
    )).await.unwrap();
    let resp = router.oneshot(json_post(
        &format!("/graphs/{graph}/query"),
        json!({"cypher": "MATCH (n:T) RETURN n.v ORDER BY n.v"}),
    )).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["rows"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn h2_export_small_graph_passes_guard() {
    let graph = "h2_export_ok";
    let router = app_with_graph(graph);
    router.clone().oneshot(json_post(
        &format!("/graphs/{graph}/nodes"), json!({"labels": ["X"], "properties": {"k": "v"}}),
    )).await.unwrap();
    let resp = router.oneshot(
        Request::builder().uri(&format!("/graphs/{graph}/export")).body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert!(!body["nodes"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn h3_schema_small_graph_passes_guard() {
    let graph = "h3_schema_ok";
    let router = app_with_graph(graph);
    router.clone().oneshot(json_post(
        &format!("/graphs/{graph}/nodes"), json!({"labels": ["Person"], "properties": {"name": "Bob"}}),
    )).await.unwrap();
    let resp = router.oneshot(
        Request::builder().uri(&format!("/graphs/{graph}/schema")).body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn explain_endpoint_returns_plan() {
    let graph = "explain_oit";
    let router = app_with_graph(graph);
    let resp = router.oneshot(json_post(
        &format!("/graphs/{graph}/explain"), json!({"cypher": "MATCH (n) RETURN n"}),
    )).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn mutate_returns_stats_with_nodes_created() {
    let graph = "mutate_oit";
    let router = app_with_graph(graph);
    let resp = router.oneshot(json_post(
        &format!("/graphs/{graph}/mutate"), json!({"cypher": "CREATE (:Foo {bar: 'baz'})"}),
    )).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["nodesCreated"], 1);
}

#[tokio::test]
async fn invalid_cypher_returns_400_not_500() {
    let graph = "invalid_cypher_oit";
    let router = app_with_graph(graph);
    let resp = router.oneshot(json_post(
        &format!("/graphs/{graph}/query"), json!({"cypher": "NOT CYPHER AT ALL"}),
    )).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn full_crud_roundtrip_validates_decomposition() {
    let graph = "crud_oit";
    let router = app_with_graph(graph);

    // Create nodes.
    let r1 = router.clone().oneshot(json_post(
        &format!("/graphs/{graph}/nodes"), json!({"labels": ["A"], "properties": {"val": 1}}),
    )).await.unwrap();
    assert_eq!(r1.status(), StatusCode::CREATED);
    let n1 = body_json(r1.into_body()).await;
    let n1_id = n1["id"].to_string().trim_matches('"').to_string();

    let r2 = router.clone().oneshot(json_post(
        &format!("/graphs/{graph}/nodes"), json!({"labels": ["B"], "properties": {"val": 2}}),
    )).await.unwrap();
    let n2 = body_json(r2.into_body()).await;
    let n2_id = n2["id"].to_string().trim_matches('"').to_string();

    // Create edge.
    let re = router.clone().oneshot(json_post(
        &format!("/graphs/{graph}/edges"),
        json!({"source": n1_id, "target": n2_id, "type": "KNOWS", "properties": {"since": 2024}}),
    )).await.unwrap();
    assert_eq!(re.status(), StatusCode::CREATED);

    // Query relationship.
    let rq = router.clone().oneshot(json_post(
        &format!("/graphs/{graph}/query"),
        json!({"cypher": "MATCH (a:A)-[r:KNOWS]->(b:B) RETURN a.val, r.since, b.val"}),
    )).await.unwrap();
    assert_eq!(rq.status(), StatusCode::OK);
    let qbody = body_json(rq.into_body()).await;
    assert_eq!(qbody["rows"].as_array().unwrap().len(), 1);

    // Update node.
    let ru = router.clone().oneshot(json_put(
        &format!("/graphs/{graph}/nodes/{n1_id}"), json!({"properties": {"val": 99}}),
    )).await.unwrap();
    assert_eq!(ru.status(), StatusCode::OK);

    // Verify updated value via query.
    let rv = router.clone().oneshot(json_post(
        &format!("/graphs/{graph}/query"), json!({"cypher": "MATCH (a:A) RETURN a.val"}),
    )).await.unwrap();
    assert_eq!(rv.status(), StatusCode::OK);
    let vbody = body_json(rv.into_body()).await;
    let rows = vbody["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 1, "expected 1 row after update");
    // The value 99 should appear somewhere in the first row.
    let row_str = rows[0].to_string();
    assert!(row_str.contains("99"), "expected 99 in row, got: {row_str}");

    // Delete node (should require detach since edge exists).
    let rd = router.clone().oneshot(
        Request::builder().method(Method::DELETE)
            .uri(&format!("/graphs/{graph}/nodes/{n1_id}?detach=true"))
            .body(Body::empty()).unwrap()
    ).await.unwrap();
    assert_eq!(rd.status(), StatusCode::NO_CONTENT);
}

// ===========================================================================
// PROFILE endpoint
// ===========================================================================

#[tokio::test]
async fn profile_returns_results_and_plan_with_row_counts() {
    let graph = "profile_basic";
    let router = app_with_graph(graph);

    // Create some nodes first.
    let mutate_resp = router
        .clone()
        .oneshot(json_post(
            &format!("/graphs/{graph}/mutate"),
            json!({"cypher": "CREATE (:Person {name: 'Alice'}), (:Person {name: 'Bob'}), (:Person {name: 'Charlie'})"}),
        ))
        .await
        .unwrap();
    assert_eq!(mutate_resp.status(), StatusCode::OK);

    // Now PROFILE a MATCH query.
    let resp = router
        .clone()
        .oneshot(json_post(
            &format!("/graphs/{graph}/profile"),
            json!({"cypher": "MATCH (n:Person) RETURN n.name AS name"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = body_json(resp.into_body()).await;

    // Should have columns
    let columns = body["columns"].as_array().unwrap();
    assert_eq!(columns.len(), 1);
    assert_eq!(columns[0], "name");

    // Should have 3 result rows
    let rows = body["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 3);

    // Should have a plan with row counts
    let plan = &body["plan"];
    assert!(plan.is_object(), "plan should be an object");
    assert!(plan["operatorType"].is_string());

    // Root operator should have rows field with non-zero value
    let root_rows = plan["rows"].as_u64().unwrap();
    assert_eq!(root_rows, 3, "root operator should have 3 rows");

    // Should have dbHits field
    assert!(plan.get("dbHits").is_some(), "plan should have dbHits field");

    // Should have estimatedRows
    assert!(
        plan.get("estimatedRows").is_some(),
        "plan should have estimatedRows"
    );

    // Should have elapsed_ms
    assert!(body["elapsedMs"].is_number());
}

#[tokio::test]
async fn profile_plan_has_children_with_row_counts() {
    let graph = "profile_children";
    let router = app_with_graph(graph);

    // Create nodes.
    router
        .clone()
        .oneshot(json_post(
            &format!("/graphs/{graph}/mutate"),
            json!({"cypher": "CREATE (:Item {val: 1}), (:Item {val: 2})"}),
        ))
        .await
        .unwrap();

    let resp = router
        .clone()
        .oneshot(json_post(
            &format!("/graphs/{graph}/profile"),
            json!({"cypher": "MATCH (n:Item) RETURN n"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = body_json(resp.into_body()).await;
    let plan = &body["plan"];

    // Plan should have children
    let children = plan["children"].as_array().unwrap();
    assert!(
        !children.is_empty(),
        "root plan node should have at least one child"
    );

    // Each child should have rows field
    for child in children {
        assert!(
            child.get("rows").is_some(),
            "child operator should have rows field"
        );
        assert!(
            child.get("dbHits").is_some(),
            "child operator should have dbHits field"
        );
    }
}

#[tokio::test]
async fn profile_nonexistent_graph_returns_error() {
    let resp = app()
        .oneshot(json_post(
            "/graphs/no_such_graph/profile",
            json!({"cypher": "MATCH (n) RETURN n"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn profile_invalid_query_returns_400() {
    let graph = "profile_invalid";
    let resp = app_with_graph(graph)
        .oneshot(json_post(
            &format!("/graphs/{graph}/profile"),
            json!({"cypher": "NOT VALID CYPHER !!!"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
