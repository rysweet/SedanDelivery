//! Auth failure rate limiting (in-memory + Redis-backed).
//!
//! When `GQL_REDIS_URL` is set, auth failure counters are shared across all
//! Bolt replicas via Redis INCR + EXPIRE. When no Redis URL is configured,
//! an in-process DashMap provides per-replica rate limiting as a fallback.
//!
//! On Redis unavailability the limiter **fails open** (allows the attempt)
//! and logs a warning, so a Redis outage never locks out legitimate users.

use std::net::IpAddr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use dashmap::DashMap;

/// Maximum number of distinct IPs tracked simultaneously (in-memory mode).
const MAX_TRACKED_IPS: usize = 100_000;

/// Redis key prefix for distributed auth rate limiting counters.
const REDIS_KEY_PREFIX: &str = "__sd2:auth_rate:";

/// Lua script: atomically INCR the counter and set EXPIRE on first increment.
const INCR_WITH_EXPIRE_LUA: &str = "\
local count = redis.call('INCR', KEYS[1])\n\
if count == 1 then\n\
  redis.call('EXPIRE', KEYS[1], ARGV[1])\n\
end\n\
return count";

/// Maximum failed auth attempts per IP before returning RateLimit.
/// Configurable via `BOLT_AUTH_MAX_ATTEMPTS` env var (default: 10).
fn max_failures_per_ip() -> u32 {
    static VAL: OnceLock<u32> = OnceLock::new();
    *VAL.get_or_init(|| {
        cypher_core::config::parse_env_range("BOLT_AUTH_MAX_ATTEMPTS", 10, 1, 10000)
    })
}

/// Seconds after which an entry is considered stale and eligible for sweep.
/// Configurable via `BOLT_AUTH_WINDOW_SECS` env var (default: 3600).
fn stale_after_secs() -> u64 {
    static VAL: OnceLock<u64> = OnceLock::new();
    *VAL.get_or_init(|| {
        cypher_core::config::parse_env_range("BOLT_AUTH_WINDOW_SECS", 3600u64, 1, 86400)
    })
}

/// Canonical string form of an IP address for use as a rate-limit key.
pub(super) fn normalize_ip(ip: IpAddr) -> String {
    // Rust's `IpAddr::to_string()` already produces canonical form for both
    // IPv4 and IPv6 (RFC 5952 compressed notation).
    ip.to_string()
}

// -- In-memory backend -------------------------------------------------------

struct InMemoryLimiter {
    map: DashMap<String, (AtomicU32, AtomicU64)>,
}

impl InMemoryLimiter {
    fn new() -> Self {
        Self {
            map: DashMap::new(),
        }
    }

    fn record_failure(&self, key: &str) -> bool {
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        if let Some(entry) = self.map.get(key) {
            entry.1.store(now_secs, Ordering::Relaxed);
            let prev = entry.0.fetch_add(1, Ordering::Relaxed);
            return prev + 1 > max_failures_per_ip();
        }

        if self.map.len() < MAX_TRACKED_IPS {
            self.map
                .entry(key.to_string())
                .or_insert_with(|| (AtomicU32::new(0), AtomicU64::new(now_secs)))
                .0
                .fetch_add(1, Ordering::Relaxed);
        }
        false
    }

    fn record_success(&self, key: &str) {
        self.map.remove(key);
    }

    fn sweep(&self) {
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.map.retain(|_, v| {
            now_secs.saturating_sub(v.1.load(Ordering::Relaxed)) < stale_after_secs()
        });
    }
}

// -- Redis backend -----------------------------------------------------------

struct RedisLimiter {
    client: redis::Client,
    conn: tokio::sync::Mutex<Option<redis::aio::MultiplexedConnection>>,
}

impl RedisLimiter {
    fn new(client: redis::Client) -> Self {
        Self {
            client,
            conn: tokio::sync::Mutex::new(None),
        }
    }

    async fn get_conn(&self) -> Result<redis::aio::MultiplexedConnection, redis::RedisError> {
        let mut guard = self.conn.lock().await;
        if let Some(ref conn) = *guard {
            return Ok(conn.clone());
        }
        let conn: redis::aio::MultiplexedConnection =
            self.client.get_multiplexed_async_connection().await?;
        *guard = Some(conn.clone());
        Ok(conn)
    }

    async fn clear_cached_conn(&self) {
        *self.conn.lock().await = None;
    }

    /// Increment failure counter via Lua script. Returns `Some(true)` if
    /// rate-limited, `Some(false)` if not, or `None` on Redis failure (fail open).
    async fn record_failure(&self, key: &str) -> Option<bool> {
        let redis_key = format!("{REDIS_KEY_PREFIX}{key}");
        let ttl = stale_after_secs();

        let mut conn = match self.get_conn().await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "Redis unavailable for auth rate limiting; failing open");
                self.clear_cached_conn().await;
                return None;
            }
        };

        let result: Result<i64, redis::RedisError> = redis::cmd("EVAL")
            .arg(INCR_WITH_EXPIRE_LUA)
            .arg(1_i32)
            .arg(&redis_key)
            .arg(ttl as i64)
            .query_async(&mut conn)
            .await;

        match result {
            Ok(count) => Some(count > i64::from(max_failures_per_ip())),
            Err(e) => {
                tracing::warn!(error = %e, "Redis auth rate limit EVAL failed; failing open");
                self.clear_cached_conn().await;
                None
            }
        }
    }

    async fn record_success(&self, key: &str) {
        let redis_key = format!("{REDIS_KEY_PREFIX}{key}");
        let mut conn = match self.get_conn().await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "Redis unavailable for auth rate limit reset");
                self.clear_cached_conn().await;
                return;
            }
        };

        let result: Result<i64, redis::RedisError> = redis::cmd("DEL")
            .arg(&redis_key)
            .query_async(&mut conn)
            .await;
        if let Err(e) = result {
            tracing::warn!(error = %e, "Redis auth rate limit DEL failed");
            self.clear_cached_conn().await;
        }
    }
}

// -- Combined rate limiter ---------------------------------------------------

enum AuthRateLimiter {
    InMemory(InMemoryLimiter),
    Redis(RedisLimiter),
}

impl AuthRateLimiter {
    async fn record_failure(&self, key: &str) -> bool {
        match self {
            Self::InMemory(lim) => lim.record_failure(key),
            Self::Redis(lim) => {
                // Fail open: if Redis is unavailable, allow the attempt.
                lim.record_failure(key).await.unwrap_or(false)
            }
        }
    }

    async fn record_success(&self, key: &str) {
        match self {
            Self::InMemory(lim) => lim.record_success(key),
            Self::Redis(lim) => lim.record_success(key).await,
        }
    }

    fn sweep(&self) {
        match self {
            Self::InMemory(lim) => lim.sweep(),
            // Redis handles TTL via EXPIRE; nothing to sweep.
            Self::Redis(_) => {}
        }
    }
}

fn auth_rate_limiter() -> &'static AuthRateLimiter {
    static LIMITER: OnceLock<AuthRateLimiter> = OnceLock::new();
    LIMITER.get_or_init(|| {
        if let Ok(url) = std::env::var("GQL_REDIS_URL") {
            match redis::Client::open(url.as_str()) {
                Ok(client) => {
                    tracing::info!("Auth rate limiter using Redis-backed counters");
                    return AuthRateLimiter::Redis(RedisLimiter::new(client));
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "Failed to create Redis client for auth rate limiter; \
                         falling back to in-memory"
                    );
                }
            }
        }
        tracing::info!("Auth rate limiter using in-memory counters");
        AuthRateLimiter::InMemory(InMemoryLimiter::new())
    })
}

/// Record a failed auth attempt for `key`; returns `true` if now rate-limited.
pub(super) async fn record_failure(key: &str) -> bool {
    auth_rate_limiter().record_failure(key).await
}

/// Record a successful auth attempt for `key`, clearing any prior failures.
pub(super) async fn record_success(key: &str) {
    auth_rate_limiter().record_success(key).await;
}

/// Spawn a background task that periodically sweeps stale rate-limiter entries.
/// In Redis mode this is a no-op (EXPIRE handles TTL), but the task still runs
/// so the sweep interval is consistent.
pub fn spawn_auth_rate_limiter_sweep() {
    tokio::spawn(async {
        let mut interval = tokio::time::interval(Duration::from_secs(300));
        loop {
            interval.tick().await;
            auth_rate_limiter().sweep();
        }
    });
}

#[cfg(test)]
pub(crate) async fn clear_auth_rate_limit(ip: IpAddr) {
    auth_rate_limiter().record_success(&normalize_ip(ip)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_rate_limit_entries_are_evicted() {
        // In tests, GQL_REDIS_URL is not set so the limiter is always InMemory.
        let stale_ip: IpAddr = "10.0.0.21".parse().unwrap();
        let stale_key = stale_ip.to_string();
        let limiter = auth_rate_limiter();
        let map = match limiter {
            AuthRateLimiter::InMemory(lim) => &lim.map,
            AuthRateLimiter::Redis(_) => panic!("expected InMemory limiter in test"),
        };
        map.insert(
            stale_key.clone(),
            (
                AtomicU32::new(99),
                AtomicU64::new(0), // epoch = maximally stale
            ),
        );
        limiter.sweep();
        assert!(
            !map.contains_key(&stale_key),
            "expired auth-failure entries should be pruned"
        );
    }
}
