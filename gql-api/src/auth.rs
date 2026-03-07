//! JWT validation with automatic JWKS key re-fetch on unknown `kid`.
//!
//! ## Entra ID middleware
//! When `GQL_API_ENTRA_TENANT_ID` and `GQL_API_ENTRA_CLIENT_ID` are set,
//! all requests (except `GET /health`) must carry a valid
//! `Authorization: Bearer <token>` header signed by the configured Entra ID
//! tenant.  When absent the middleware is a no-op.
//!
//! ## Async JWKS cache with re-fetch
//! When a token presents a `kid` not found in the cached JWKS keyset,
//! a single re-fetch is attempted (subject to a cooldown) before rejecting.
//! This handles key rotation without restarts while preventing thundering
//! herd from repeated unknown kids.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use jsonwebtoken::{decode, decode_header, DecodingKey, TokenData, Validation};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::info;

// ═════════════════════════════════════════════════════════════════════════════
// Entra ID configuration & middleware (synchronous, OnceLock-based)
// ═════════════════════════════════════════════════════════════════════════════

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

// ── Legacy JWKS cache (OnceLock, sync) ──────────────────────────────────────

#[derive(Debug, Deserialize)]
struct LegacyJwksKey {
    kid: String,
    n: String,
    e: String,
    kty: String,
}

#[derive(Debug, Deserialize)]
struct LegacyJwksResponse {
    keys: Vec<LegacyJwksKey>,
}

fn legacy_jwks_keys(tenant_id: &str) -> &'static Vec<LegacyJwksKey> {
    static KEYS: OnceLock<Vec<LegacyJwksKey>> = OnceLock::new();
    KEYS.get_or_init(|| {
        let url = format!("https://login.microsoftonline.com/{tenant_id}/discovery/v2.0/keys");
        tracing::info!("fetching Entra ID JWKS from {url}");
        let mut resp = ureq::get(&url)
            .call()
            .unwrap_or_else(|e| panic!("failed to fetch JWKS from {url}: {e}"));
        let jwks: LegacyJwksResponse = resp
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
) -> Result<TokenData<EntraClaims>, String> {
    let header = decode_header(token).map_err(|e| format!("invalid JWT header: {e}"))?;

    let kid = header.kid.as_deref().ok_or("JWT missing kid header")?;

    let keys = legacy_jwks_keys(&config.tenant_id);
    let jwk = keys
        .iter()
        .find(|k| k.kid == kid && k.kty == "RSA")
        .ok_or_else(|| format!("no matching JWKS key for kid={kid}"))?;

    let decoding_key = DecodingKey::from_rsa_components(&jwk.n, &jwk.e)
        .map_err(|e| format!("invalid RSA key components: {e}"))?;

    let mut validation = Validation::new(header.alg);
    validation.set_audience(&[&config.client_id, &format!("api://{}", config.client_id)]);
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

/// Axum middleware that enforces Entra ID JWT auth.
///
/// Skips auth for `GET /health` and when Entra config is not set.
pub async fn require_entra_auth(request: Request, next: Next) -> Response {
    let config = match entra_config() {
        Some(c) => c,
        None => return next.run(request).await,
    };

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

    match validate_entra_token(token, config) {
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

// ═════════════════════════════════════════════════════════════════════════════
// Async JWKS cache with re-fetch on unknown kid
// ═════════════════════════════════════════════════════════════════════════════

/// Default cooldown between JWKS re-fetches: 5 minutes.
const DEFAULT_COOLDOWN_SECS: u64 = 300;

/// Claims expected in incoming JWTs. Extend as needed.
#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: Option<String>,
    pub exp: Option<u64>,
    pub iss: Option<String>,
    pub aud: Option<serde_json::Value>,
}

/// A single JWK entry from the JWKS endpoint.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct Jwk {
    kid: Option<String>,
    kty: String,
    n: Option<String>,
    e: Option<String>,
    alg: Option<String>,
}

/// The JWKS response envelope.
#[derive(Debug, Deserialize)]
struct JwksResponse {
    keys: Vec<Jwk>,
}

/// Cached JWKS keyset with cooldown tracking.
pub struct JwksCache {
    /// kid -> DecodingKey
    keys: RwLock<HashMap<String, DecodingKey>>,
    /// When the keyset was last fetched.
    last_fetch: RwLock<Option<Instant>>,
    /// Minimum interval between re-fetches.
    cooldown: Duration,
    /// JWKS endpoint URL.
    jwks_url: String,
    /// HTTP client for fetching JWKS.
    client: reqwest::Client,
}

impl JwksCache {
    /// Create a new JWKS cache for the given endpoint URL.
    pub fn new(jwks_url: String) -> Self {
        let cooldown_secs: u64 = std::env::var("GQL_API_JWKS_RETRY_COOLDOWN_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_COOLDOWN_SECS);
        Self {
            keys: RwLock::new(HashMap::new()),
            last_fetch: RwLock::new(None),
            cooldown: Duration::from_secs(cooldown_secs),
            jwks_url,
            client: reqwest::Client::new(),
        }
    }

    /// Create with explicit cooldown (for testing).
    #[cfg(test)]
    pub fn with_cooldown(jwks_url: String, cooldown: Duration) -> Self {
        Self {
            keys: RwLock::new(HashMap::new()),
            last_fetch: RwLock::new(None),
            cooldown,
            jwks_url,
            client: reqwest::Client::new(),
        }
    }

    /// Fetch the JWKS from the remote endpoint and update the cache.
    async fn refresh(&self) -> Result<(), AuthError> {
        info!(url = %self.jwks_url, "refreshing JWKS keyset");

        let resp: JwksResponse = self
            .client
            .get(&self.jwks_url)
            .send()
            .await
            .map_err(|e| AuthError::JwksFetch(e.to_string()))?
            .json()
            .await
            .map_err(|e| AuthError::JwksFetch(e.to_string()))?;

        let mut new_keys = HashMap::new();
        for jwk in &resp.keys {
            let kid = match &jwk.kid {
                Some(k) => k.clone(),
                None => continue,
            };
            if jwk.kty != "RSA" {
                continue;
            }
            let (n, e) = match (&jwk.n, &jwk.e) {
                (Some(n), Some(e)) => (n, e),
                _ => continue,
            };
            if let Ok(dk) = DecodingKey::from_rsa_components(n, e) {
                new_keys.insert(kid, dk);
            }
        }

        *self.keys.write().await = new_keys;
        *self.last_fetch.write().await = Some(Instant::now());
        Ok(())
    }

    /// Returns true if a refresh is allowed (cooldown has elapsed or no fetch yet).
    async fn can_refresh(&self) -> bool {
        let last = self.last_fetch.read().await;
        match *last {
            None => true,
            Some(t) => t.elapsed() >= self.cooldown,
        }
    }

    /// Look up a decoding key by `kid`, attempting one re-fetch on miss.
    async fn get_key(&self, kid: &str) -> Result<DecodingKey, AuthError> {
        // First try: check existing cache.
        {
            let keys = self.keys.read().await;
            if let Some(dk) = keys.get(kid) {
                return Ok(dk.clone());
            }
        }

        // Cache miss -- attempt re-fetch if cooldown allows.
        if self.can_refresh().await {
            info!(kid = kid, "JWKS key not found, attempting re-fetch");
            self.refresh().await?;

            // Second try after refresh.
            let keys = self.keys.read().await;
            if let Some(dk) = keys.get(kid) {
                return Ok(dk.clone());
            }
        }

        Err(AuthError::UnknownKid(kid.to_string()))
    }
}

/// Errors from token validation.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("failed to fetch JWKS: {0}")]
    JwksFetch(String),
    #[error("invalid token header: {0}")]
    InvalidHeader(String),
    #[error("token has no kid header")]
    MissingKid,
    #[error("no JWKS key matches kid \"{0}\"")]
    UnknownKid(String),
    #[error("token validation failed: {0}")]
    ValidationFailed(String),
}

/// Validate a JWT against the cached JWKS keyset.
///
/// On `kid` miss, attempts one JWKS re-fetch (subject to cooldown) before
/// returning an error.
pub async fn validate_token(token: &str, jwks: &JwksCache) -> Result<TokenData<Claims>, AuthError> {
    let header = decode_header(token).map_err(|e| AuthError::InvalidHeader(e.to_string()))?;

    let kid = header.kid.ok_or(AuthError::MissingKid)?;
    let key = jwks.get_key(&kid).await?;

    let alg = header.alg;
    let mut validation = Validation::new(alg);
    // Disable audience validation by default -- callers may tighten this.
    validation.validate_aud = false;

    decode::<Claims>(token, &key, &validation)
        .map_err(|e| AuthError::ValidationFailed(e.to_string()))
}

/// Shared JWKS state suitable for embedding in `AppState`.
pub type SharedJwksCache = Arc<JwksCache>;

/// Build a `SharedJwksCache` from the `GQL_API_JWKS_URL` env var.
/// Returns `None` if the env var is not set (auth disabled).
pub fn jwks_cache_from_env() -> Option<SharedJwksCache> {
    std::env::var("GQL_API_JWKS_URL").ok().map(|url| {
        info!(url = %url, "JWKS auth enabled");
        Arc::new(JwksCache::new(url))
    })
}

// ═════════════════════════════════════════════════════════════════════════════
// Tests
// ═════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    // -- Entra middleware tests -----------------------------------------------

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

    // -- Async JWKS cache tests -----------------------------------------------

    use base64::Engine;
    use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
    use rsa::pkcs1::EncodeRsaPrivateKey;
    use rsa::rand_core::OsRng;
    use rsa::traits::PublicKeyParts;
    use serde_json::json;

    /// Helper: generate an RSA key pair for testing.
    fn test_rsa_keys() -> (EncodingKey, String, String) {
        let priv_key = rsa::RsaPrivateKey::new(&mut OsRng, 2048).unwrap();
        let pub_key = rsa::RsaPublicKey::from(&priv_key);

        let n = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(pub_key.n().to_bytes_be());
        let e = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(pub_key.e().to_bytes_be());

        let der = priv_key.to_pkcs1_der().unwrap();
        let encoding_key = EncodingKey::from_rsa_der(der.as_bytes());

        (encoding_key, n, e)
    }

    /// Start a minimal mock JWKS server.
    async fn mock_jwks_server(
        n: &str,
        e: &str,
        kid: &str,
    ) -> (tokio::task::JoinHandle<()>, String) {
        let body = serde_json::to_string(&json!({
            "keys": [{
                "kty": "RSA",
                "kid": kid,
                "n": n,
                "e": e,
                "alg": "RS256",
                "use": "sig"
            }]
        }))
        .unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://{addr}/.well-known/jwks.json");

        let handle = tokio::spawn(async move {
            // Serve exactly 2 requests (initial + one re-fetch).
            for _ in 0..2 {
                let (mut stream, _) = match listener.accept().await {
                    Ok(s) => s,
                    Err(_) => break,
                };
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf).await;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });

        (handle, url)
    }

    #[tokio::test]
    async fn test_validate_token_success() {
        let (encoding_key, n, e) = test_rsa_keys();
        let kid = "test-kid-1";
        let (_server, url) = mock_jwks_server(&n, &e, kid).await;

        let cache = JwksCache::with_cooldown(url, Duration::from_secs(0));

        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(kid.to_string());
        let claims = Claims {
            sub: Some("user@example.com".to_string()),
            exp: Some(u64::MAX),
            iss: None,
            aud: None,
        };
        let token = encode(&header, &claims, &encoding_key).unwrap();

        let result = validate_token(&token, &cache).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().claims.sub.unwrap(), "user@example.com");
    }

    #[tokio::test]
    async fn test_unknown_kid_triggers_refetch() {
        let (encoding_key, n, e) = test_rsa_keys();
        let kid = "rotated-kid";
        let (_server, url) = mock_jwks_server(&n, &e, kid).await;

        let cache = JwksCache::with_cooldown(url, Duration::from_secs(0));

        // Pre-populate cache with a different key so the first lookup misses.
        {
            let mut keys = cache.keys.write().await;
            keys.insert(
                "old-kid".to_string(),
                DecodingKey::from_rsa_components(&n, &e).unwrap(),
            );
        }
        *cache.last_fetch.write().await = Some(Instant::now() - Duration::from_secs(600));

        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(kid.to_string());
        let claims = Claims {
            sub: Some("rotated@example.com".to_string()),
            exp: Some(u64::MAX),
            iss: None,
            aud: None,
        };
        let token = encode(&header, &claims, &encoding_key).unwrap();

        let result = validate_token(&token, &cache).await;
        assert!(result.is_ok(), "should succeed after re-fetch");
    }

    #[tokio::test]
    async fn test_cooldown_prevents_refetch() {
        // Use a non-existent URL -- if a fetch is attempted it will fail.
        let cache = JwksCache::with_cooldown(
            "http://127.0.0.1:1/nope".to_string(),
            Duration::from_secs(9999),
        );

        // Pretend we just fetched.
        *cache.last_fetch.write().await = Some(Instant::now());

        assert!(!cache.can_refresh().await, "cooldown should block re-fetch");
    }

    #[tokio::test]
    async fn test_missing_kid_error() {
        let cache = JwksCache::with_cooldown(
            "http://127.0.0.1:1/nope".to_string(),
            Duration::from_secs(9999),
        );

        // A token without a kid header.
        let token = encode(
            &Header::new(Algorithm::HS256),
            &json!({"sub": "x"}),
            &EncodingKey::from_secret(b"secret"),
        )
        .unwrap();

        let result = validate_token(&token, &cache).await;
        assert!(matches!(result, Err(AuthError::MissingKid)));
    }
}
