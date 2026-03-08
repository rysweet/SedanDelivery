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

async fn health_handler(State(state): State<AppState>) -> impl IntoResponse {
    let entra_enabled = crate::auth::entra_config().is_some();
    match state.store.graph_exists("__health_probe") {
        Ok(_) => {
            let demo_graph_exists = if entra_enabled {
                Some(state.store.graph_exists("demo").unwrap_or(false))
            } else {
                None
            };
            (
                StatusCode::OK,
                Json(HealthResponse {
                    status: "ok",
                    version: env!("CARGO_PKG_VERSION"),
                    demo_graph_exists,
                    error: None,
                }),
            )
        }
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(HealthResponse {
                status: "degraded",
                version: env!("CARGO_PKG_VERSION"),
                demo_graph_exists: None,
                error: Some(e.to_string()),
            }),
        ),
    }
}
