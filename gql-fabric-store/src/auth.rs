// Token management: Azure AD client-credentials flow for Fabric API.

use std::time::{Duration, Instant};

use serde_json::Value as JValue;

use gql_core::error::StorageError;

use crate::sanitize::urlenc;
use crate::FabricGraphStore;

pub(crate) struct TokenCache {
    pub(crate) token: String,
    pub(crate) expires_at: Instant,
}

impl FabricGraphStore {
    pub(crate) fn bearer_token(&self) -> Result<String, StorageError> {
        // Phase 1: Check the cache under the lock.  If the token is still
        // valid, clone it and return immediately -- no HTTP call needed.
        {
            let cache = self.token.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(ref c) = *cache {
                // Safe comparison: avoids Instant subtraction panic when
                // expires_at is in the past.
                if c.expires_at > Instant::now() + Duration::from_secs(60) {
                    return Ok(c.token.clone());
                }
            }
            // Lock is dropped here before the (potentially slow) HTTP call,
            // so other threads are NOT blocked during a network partition.
        }

        // Phase 2: Fetch a new token WITHOUT holding the Mutex.
        let url = format!(
            "https://login.microsoftonline.com/{}/oauth2/v2.0/token",
            self.tenant_id
        );
        let body = format!(
            "grant_type=client_credentials&client_id={}&client_secret={}&scope=https%3A%2F%2Fapi.fabric.microsoft.com%2F.default",
            urlenc(&self.client_id),
            urlenc(&self.client_secret),
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
        let expires_in = resp["expires_in"].as_u64().unwrap_or(3600);

        // Phase 3: Re-acquire the lock and update the cache.  Another thread
        // may have raced us and already written a fresh token -- that is fine;
        // we overwrite with our equally-fresh one.
        {
            let mut cache = self.token.lock().unwrap_or_else(|e| e.into_inner());
            *cache = Some(TokenCache {
                token: token.clone(),
                expires_at: Instant::now() + Duration::from_secs(expires_in),
            });
        }
        Ok(token)
    }
}
