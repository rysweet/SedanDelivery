//! Shared error type that converts to an Axum JSON response.
//!
//! The module is split into focused submodules:
//! - [`types`]: the [`ApiError`] struct, its constructors, [`ErrorBody`], and
//!   the `IntoResponse` impl.
//! - [`conversions`]: `From` impls that map `cypher_core` errors to
//!   [`ApiError`] with appropriate HTTP status codes.

mod conversions;
mod types;

pub use types::{ApiError, ErrorBody};
