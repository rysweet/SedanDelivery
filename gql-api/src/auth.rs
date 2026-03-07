//! Entra ID (Azure AD) JWT validation middleware.
//!
//! When `GQL_API_ENTRA_TENANT_ID` and `GQL_API_ENTRA_CLIENT_ID` are set,
//! all requests (except `GET /health`) must carry a valid
//! `Authorization: Bearer <token>` header signed by the configured Entra ID
//! tenant.
//!
//! When these env vars are absent the middleware is a no-op, allowing
//! unauthenticated local development.

use std::sync::OnceLock;

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

// ── JWKS cache ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
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

/// Fetch JWKS keys from the Entra OIDC endpoint.
///
/// Keys are cached in a `OnceLock` on success. On failure, the function
/// **panics** rather than caching an empty set — a permanently empty JWKS
/// cache would silently reject all tokens for the process lifetime.
fn jwks_keys(tenant_id: &str) -> &'static Vec<JwksKey> {
    static KEYS: OnceLock<Vec<JwksKey>> = OnceLock::new();
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

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entra_config_from_env_returns_some_or_none() {
        // EntraConfig::from_env() should return Some only when both env vars are set.
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
}
