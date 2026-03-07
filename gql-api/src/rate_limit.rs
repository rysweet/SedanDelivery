//! Per-IP rate limiting middleware using the governor crate.
//!
//! Configurable via `GQL_API_RATE_LIMIT_RPS` env var (default: 100 requests/second per IP).
//! Returns HTTP 429 Too Many Requests when a client exceeds the limit.

use std::net::IpAddr;
use std::num::NonZeroU32;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use governor::clock::DefaultClock;
use governor::state::keyed::DashMapStateStore;
use governor::{Quota, RateLimiter};

pub type KeyedRateLimiter = RateLimiter<IpAddr, DashMapStateStore<IpAddr>, DefaultClock>;

/// Read the configured rate limit from the environment.
pub fn rps_from_env() -> NonZeroU32 {
    std::env::var("GQL_API_RATE_LIMIT_RPS")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .and_then(NonZeroU32::new)
        .unwrap_or(NonZeroU32::new(100).unwrap())
}

/// Build a keyed rate limiter for the given requests-per-second quota.
pub fn build_limiter(rps: NonZeroU32) -> Arc<KeyedRateLimiter> {
    let quota = Quota::per_second(rps);
    Arc::new(RateLimiter::dashmap(quota))
}

/// Axum middleware that enforces per-IP rate limiting.
///
/// Requests to `/health` are exempt.
pub async fn rate_limit_middleware(
    axum::Extension(limiter): axum::Extension<Arc<KeyedRateLimiter>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    // Exempt /health
    if request.uri().path() == "/health" {
        return next.run(request).await;
    }

    // Extract client IP from ConnectInfo if available, otherwise default to loopback.
    let ip = request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|ci| ci.0.ip())
        .unwrap_or(IpAddr::from([127, 0, 0, 1]));

    match limiter.check_key(&ip) {
        Ok(_) => next.run(request).await,
        Err(_) => (StatusCode::TOO_MANY_REQUESTS, "Too Many Requests").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get;
    use axum::{middleware, Router};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn test_app(rps: u32) -> Router {
        let limiter = build_limiter(NonZeroU32::new(rps).unwrap());

        Router::new()
            .route("/health", get(|| async { "ok" }))
            .route("/graphql", get(|| async { "data" }))
            .layer(middleware::from_fn(rate_limit_middleware))
            .layer(axum::Extension(limiter))
    }

    #[tokio::test]
    async fn allows_requests_under_limit() {
        let app = test_app(10);
        let req = Request::builder()
            .uri("/graphql")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn returns_429_when_limit_exceeded() {
        let app = test_app(1);

        // First request should succeed
        let req = Request::builder()
            .uri("/graphql")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Second request should be rate limited
        let req = Request::builder()
            .uri("/graphql")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn health_endpoint_exempt_from_rate_limit() {
        let app = test_app(1);

        // Exhaust the limit
        let req = Request::builder()
            .uri("/graphql")
            .body(Body::empty())
            .unwrap();
        let _ = app.clone().oneshot(req).await.unwrap();

        // /health should still work even after limit exhausted
        let req = Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"ok");
    }
}
