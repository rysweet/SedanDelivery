//! Entra ID (Azure AD) JWT validation middleware with JWKS TTL cache.
//!
//! When `GQL_API_ENTRA_TENANT_ID` and `GQL_API_ENTRA_CLIENT_ID` are set,
//! all requests (except `GET /health`) must carry a valid
//! `Authorization: Bearer <token>` header signed by the configured Entra ID
//! tenant.
//!
//! When these env vars are absent the middleware is a no-op, allowing
//! unauthenticated local development.
//!
//! Keys are cached in an `RwLock<(Vec<JwksKey>, Instant)>` and refreshed when
//! older than `GQL_API_JWKS_REFRESH_SECS` (default 86 400 s = 24 h).

use std::sync::OnceLock;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, TokenData, Validation};
use serde::{Deserialize, Serialize};

// ── Configuration ────────────────────────────────────────────────────────────

/// Entra ID auth configuration, lazily loaded from env vars.
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

/// Returns the Entra config if both env vars are set.
pub fn entra_config() -> Option<&'static EntraConfig> {
    static CFG: OnceLock<Option<EntraConfig>> = OnceLock::new();
    CFG.get_or_init(EntraConfig::from_env).as_ref()
}

// ── Legacy JWKS cache (OnceLock) ────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct LegacyJwksKey {
    kid: String,
    n: String,
    e: String,
    kty: String,
}

#[derive(Debug, Deserialize)]
struct JwksResponse {
    keys: Vec<LegacyJwksKey>,
}

/// Fetch JWKS keys from the Entra OIDC endpoint.
///
/// Keys are cached in a `OnceLock` on success. On failure, the function
/// **panics** rather than caching an empty set — a permanently empty JWKS
/// cache would silently reject all tokens for the process lifetime.
fn jwks_keys(tenant_id: &str) -> &'static Vec<LegacyJwksKey> {
    static KEYS: OnceLock<Vec<LegacyJwksKey>> = OnceLock::new();
    KEYS.get_or_init(|| {
        let url = format!("https://login.microsoftonline.com/{tenant_id}/discovery/v2.0/keys");
        tracing::info!("fetching Entra ID JWKS from {url}");
        let mut resp = ureq::get(&url)
            .call()
            .unwrap_or_else(|e| panic!("failed to fetch JWKS from {url}: {e}"));
        let jwks: JwksResponse = resp
            .body_mut()
            .read_json()
            .unwrap_or_else(|e| panic!("failed to parse JWKS response from {url}: {e}"));
        if jwks.keys.is_empty() {
            panic!("JWKS response from {url} contains no keys");
        }
        tracing::info!("loaded {} JWKS keys", jwks.keys.len());
        jwks.keys
    })
}

// ── JWT claims ───────────────────────────────────────────────────────────────

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

// ── Token validation ─────────────────────────────────────────────────────────

fn validate_token(token: &str, config: &EntraConfig) -> Result<TokenData<EntraClaims>, String> {
    let header = decode_header(token).map_err(|e| format!("invalid JWT header: {e}"))?;

    let kid = header.kid.as_deref().ok_or("JWT missing kid header")?;

    let keys = jwks_keys(&config.tenant_id);
    let jwk = keys
        .iter()
        .find(|k| k.kid == kid && k.kty == "RSA")
        .ok_or_else(|| format!("no matching JWKS key for kid={kid}"))?;

    let decoding_key = DecodingKey::from_rsa_components(&jwk.n, &jwk.e)
        .map_err(|e| format!("invalid RSA key components: {e}"))?;

    let mut validation = Validation::new(Algorithm::RS256);
    // Entra ID uses both v1 (`api://<client_id>`) and v2 (`<client_id>`) audience formats.
    validation.set_audience(&[&config.client_id, &format!("api://{}", config.client_id)]);
    // Accept both v1 and v2 Entra issuers.
    validation.set_issuer(&[
        &format!(
            "https://login.microsoftonline.com/{}/v2.0",
            config.tenant_id
        ),
        &format!("https://sts.windows.net/{}/", config.tenant_id),
    ]);

    decode::<EntraClaims>(token, &decoding_key, &validation)
        .map_err(|e| format!("JWT validation failed: {e}"))
}

// ── Axum middleware ──────────────────────────────────────────────────────────

/// Axum middleware that enforces Entra ID JWT auth.
///
/// Skips auth for `GET /health` and when Entra config is not set.
pub async fn require_entra_auth(request: Request, next: Next) -> Response {
    let config = match entra_config() {
        Some(c) => c,
        None => return next.run(request).await,
    };

    // Always allow health checks without auth.
    if request.uri().path() == "/health" {
        return next.run(request).await;
    }

    let auth_header = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    let token = match auth_header {
        Some(h) if h.starts_with("Bearer ") => &h[7..],
        _ => {
            return (
                StatusCode::UNAUTHORIZED,
                [("WWW-Authenticate", "Bearer")],
                "missing or invalid Authorization header",
            )
                .into_response();
        }
    };

    match validate_token(token, config) {
        Ok(_claims) => next.run(request).await,
        Err(e) => {
            tracing::warn!("auth rejected: {e}");
            (
                StatusCode::UNAUTHORIZED,
                [("WWW-Authenticate", "Bearer error=\"invalid_token\"")],
                "authentication failed",
            )
                .into_response()
        }
    }
}

// ── JWKS TTL cache (RwLock-based) ───────────────────────────────────────────

/// Default TTL for cached JWKS keys: 24 hours.
const DEFAULT_JWKS_REFRESH_SECS: u64 = 86_400;

/// A single JSON Web Key from a JWKS endpoint.
#[derive(Debug, Clone, PartialEq)]
pub struct JwksKey {
    pub kid: String,
    pub n: String,
    pub e: String,
}

/// Fetches JWKS keys from a remote endpoint.
///
/// In production this performs an HTTP GET; tests inject a custom fetcher.
pub type JwksFetcher = Box<dyn Fn() -> Result<Vec<JwksKey>, String> + Send + Sync>;

/// Thread-safe JWKS key cache with TTL-based refresh.
pub struct JwksCache {
    /// Cached keys and the instant they were fetched.
    inner: RwLock<(Vec<JwksKey>, Instant)>,
    /// How long keys are considered fresh.
    ttl: Duration,
    /// Function that retrieves fresh keys.
    fetcher: JwksFetcher,
}

impl JwksCache {
    /// Creates a new cache, immediately fetching keys via `fetcher`.
    ///
    /// `ttl` is read from `GQL_API_JWKS_REFRESH_SECS` (falls back to 86 400).
    pub fn new(fetcher: JwksFetcher) -> Result<Self, String> {
        let ttl = jwks_ttl_from_env();
        let keys = fetcher()?;
        Ok(Self {
            inner: RwLock::new((keys, Instant::now())),
            ttl,
            fetcher,
        })
    }

    /// Creates a cache with an explicit TTL (useful for tests).
    pub fn with_ttl(fetcher: JwksFetcher, ttl: Duration) -> Result<Self, String> {
        let keys = fetcher()?;
        Ok(Self {
            inner: RwLock::new((keys, Instant::now())),
            ttl,
            fetcher,
        })
    }

    /// Returns a snapshot of the current keys, refreshing first if stale.
    pub fn keys(&self) -> Result<Vec<JwksKey>, String> {
        // Fast path: read lock — check freshness.
        {
            let guard = self.inner.read().map_err(|e| format!("read lock: {e}"))?;
            if guard.1.elapsed() < self.ttl {
                return Ok(guard.0.clone());
            }
        }
        // Slow path: write lock — re-check then refresh.
        {
            let mut guard = self.inner.write().map_err(|e| format!("write lock: {e}"))?;
            // Another thread may have refreshed while we waited.
            if guard.1.elapsed() < self.ttl {
                return Ok(guard.0.clone());
            }
            let fresh = (self.fetcher)()?;
            *guard = (fresh.clone(), Instant::now());
            Ok(fresh)
        }
    }

    /// Returns `true` if the cached keys are older than the TTL.
    pub fn is_stale(&self) -> bool {
        self.inner
            .read()
            .map(|g| g.1.elapsed() >= self.ttl)
            .unwrap_or(true)
    }
}

/// Reads `GQL_API_JWKS_REFRESH_SECS` from the environment.
fn jwks_ttl_from_env() -> Duration {
    let secs = std::env::var("GQL_API_JWKS_REFRESH_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_JWKS_REFRESH_SECS);
    Duration::from_secs(secs)
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

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
    fn validate_token_rejects_garbage() {
        let config = EntraConfig {
            tenant_id: "fake-tenant".to_string(),
            client_id: "fake-client".to_string(),
        };
        let result = validate_token("not.a.jwt", &config);
        assert!(result.is_err());
    }

    fn sample_keys() -> Vec<JwksKey> {
        vec![JwksKey {
            kid: "key-1".into(),
            n: "modulus".into(),
            e: "AQAB".into(),
        }]
    }

    #[test]
    fn fresh_cache_returns_keys_without_refetch() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let cc = Arc::clone(&call_count);

        let cache = JwksCache::with_ttl(
            Box::new(move || {
                cc.fetch_add(1, Ordering::SeqCst);
                Ok(sample_keys())
            }),
            Duration::from_secs(3600),
        )
        .unwrap();

        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        let keys = cache.keys().unwrap();
        assert_eq!(keys, sample_keys());
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn stale_cache_triggers_refetch() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let cc = Arc::clone(&call_count);

        let cache = JwksCache::with_ttl(
            Box::new(move || {
                cc.fetch_add(1, Ordering::SeqCst);
                Ok(sample_keys())
            }),
            Duration::from_secs(0),
        )
        .unwrap();

        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        let _ = cache.keys().unwrap();
        assert_eq!(call_count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn is_stale_reflects_ttl() {
        let cache =
            JwksCache::with_ttl(Box::new(|| Ok(sample_keys())), Duration::from_secs(3600)).unwrap();

        assert!(!cache.is_stale());

        // Zero-TTL cache is immediately stale.
        let stale_cache =
            JwksCache::with_ttl(Box::new(|| Ok(sample_keys())), Duration::from_secs(0)).unwrap();

        assert!(stale_cache.is_stale());
    }

    #[test]
    fn fetcher_error_propagates() {
        let result = JwksCache::with_ttl(
            Box::new(|| Err("network error".into())),
            Duration::from_secs(60),
        );

        let err = result.err().expect("should be Err");
        assert!(err.contains("network error"));
    }

    #[test]
    fn default_ttl_is_24_hours() {
        std::env::remove_var("GQL_API_JWKS_REFRESH_SECS");
        let ttl = jwks_ttl_from_env();
        assert_eq!(ttl, Duration::from_secs(86_400));
    }

    #[test]
    fn env_var_overrides_ttl() {
        std::env::set_var("GQL_API_JWKS_REFRESH_SECS", "300");
        let ttl = jwks_ttl_from_env();
        assert_eq!(ttl, Duration::from_secs(300));
        std::env::remove_var("GQL_API_JWKS_REFRESH_SECS");
    }
}
