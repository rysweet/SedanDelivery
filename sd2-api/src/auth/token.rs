//! Entra ID JWT validation.

use axum::http::StatusCode;
use jsonwebtoken::{decode, decode_header, DecodingKey, TokenData, Validation};

use super::claims::EntraClaims;
use super::config::EntraConfig;
use super::jwks::find_jwks_key;

pub(super) async fn validate_entra_token(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_validate_token_rejects_garbage() {
        let config = EntraConfig {
            tenant_id: "test-tenant".to_string(),
            client_id: "test-client".to_string(),
        };
        let result = validate_entra_token("not.a.jwt", &config).await;
        assert!(result.is_err(), "garbage token must be rejected");
    }
}
