//! Cypher query and mutate endpoints.
//!
//! POST /graphs/{name}/query   — read queries
//! POST /graphs/{name}/mutate  — write queries
//! POST /graphs/{name}/explain — plan inspection
//! POST /graphs/{name}/profile — execution profiling

mod common;
mod explain;
mod mutate;
mod profile;
mod read;

use axum::extract::DefaultBodyLimit;
use axum::routing::post;
use axum::Router;

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/graphs/{name}/query", post(read::query_handler))
        .route("/graphs/{name}/mutate", post(mutate::mutate_handler))
        .route("/graphs/{name}/explain", post(explain::explain_handler))
        .route("/graphs/{name}/profile", post(profile::profile_handler))
        .layer(DefaultBodyLimit::max(common::query_body_limit()))
}
