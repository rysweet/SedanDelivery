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

use std::sync::{OnceLock, RwLock};
use std::time::{Duration, Instant};

use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use jsonwebtoken::{decode, decode_header, DecodingKey, TokenData, Validation};
use serde::{Deserialize, Serialize};

// ═════════════════════════════════════════════════════════════════════════════
// Entra ID configuration
// ═════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct EntraConfig {
    pub tenant_id: String,
    pub client_id: String,
}

impl EntraConfig {
    fn from_env() -> Option<Self> {
        let tenant_id = std::env::var("GQL_API_ENTRA_TENANT_ID").ok()?;
        let client_id = std::env::var("GQL_API_ENTRA_CLIENT_ID").ok()?;
        Some(Self {
            tenant_id,
            client_id,
        })
    }
}

pub fn entra_config() -> Option<&'static EntraConfig> {
    static CFG: OnceLock<Option<EntraConfig>> = OnceLock::new();
    CFG.get_or_init(EntraConfig::from_env).as_ref()
}

// ═════════════════════════════════════════════════════════════════════════════
// JWKS cache with TTL refresh and unknown-kid re-fetch
// ═════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Deserialize, Clone)]
struct JwksKey {
    kid: String,
    n: String,
    e: String,
    kty: String,
}

#[derive(Debug, Deserialize)]
struct JwksResponse {
    keys: Vec<JwksKey>,
}

struct JwksCacheInner {
    keys: Vec<JwksKey>,
    fetched_at: Instant,
    last_retry_at: Option<Instant>,
}

struct JwksCache {
    inner: RwLock<Option<JwksCacheInner>>,
    refresh_ttl: Duration,
    retry_cooldown: Duration,
}

fn env_duration_secs(name: &str, default: u64) -> Duration {
    Duration::from_secs(
        std::env::var(name)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default),
    )
}

impl JwksCache {
    fn new() -> Self {
        Self {
            inner: RwLock::new(None),
            refresh_ttl: env_duration_secs("GQL_API_JWKS_REFRESH_SECS", 86400),
            retry_cooldown: env_duration_secs("GQL_API_JWKS_RETRY_COOLDOWN_SECS", 60),
        }
    }
}

fn fetch_jwks(tenant_id: &str) -> Result<Vec<JwksKey>, String> {
    let url = format!("https://login.microsoftonline.com/{tenant_id}/discovery/v2.0/keys");
    tracing::info!("fetching Entra ID JWKS from {url}");
    let mut resp = ureq::get(&url)
        .call()
        .map_err(|e| format!("failed to fetch JWKS from {url}: {e}"))?;
    let jwks: JwksResponse = resp
        .body_mut()
        .read_json()
        .map_err(|e| format!("failed to parse JWKS response from {url}: {e}"))?;
    if jwks.keys.is_empty() {
        return Err(format!("JWKS response from {url} contains no keys"));
    }
    tracing::info!("loaded {} JWKS keys", jwks.keys.len());
    Ok(jwks.keys)
}

static JWKS_CACHE: OnceLock<JwksCache> = OnceLock::new();

fn jwks_cache() -> &'static JwksCache {
    JWKS_CACHE.get_or_init(JwksCache::new)
}

/// Look up a key by `kid`, refreshing the cache when needed.
///
/// Returns `Err(String)` if the key cannot be found after a refresh attempt.
fn find_jwks_key(tenant_id: &str, kid: &str) -> Result<JwksKey, String> {
    let cache = jwks_cache();
    let now = Instant::now();

    // Fast path: read lock — check if we have a non-expired key.
    {
        let guard = cache
            .inner
            .read()
            .map_err(|_| "JWKS cache lock poisoned".to_string())?;
        if let Some(ref inner) = *guard {
            if now.duration_since(inner.fetched_at) < cache.refresh_ttl {
                if let Some(key) = inner.keys.iter().find(|k| k.kid == kid && k.kty == "RSA") {
                    return Ok(key.clone());
                }
            }
        }
    }

    // Slow path: write lock — refresh keys.
    let mut guard = cache
        .inner
        .write()
        .map_err(|_| "JWKS cache lock poisoned".to_string())?;

    // Double-check: another thread may have refreshed while we waited.
    if let Some(ref inner) = *guard {
        if now.duration_since(inner.fetched_at) < cache.refresh_ttl {
            if let Some(key) = inner.keys.iter().find(|k| k.kid == kid && k.kty == "RSA") {
                return Ok(key.clone());
            }
            // Key not found — check retry cooldown before re-fetching.
            if let Some(last) = inner.last_retry_at {
                if now.duration_since(last) < cache.retry_cooldown {
                    return Err(format!(
                        "no matching JWKS key for kid={kid} (retry cooldown)"
                    ));
                }
            }
        }
    }

    // Fetch fresh keys.
    match fetch_jwks(tenant_id) {
        Ok(keys) => {
            let result = keys
                .iter()
                .find(|k| k.kid == kid && k.kty == "RSA")
                .cloned();
            *guard = Some(JwksCacheInner {
                keys,
                fetched_at: now,
                last_retry_at: Some(now),
            });
            result.ok_or_else(|| format!("no matching JWKS key for kid={kid}"))
        }
        Err(e) => {
            crate::metrics::jwks_refresh_failure();
            // If we have stale keys, update the retry timestamp and try them.
            if let Some(ref mut inner) = *guard {
                inner.last_retry_at = Some(now);
                if let Some(key) = inner.keys.iter().find(|k| k.kid == kid && k.kty == "RSA") {
                    tracing::warn!("JWKS refresh failed ({e}), using stale key for kid={kid}");
                    return Ok(key.clone());
                }
            }
            Err(format!(
                "JWKS fetch failed and no cached key for kid={kid}: {e}"
            ))
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Token validation
// ═════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Serialize, Deserialize)]
pub struct EntraClaims {
    pub aud: String,
    pub iss: String,
    pub sub: String,
    #[serde(default)]
    pub oid: String,
    #[serde(default)]
    pub preferred_username: String,
    pub exp: usize,
    pub iat: usize,
}

fn validate_entra_token(
    token: &str,
    config: &EntraConfig,
) -> Result<TokenData<EntraClaims>, (StatusCode, String)> {
    let header = decode_header(token)
        .map_err(|e| (StatusCode::UNAUTHORIZED, format!("invalid JWT header: {e}")))?;

    let kid = header.kid.as_deref().ok_or_else(|| {
        (
            StatusCode::UNAUTHORIZED,
            "JWT missing kid header".to_string(),
        )
    })?;

    let jwk = find_jwks_key(&config.tenant_id, kid).map_err(|e| {
        if e.contains("JWKS fetch failed") || e.contains("lock poisoned") {
            (StatusCode::SERVICE_UNAVAILABLE, e)
        } else {
            (StatusCode::UNAUTHORIZED, e)
        }
    })?;

    let decoding_key = DecodingKey::from_rsa_components(&jwk.n, &jwk.e).map_err(|e| {
        (
            StatusCode::UNAUTHORIZED,
            format!("invalid RSA key components: {e}"),
        )
    })?;

    let mut validation = Validation::new(header.alg);
    validation.set_audience(&[&config.client_id, &format!("api://{}", config.client_id)]);
    validation.set_issuer(&[
        &format!(
            "https://login.microsoftonline.com/{}/v2.0",
            config.tenant_id
        ),
        &format!("https://sts.windows.net/{}/", config.tenant_id),
    ]);

    decode::<EntraClaims>(token, &decoding_key, &validation).map_err(|e| {
        (
            StatusCode::UNAUTHORIZED,
            format!("JWT validation failed: {e}"),
        )
    })
}

// ═════════════════════════════════════════════════════════════════════════════
// Middleware
// ═════════════════════════════════════════════════════════════════════════════

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

    let auth_header = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    let token = match auth_header {
        Some(h) if h.starts_with("Bearer ") => &h[7..],
        _ => {
            crate::metrics::auth_rejection();
            return (
                StatusCode::UNAUTHORIZED,
                [("WWW-Authenticate", "Bearer")],
                "missing or invalid Authorization header",
            )
                .into_response();
        }
    };

    match validate_entra_token(token, config) {
        Ok(_claims) => next.run(request).await,
        Err((status, e)) => {
            tracing::warn!("auth rejected: {e}");
            crate::metrics::auth_rejection();
            let challenge = if status == StatusCode::SERVICE_UNAVAILABLE {
                "Bearer"
            } else {
                "Bearer error=\"invalid_token\""
            };
            (
                status,
                [("WWW-Authenticate", challenge)],
                "authentication failed",
            )
                .into_response()
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Tests
// ═════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entra_config_from_env_returns_some_or_none() {
        let cfg = EntraConfig::from_env();
        if std::env::var("GQL_API_ENTRA_TENANT_ID").is_ok()
            && std::env::var("GQL_API_ENTRA_CLIENT_ID").is_ok()
        {
            assert!(cfg.is_some(), "expected Some when both env vars are set");
        } else {
            assert!(cfg.is_none(), "expected None when env vars are missing");
        }
    }

    #[test]
    fn validate_entra_token_rejects_garbage() {
        let config = EntraConfig {
            tenant_id: "fake-tenant".to_string(),
            client_id: "fake-client".to_string(),
        };
        let result = validate_entra_token("not.a.jwt", &config);
        assert!(result.is_err());
    }

    #[test]
    fn env_duration_secs_uses_default() {
        let d = env_duration_secs("__NONEXISTENT_ENV_VAR__", 42);
        assert_eq!(d, Duration::from_secs(42));
    }

    #[test]
    fn jwks_cache_initializes() {
        let cache = jwks_cache();
        let guard = cache.inner.read().unwrap();
        assert!(guard.is_none(), "fresh cache should be empty");
    }

    #[test]
    fn test_validate_token_rejects_garbage() {
        let config = EntraConfig {
            tenant_id: "test-tenant".to_string(),
            client_id: "test-client".to_string(),
        };
        let result = validate_entra_token("not.a.jwt", &config);
        assert!(result.is_err(), "garbage token must be rejected");
    }

    #[test]
    fn test_entra_config_none_without_env_vars() {
        // Save current values so we can restore them.
        let saved_tenant = std::env::var("GQL_API_ENTRA_TENANT_ID").ok();
        let saved_client = std::env::var("GQL_API_ENTRA_CLIENT_ID").ok();

        std::env::remove_var("GQL_API_ENTRA_TENANT_ID");
        std::env::remove_var("GQL_API_ENTRA_CLIENT_ID");

        let cfg = EntraConfig::from_env();
        assert!(cfg.is_none(), "EntraConfig should be None without env vars");

        // Restore env vars if they were set.
        if let Some(v) = saved_tenant {
            std::env::set_var("GQL_API_ENTRA_TENANT_ID", v);
        }
        if let Some(v) = saved_client {
            std::env::set_var("GQL_API_ENTRA_CLIENT_ID", v);
        }
    }
}
