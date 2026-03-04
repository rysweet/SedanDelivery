//! GET /health

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};

use crate::models::HealthResponse;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/health", get(health_handler))
}

async fn health_handler(State(state): State<AppState>) -> Json<HealthResponse> {
    let demo_graph_exists = state
        .stores
        .lock()
        .map(|map| map.contains_key("demo"))
        .unwrap_or(false);

    Json(HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
        demo_graph_exists,
    })
}
