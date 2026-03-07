//! gql-api: HTTP REST API for the SedanDelivery graph engine.
//!
//! This library crate contains all API logic (routes, models, serialization,
//! error handling). The binary crate (`main.rs`) handles server startup only.

pub mod auth;
pub mod error;
pub mod metrics;
pub mod models;
pub mod rate_limit;
pub mod routes;
pub mod serialize;
pub mod state;
