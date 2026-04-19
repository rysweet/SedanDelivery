//! Health and metrics HTTP endpoint for sedandelivery-bolt.
//!
//! Spawns a lightweight axum HTTP server on a separate port (default 7688)
//! that exposes:
//!   - `GET /health`  -- JSON health status
//!   - `GET /metrics` -- Prometheus text-format metrics

mod handlers;
mod server;
mod state;

pub use server::{spawn_health_server, spawn_health_server_with_addr};
pub use state::HealthState;

#[cfg(test)]
mod tests;
