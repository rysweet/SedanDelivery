// Token management: Azure AD client-credentials flow for Fabric API.

use std::time::{Duration, Instant};

use serde_json::Value as JValue;

use cypher_core::error::StorageError;

use crate::sanitize::urlenc;
use crate::FabricGraphStore;

pub(crate) struct TokenCache {
    pub(crate) token: String,
    pub(crate) expires_at: Instant,
}

impl FabricGraphStore {
    pub(crate) fn bearer_token(&self) -> Result<String, StorageError> {
        // Fast path: check the cache. If the token is still valid, return it.
        {
            let cache = self.token.lock().unwrap_or_else(|e| {
                tracing::warn!("token cache mutex was poisoned, recovering");
                e.into_inner()
            });
            if let Some(ref c) = *cache {
                if c.expires_at > Instant::now() + Duration::from_secs(60) {
                    return Ok(c.token.clone());
                }
            }
        }

        // Single-flight: acquire the refresh lock so only one thread performs
        // the HTTP call. Other threads block here and will find a fresh token
        // in the cache when they proceed.
        let _refresh_guard = self.refresh_lock.lock().unwrap_or_else(|e| {
            tracing::warn!("refresh_lock mutex was poisoned, recovering");
            e.into_inner()
        });

        // Re-check the cache: another thread may have refreshed while we waited.
        {
            let cache = self.token.lock().unwrap_or_else(|e| {
                tracing::warn!("token cache mutex was poisoned, recovering");
                e.into_inner()
            });
            if let Some(ref c) = *cache {
                if c.expires_at > Instant::now() + Duration::from_secs(60) {
                    return Ok(c.token.clone());
                }
            }
        }

        // Fetch a new token (only one thread reaches here at a time).
        let url = self.token_url_override.clone().unwrap_or_else(|| {
            format!(
                "https://login.microsoftonline.com/{}/oauth2/v2.0/token",
                self.tenant_id
            )
        });
        let body = format!(
            "grant_type=client_credentials&client_id={}&client_secret={}&scope=https%3A%2F%2Fapi.fabric.microsoft.com%2F.default",
            urlenc(&self.client_id),
            urlenc(self.client_secret.expose_secret()),
        );
        let resp: JValue = self
            .agent
            .post(&url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .send(body.as_bytes())
            .map_err(|e| StorageError::Backend {
                msg: format!("token fetch: {e}"),
            })?
            .body_mut()
            .read_json()
            .map_err(|e| StorageError::Backend {
                msg: format!("token parse: {e}"),
            })?;

        let token = resp["access_token"]
            .as_str()
            .ok_or_else(|| StorageError::Backend {
                msg: "no access_token in response".into(),
            })?
            .to_string();
        let expires_in = resp["expires_in"].as_u64().unwrap_or_else(|| {
            tracing::warn!("token response missing expires_in field, defaulting to 3600s");
            3600
        });

        // Update the cache so waiting threads (and future callers) use this token.
        {
            let mut cache = self.token.lock().unwrap_or_else(|e| {
                tracing::warn!("token cache mutex was poisoned, recovering");
                e.into_inner()
            });
            *cache = Some(TokenCache {
                token: token.clone(),
                expires_at: Instant::now() + Duration::from_secs(expires_in),
            });
        }
        Ok(token)
    }
}
