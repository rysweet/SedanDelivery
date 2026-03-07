//! Entra ID JWT authentication middleware.
//!
//! When `GQL_API_ENTRA_TENANT_ID` and `GQL_API_ENTRA_CLIENT_ID` are set,
//! all requests (except `GET /health`) must carry a valid
//! `Authorization: Bearer <token>` header signed by the configured Entra ID
//! tenant.  When absent the middleware is a no-op.

use std::sync::OnceLock;

use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use jsonwebtoken::{decode, decode_header, DecodingKey, TokenData, Validation};
use serde::{Deserialize, Serialize};

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
        Err(e) => {
            tracing::warn!("auth rejected: {e}");
            crate::metrics::auth_rejection();
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
}
