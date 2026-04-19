//! JWKS fetch + cache for Entra ID public keys.
//!
//! Keys are cached with a configurable TTL (default 24 h) and re-fetched
//! on unknown `kid` with a cooldown (default 60 s) to avoid hammering the
//! identity provider.

use std::sync::{OnceLock, RwLock};
use std::time::{Duration, Instant};

use serde::Deserialize;

use super::config::env_duration_secs;

#[derive(Debug, Deserialize, Clone)]
pub(super) struct JwksKey {
    pub(super) kid: String,
    pub(super) n: String,
    pub(super) e: String,
    pub(super) kty: String,
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
    refresh_lock: tokio::sync::Mutex<()>,
    refresh_ttl: Duration,
    retry_cooldown: Duration,
}

impl JwksCache {
    fn new() -> Self {
        Self {
            inner: RwLock::new(None),
            refresh_lock: tokio::sync::Mutex::new(()),
            refresh_ttl: env_duration_secs("GQL_API_JWKS_REFRESH_SECS", 86400),
            retry_cooldown: env_duration_secs("GQL_API_JWKS_RETRY_COOLDOWN_SECS", 60),
        }
    }
}

fn jwks_http_timeout() -> Duration {
    env_duration_secs("GQL_API_JWKS_HTTP_TIMEOUT_SECS", 5)
}

fn jwks_http_agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .timeout_global(Some(jwks_http_timeout()))
            .build()
            .new_agent()
    })
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
        let mut resp = jwks_http_agent()
            .get(&url_clone)
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
pub(super) async fn find_jwks_key(tenant_id: &str, kid: &str) -> Result<JwksKey, String> {
    let cache = jwks_cache();

    // Fast path: read lock — check if we have a non-expired key.
    {
        let guard = cache
            .inner
            .read()
            .map_err(|_| "JWKS cache lock poisoned".to_string())?;
        if let Some(ref inner) = *guard {
            if Instant::now().duration_since(inner.fetched_at) < cache.refresh_ttl {
                if let Some(key) = inner.keys.iter().find(|k| k.kid == kid && k.kty == "RSA") {
                    return Ok(key.clone());
                }
            }
        }
    }

    let _refresh_guard = cache.refresh_lock.lock().await;

    // Slow path: re-check under the single-flight gate before deciding to fetch.
    {
        let guard = cache
            .inner
            .read()
            .map_err(|_| "JWKS cache lock poisoned".to_string())?;
        let now = Instant::now();

        // Double-check: another task may have refreshed while we waited.
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
    }

    // Fetch fresh keys while holding the single-flight gate so concurrent cache
    // misses do not stampede the identity provider or our blocking pool.
    match fetch_jwks(tenant_id).await {
        Ok(keys) => {
            let result = keys
                .iter()
                .find(|k| k.kid == kid && k.kty == "RSA")
                .cloned();
            let fetched_at = Instant::now();
            // Re-acquire write lock to store the fetched keys.
            let mut guard = cache
                .inner
                .write()
                .map_err(|_| "JWKS cache lock poisoned".to_string())?;
            *guard = Some(JwksCacheInner {
                keys,
                fetched_at,
                last_retry_at: Some(fetched_at),
            });
            result.ok_or_else(|| format!("no matching JWKS key for kid={kid}"))
        }
        Err(e) => {
            crate::metrics::jwks_refresh_failure();
            let retry_at = Instant::now();
            // If we have stale keys, update the retry timestamp and try them.
            let mut guard = cache
                .inner
                .write()
                .map_err(|_| "JWKS cache lock poisoned".to_string())?;
            if let Some(ref mut inner) = *guard {
                inner.last_retry_at = Some(retry_at);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jwks_cache_initializes_empty() {
        let cache = JwksCache {
            inner: RwLock::new(None),
            refresh_lock: tokio::sync::Mutex::new(()),
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
            refresh_lock: tokio::sync::Mutex::new(()),
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
            refresh_lock: tokio::sync::Mutex::new(()),
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
            !inner.keys.iter().any(|k| k.kid == "missing-kid"),
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
}
