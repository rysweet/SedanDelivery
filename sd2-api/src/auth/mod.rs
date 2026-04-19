//! Entra ID JWT authentication middleware.
//!
//! When `GQL_API_ENTRA_TENANT_ID` and `GQL_API_ENTRA_CLIENT_ID` are set,
//! all requests (except `GET /health`) must carry a valid
//! `Authorization: Bearer <token>` header signed by the configured Entra ID
//! tenant.  When absent the middleware is a no-op.
//!
//! JWKS keys are cached with a configurable TTL (default 24 h) and re-fetched
//! on unknown `kid` with a cooldown (default 60 s) to avoid hammering the
//! identity provider.

mod claims;
mod config;
mod jwks;
mod token;

use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

pub use claims::EntraClaims;
pub use config::{entra_config, EntraConfig};

use token::validate_entra_token;

/// Axum middleware that enforces Entra ID JWT auth.
///
/// Skips auth for `GET /health` and when Entra config is not set.
pub async fn require_entra_auth(request: Request, next: Next) -> Response {
    let config = match entra_config() {
        Some(c) => c,
        None => return next.run(request).await,
    };

    // /health and /metrics are exempt: health for liveness probes,
    // /metrics for Prometheus scraping (typically from a pod-network sidecar).
    let path = request.uri().path();
    if path == "/health" || path == "/metrics" {
        return next.run(request).await;
    }

    match extract_and_validate(request.headers(), config).await {
        Ok(()) => next.run(request).await,
        Err(resp) => resp,
    }
}

/// Core auth-check logic, factored out so it can be unit-tested without
/// requiring a running `Next` handler or global Entra config.
async fn extract_and_validate(
    headers: &axum::http::HeaderMap,
    config: &EntraConfig,
) -> Result<(), Response> {
    let auth_header = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    let token = match auth_header {
        Some(h) if h.starts_with("Bearer ") => &h[7..],
        _ => {
            crate::metrics::auth_rejection();
            return Err((
                StatusCode::UNAUTHORIZED,
                [("WWW-Authenticate", "Bearer")],
                "missing or invalid Authorization header",
            )
                .into_response());
        }
    };

    match validate_entra_token(token, config).await {
        Ok(_claims) => Ok(()),
        Err((status, e)) => {
            tracing::warn!("auth rejected: {e}");
            crate::metrics::auth_rejection();
            let challenge = if status == StatusCode::SERVICE_UNAVAILABLE {
                "Bearer"
            } else {
                "Bearer error=\"invalid_token\""
            };
            Err((
                status,
                [("WWW-Authenticate", challenge)],
                "authentication failed",
            )
                .into_response())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn middleware_rejects_request_without_auth_header() {
        let config = EntraConfig {
            tenant_id: "test-tenant".into(),
            client_id: "test-client".into(),
        };

        let headers = axum::http::HeaderMap::new();
        let result = extract_and_validate(&headers, &config).await;
        assert!(result.is_err(), "request without Authorization must fail");
        let resp = result.unwrap_err();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn middleware_rejects_non_bearer_auth() {
        let config = EntraConfig {
            tenant_id: "test-tenant".into(),
            client_id: "test-client".into(),
        };

        let mut headers = axum::http::HeaderMap::new();
        headers.insert(header::AUTHORIZATION, "Basic dXNlcjpwYXNz".parse().unwrap());
        let result = extract_and_validate(&headers, &config).await;
        assert!(result.is_err(), "non-Bearer auth must be rejected");
        assert_eq!(result.unwrap_err().status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn middleware_rejects_invalid_bearer_token() {
        let config = EntraConfig {
            tenant_id: "test-tenant".into(),
            client_id: "test-client".into(),
        };

        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            "Bearer not.a.valid.jwt".parse().unwrap(),
        );
        let result = extract_and_validate(&headers, &config).await;
        assert!(result.is_err(), "invalid JWT must be rejected");
        assert_eq!(result.unwrap_err().status(), StatusCode::UNAUTHORIZED);
    }
}
