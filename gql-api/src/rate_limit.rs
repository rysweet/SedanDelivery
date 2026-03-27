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

use crate::tls::TlsConnectInfo;

pub type KeyedRateLimiter = RateLimiter<IpAddr, DashMapStateStore<IpAddr>, DefaultClock>;

/// Read the configured rate limit from the environment.
pub fn rps_from_env() -> NonZeroU32 {
    std::env::var("GQL_API_RATE_LIMIT_RPS")
        .ok()
        .and_then(|v| match v.parse::<u32>() {
            Ok(val) => NonZeroU32::new(val).or_else(|| {
                tracing::warn!(
                    "GQL_API_RATE_LIMIT_RPS=0 is invalid (must be >0), using default 100"
                );
                None
            }),
            Err(_) => {
                tracing::warn!(
                    "invalid value for GQL_API_RATE_LIMIT_RPS: {v:?}, using default 100"
                );
                None
            }
        })
        .unwrap_or(NonZeroU32::new(100).unwrap())
}

/// Build a keyed rate limiter for the given requests-per-second quota.
pub fn build_limiter(rps: NonZeroU32) -> Arc<KeyedRateLimiter> {
    let quota = Quota::per_second(rps);
    Arc::new(RateLimiter::dashmap(quota))
}

fn sweep_interval_secs() -> u64 {
    static INTERVAL: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *INTERVAL.get_or_init(|| {
        gql_core::config::parse_env_range("GQL_API_RATE_LIMIT_SWEEP_SECS", 300u64, 1, 86_400)
    })
}

pub fn spawn_limiter_sweeper(limiter: Arc<KeyedRateLimiter>) {
    let interval_secs = sweep_interval_secs();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
        loop {
            interval.tick().await;
            limiter.retain_recent();
            limiter.shrink_to_fit();
        }
    });
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

    // Extract client IP from ConnectInfo. If absent, rate limiting cannot function
    // correctly so we fail with 500 rather than silently allowing all traffic through.
    let ip = request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|ci| ci.0.ip())
        .or_else(|| {
            request
                .extensions()
                .get::<axum::extract::ConnectInfo<TlsConnectInfo>>()
                .map(|ci| (ci.0).0.ip())
        });

    let Some(ip) = ip else {
        tracing::error!("ConnectInfo missing — cannot identify client for rate limiting");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "server misconfiguration: unable to identify client",
        )
            .into_response();
    };

    match limiter.check_key(&ip) {
        Ok(_) => next.run(request).await,
        Err(_) => (StatusCode::TOO_MANY_REQUESTS, "Too Many Requests").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tls::TlsConnectInfo;
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

    /// Build a request with ConnectInfo set (required by the middleware).
    fn req_with_connect_info(uri: &str) -> Request<Body> {
        let addr: std::net::SocketAddr = "127.0.0.1:12345".parse().unwrap();
        let mut req = Request::builder().uri(uri).body(Body::empty()).unwrap();
        req.extensions_mut()
            .insert(axum::extract::ConnectInfo(addr));
        req
    }

    /// Build a request with TLS-mode ConnectInfo set.
    fn req_with_tls_connect_info(uri: &str) -> Request<Body> {
        let addr: std::net::SocketAddr = "127.0.0.1:12345".parse().unwrap();
        let mut req = Request::builder().uri(uri).body(Body::empty()).unwrap();
        req.extensions_mut()
            .insert(axum::extract::ConnectInfo(TlsConnectInfo(addr)));
        req
    }

    #[tokio::test]
    async fn allows_requests_under_limit() {
        let app = test_app(10);
        let req = req_with_connect_info("/graphql");
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn returns_429_when_limit_exceeded() {
        let app = test_app(1);

        // First request should succeed
        let req = req_with_connect_info("/graphql");
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Second request should be rate limited
        let req = req_with_connect_info("/graphql");
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn health_endpoint_exempt_from_rate_limit() {
        let app = test_app(1);

        // Exhaust the limit
        let req = req_with_connect_info("/graphql");
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

    #[tokio::test]
    async fn returns_500_when_connect_info_missing() {
        let app = test_app(10);
        // Request without ConnectInfo
        let req = Request::builder()
            .uri("/graphql")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn tls_connect_info_requests_are_rate_limited_instead_of_500ing() {
        let app = test_app(1);

        let resp = app
            .clone()
            .oneshot(req_with_tls_connect_info("/graphql"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let resp = app
            .oneshot(req_with_tls_connect_info("/graphql"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    }
}
