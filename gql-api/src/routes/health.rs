//! GET /health

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};

use crate::models::HealthResponse;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/health", get(health_handler))
}

/// Health check handler.
///
/// Uses `"__health_probe"` as a sentinel graph name to verify that the backing
/// store is reachable and responsive.  The name is deliberately invalid for
/// user-created graphs (leading underscores are reserved) so it will never
/// collide with real data.  We only care whether `graph_exists` succeeds or
/// returns an error — the boolean result itself is ignored.
async fn health_handler(State(state): State<AppState>) -> impl IntoResponse {
    match state.store.graph_exists("__health_probe") {
        Ok(_) => (
            StatusCode::OK,
            Json(HealthResponse {
                status: "ok",
                error: None,
            }),
        ),
        Err(_e) => {
            // Do NOT expose the backend error message — it may contain
            // connection strings, internal paths, or store implementation
            // details.  Log it server-side for operators instead.
            tracing::error!("health probe failed: {_e}");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(HealthResponse {
                    status: "degraded",
                    error: Some("backend unreachable".to_string()),
                }),
            )
        }
    }
}
