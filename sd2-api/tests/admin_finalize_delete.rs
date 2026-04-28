//! TDD tests for the cluster-consistency hardening admin endpoint.
//!
//! Issue #1769 (F4): operator runbook + admin endpoint for stuck
//! `G:{g}:_deleting` markers.
//!
//!   POST /admin/graphs/{name}/finalize-delete?confirm=true
//!
//! These tests pin the contract for the new route. They will fail against
//! a router that does not yet expose the route (current state — should
//! produce 404 for the route below until `routes::graphs::router` is updated).
//!
//! Behaviors locked in here:
//!   1. Missing `?confirm=true` → 400 Bad Request (operator confirmation
//!      is mandatory; no body, no surprise side-effects).
//!   2. Non-Redis backend → 501 Not Implemented (mirrors the existing
//!      `/audit` route's behavior on `InMemoryGraphStore`).
//!   3. The route must be reachable (i.e. not 404) — proves it was wired in.

use std::sync::Arc;

use axum::body::Body;
use axum::Router;
use cypher_core::storage::{GraphStore, InMemoryGraphStore};
use http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

use sd2_api::routes;
use sd2_api::state::AppState;

fn app_with_graph(graph_name: &str) -> Router {
    let store = Arc::new(InMemoryGraphStore::new());
    store.create_graph(graph_name).expect("seed graph");
    let state = AppState::new(store);
    Router::new()
        .merge(routes::health::router())
        .merge(routes::graphs::router())
        .with_state(state)
}

async fn body_json(body: Body) -> Value {
    let bytes = body.collect().await.expect("collect body").to_bytes();
    if bytes.is_empty() {
        return Value::Null;
    }
    serde_json::from_slice(&bytes).expect("response body must be valid JSON")
}

#[tokio::test]
async fn finalize_delete_route_requires_confirm_query_parameter() {
    let graph = "finalize_needs_confirm";
    let router = app_with_graph(graph);

    let resp = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!("/admin/graphs/{graph}/finalize-delete"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_ne!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "the finalize-delete route must be wired in routes::graphs::router"
    );
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "missing ?confirm=true must be rejected with 400 — operator confirmation is mandatory"
    );

    let body = body_json(resp.into_body()).await;
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|m| m.contains("confirm")),
        "error body should explain that confirm=true is required: {body:?}"
    );
}

#[tokio::test]
async fn finalize_delete_route_returns_not_implemented_for_non_redis_store() {
    let graph = "finalize_not_impl_non_redis";
    let router = app_with_graph(graph);

    let resp = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/admin/graphs/{graph}/finalize-delete?confirm=true"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_ne!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "the finalize-delete route must be wired in routes::graphs::router"
    );
    assert_eq!(
        resp.status(),
        StatusCode::NOT_IMPLEMENTED,
        "non-Redis backends must return 501 just like /admin/graphs/{{name}}/audit"
    );

    let body = body_json(resp.into_body()).await;
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|msg| msg.contains("Redis") || msg.contains("unsupported")),
        "501 body must explain why the route is unavailable: {body:?}"
    );
}
