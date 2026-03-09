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
            .and_then(|v| match v.parse::<u64>() {
                Ok(val) => Some(val),
                Err(_) => {
                    tracing::warn!(
                        "invalid non-numeric value for {name}: {v:?}, using default {default}"
                    );
                    None
                }
            })
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

/// Fetch JWKS keys from the Entra ID discovery endpoint.
///
/// The underlying HTTP call uses `ureq` (synchronous), so we offload it to
/// `tokio::task::spawn_blocking` to avoid stalling an async worker thread.
async fn fetch_jwks(tenant_id: &str) -> Result<Vec<JwksKey>, String> {
    let url = format!("https://login.microsoftonline.com/{tenant_id}/discovery/v2.0/keys");
    tracing::info!("fetching Entra ID JWKS from {url}");

    let url_clone = url.clone();
    let keys = tokio::task::spawn_blocking(move || {
        let mut resp = ureq::get(&url_clone)
            .call()
            .map_err(|e| format!("failed to fetch JWKS from {url_clone}: {e}"))?;
        let jwks: JwksResponse = resp
            .body_mut()
            .read_json()
            .map_err(|e| format!("failed to parse JWKS response from {url_clone}: {e}"))?;
        Ok::<Vec<JwksKey>, String>(jwks.keys)
    })
    .await
    .map_err(|e| format!("JWKS fetch task panicked: {e}"))??;

    if keys.is_empty() {
        return Err(format!("JWKS response from {url} contains no keys"));
    }
    tracing::info!("loaded {} JWKS keys", keys.len());
    Ok(keys)
}

static JWKS_CACHE: OnceLock<JwksCache> = OnceLock::new();

fn jwks_cache() -> &'static JwksCache {
    JWKS_CACHE.get_or_init(JwksCache::new)
}

/// Look up a key by `kid`, refreshing the cache when needed.
///
/// Returns `Err(String)` if the key cannot be found after a refresh attempt.
async fn find_jwks_key(tenant_id: &str, kid: &str) -> Result<JwksKey, String> {
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

    // Slow path: write lock — check again, then decide whether to fetch.
    {
        let guard = cache
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
        // Drop the write lock before the async fetch so we don't hold a
        // std::sync::RwLockWriteGuard (which is !Send) across an .await point.
    }

    // Fetch fresh keys outside any lock.
    match fetch_jwks(tenant_id).await {
        Ok(keys) => {
            let result = keys
                .iter()
                .find(|k| k.kid == kid && k.kty == "RSA")
                .cloned();
            // Re-acquire write lock to store the fetched keys.
            let mut guard = cache
                .inner
                .write()
                .map_err(|_| "JWKS cache lock poisoned".to_string())?;
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
            let mut guard = cache
                .inner
                .write()
                .map_err(|_| "JWKS cache lock poisoned".to_string())?;
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

async fn validate_entra_token(
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

    let jwk = find_jwks_key(&config.tenant_id, kid).await.map_err(|e| {
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

// ═════════════════════════════════════════════════════════════════════════════
// Tests
// ═════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    // ── EntraConfig ──────────────────────────────────────────────────────

    /// Regression: the old version of this test was tautological — it
    /// branched on the current env state and always asserted the matching
    /// arm, so it could never fail.  The replacement explicitly clears the
    /// env vars, asserts `None`, then sets them and asserts `Some`.
    ///
    /// WARNING: mutates process-wide env vars. Not safe to run in parallel
    /// with other tests that read these variables.
    #[test]
    fn entra_config_from_env_requires_both_vars() {
        let saved_tenant = std::env::var("GQL_API_ENTRA_TENANT_ID").ok();
        let saved_client = std::env::var("GQL_API_ENTRA_CLIENT_ID").ok();

        // Neither var set → None
        std::env::remove_var("GQL_API_ENTRA_TENANT_ID");
        std::env::remove_var("GQL_API_ENTRA_CLIENT_ID");
        assert!(
            EntraConfig::from_env().is_none(),
            "expected None when both env vars are missing"
        );

        // Only tenant → None
        std::env::set_var("GQL_API_ENTRA_TENANT_ID", "t");
        assert!(
            EntraConfig::from_env().is_none(),
            "expected None when only TENANT_ID is set"
        );
        std::env::remove_var("GQL_API_ENTRA_TENANT_ID");

        // Only client → None
        std::env::set_var("GQL_API_ENTRA_CLIENT_ID", "c");
        assert!(
            EntraConfig::from_env().is_none(),
            "expected None when only CLIENT_ID is set"
        );

        // Both set → Some
        std::env::set_var("GQL_API_ENTRA_TENANT_ID", "t");
        let cfg = EntraConfig::from_env();
        assert!(cfg.is_some(), "expected Some when both env vars are set");
        let cfg = cfg.unwrap();
        assert_eq!(cfg.tenant_id, "t");
        assert_eq!(cfg.client_id, "c");

        // Restore
        std::env::remove_var("GQL_API_ENTRA_TENANT_ID");
        std::env::remove_var("GQL_API_ENTRA_CLIENT_ID");
        if let Some(v) = saved_tenant {
            std::env::set_var("GQL_API_ENTRA_TENANT_ID", v);
        }
        if let Some(v) = saved_client {
            std::env::set_var("GQL_API_ENTRA_CLIENT_ID", v);
        }
    }

    #[test]
    fn env_duration_secs_uses_default() {
        let d = env_duration_secs("__NONEXISTENT_ENV_VAR__", 42);
        assert_eq!(d, Duration::from_secs(42));
    }

    // ── JwksCache ────────────────────────────────────────────────────────

    #[test]
    fn jwks_cache_initializes_empty() {
        let cache = JwksCache {
            inner: RwLock::new(None),
            refresh_ttl: Duration::from_secs(60),
            retry_cooldown: Duration::from_secs(5),
        };
        let guard = cache.inner.read().unwrap();
        assert!(guard.is_none(), "fresh cache should be empty");
    }

    #[test]
    fn jwks_cache_ttl_expires_entries() {
        let cache = JwksCache {
            inner: RwLock::new(None),
            refresh_ttl: Duration::from_secs(0), // immediate expiry
            retry_cooldown: Duration::from_secs(0),
        };

        let key = JwksKey {
            kid: "key-1".into(),
            n: "n-value".into(),
            e: "e-value".into(),
            kty: "RSA".into(),
        };

        // Populate the cache with a key fetched "now".
        {
            let mut guard = cache.inner.write().unwrap();
            *guard = Some(JwksCacheInner {
                keys: vec![key.clone()],
                fetched_at: Instant::now() - Duration::from_secs(1),
                last_retry_at: None,
            });
        }

        // Read back: the entry exists but its TTL has expired (refresh_ttl=0,
        // fetched_at is 1 s ago).  The find_jwks_key function would skip
        // expired entries on the fast path.  We verify the TTL check here.
        let guard = cache.inner.read().unwrap();
        let inner = guard.as_ref().unwrap();
        let elapsed = Instant::now().duration_since(inner.fetched_at);
        assert!(
            elapsed >= cache.refresh_ttl,
            "entry should be past its TTL: elapsed {elapsed:?} >= ttl {:?}",
            cache.refresh_ttl
        );
    }

    #[test]
    fn jwks_cache_cooldown_prevents_rapid_refetch() {
        let cache = JwksCache {
            inner: RwLock::new(None),
            refresh_ttl: Duration::from_secs(3600),
            retry_cooldown: Duration::from_secs(300), // 5 minutes
        };

        let now = Instant::now();

        // Populate cache with a valid (non-expired) entry that was retried
        // very recently.
        {
            let mut guard = cache.inner.write().unwrap();
            *guard = Some(JwksCacheInner {
                keys: vec![JwksKey {
                    kid: "other-key".into(),
                    n: "n".into(),
                    e: "e".into(),
                    kty: "RSA".into(),
                }],
                fetched_at: now,
                last_retry_at: Some(now), // just retried
            });
        }

        // Simulate the cooldown check that find_jwks_key performs when a
        // requested kid is not found in a non-expired cache.
        let guard = cache.inner.read().unwrap();
        let inner = guard.as_ref().unwrap();

        // The cache is not expired (fetched_at is now, ttl is 1 hour).
        assert!(
            Instant::now().duration_since(inner.fetched_at) < cache.refresh_ttl,
            "cache should still be valid"
        );

        // The requested kid="missing-kid" is not in keys.
        assert!(
            inner.keys.iter().find(|k| k.kid == "missing-kid").is_none(),
            "key should not be found"
        );

        // Cooldown check: last_retry_at is very recent, so elapsed < cooldown.
        let last = inner.last_retry_at.unwrap();
        let elapsed_since_retry = Instant::now().duration_since(last);
        assert!(
            elapsed_since_retry < cache.retry_cooldown,
            "cooldown should block re-fetch: elapsed {elapsed_since_retry:?} < cooldown {:?}",
            cache.retry_cooldown
        );
    }

    // ── Token validation ─────────────────────────────────────────────────

    #[tokio::test]
    async fn test_validate_token_rejects_garbage() {
        let config = EntraConfig {
            tenant_id: "test-tenant".to_string(),
            client_id: "test-client".to_string(),
        };
        let result = validate_entra_token("not.a.jwt", &config).await;
        assert!(result.is_err(), "garbage token must be rejected");
    }

    // ── Middleware (extract_and_validate) ─────────────────────────────────

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
