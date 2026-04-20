//! Redis-backed global bookmark counter for cross-replica consistency.
//!
//! When Redis is configured (`GQL_REDIS_URL` or `REDIS_URL`),
//! the counter uses `INCR __sd2_bookmark` so all replicas share the same
//! monotonic sequence.  When Redis is not configured (local dev / tests),
//! falls back to an in-process `AtomicU64`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use redis::AsyncConnectionConfig;

pub struct RedisBookmarkStore {
    client: redis::Client,
    conn: tokio::sync::Mutex<Option<redis::aio::MultiplexedConnection>>,
}

impl RedisBookmarkStore {
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
        let conn = self
            .client
            .get_multiplexed_async_connection_with_config(&bookmark_redis_async_config())
            .await?;
        *guard = Some(conn.clone());
        Ok(conn)
    }

    async fn clear_cached_conn(&self) {
        *self.conn.lock().await = None;
    }
}

/// Global bookmark counter, backed by Redis or an in-process atomic.
pub enum BookmarkCounter {
    /// In-process counter (no Redis).
    InMemory(AtomicU64),
    /// Redis-backed counter using `INCR __sd2_bookmark`.
    Redis(Box<RedisBookmarkStore>),
}

/// Redis key for the global bookmark counter.
const BOOKMARK_KEY: &str = "__sd2_bookmark";
const BOOKMARK_TS_KEY_PREFIX: &str = "__sd2_bookmark_ts:";
const RECENT_BOOKMARK_TTL_SECS: u64 = 30;
const FALLBACK_BOOKMARK_START: u64 = 1 << 63;

fn local_bookmark_timestamps() -> &'static Mutex<HashMap<u64, Instant>> {
    static BOOKMARK_TIMESTAMPS: OnceLock<Mutex<HashMap<u64, Instant>>> = OnceLock::new();
    BOOKMARK_TIMESTAMPS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn bookmark_timestamp_key(tx_id: u64) -> String {
    format!("{BOOKMARK_TS_KEY_PREFIX}{tx_id}")
}

fn now_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before UNIX epoch — cannot generate bookmark timestamps")
        .as_millis() as u64
}

fn next_fallback_bookmark_id() -> u64 {
    static FALLBACK: AtomicU64 = AtomicU64::new(FALLBACK_BOOKMARK_START - 1);
    FALLBACK.fetch_add(1, Ordering::SeqCst) + 1
}

fn env_duration_ms(name: &str, default_ms: u64) -> Duration {
    Duration::from_millis(
        std::env::var(name)
            .ok()
            .and_then(|value| match value.parse::<u64>() {
                Ok(ms) => Some(ms),
                Err(err) => {
                    tracing::warn!(
                        "{name}=<invalid> is not a valid u64 ({err}), using default {default_ms}"
                    );
                    None
                }
            })
            .unwrap_or(default_ms),
    )
}

fn bookmark_redis_connect_timeout() -> Duration {
    static TIMEOUT: OnceLock<Duration> = OnceLock::new();
    *TIMEOUT.get_or_init(|| env_duration_ms("BOLT_BOOKMARK_REDIS_CONNECT_TIMEOUT_MS", 1_000))
}

fn bookmark_redis_read_timeout() -> Duration {
    static TIMEOUT: OnceLock<Duration> = OnceLock::new();
    *TIMEOUT.get_or_init(|| env_duration_ms("BOLT_BOOKMARK_REDIS_READ_TIMEOUT_MS", 1_000))
}

fn bookmark_redis_write_timeout() -> Duration {
    static TIMEOUT: OnceLock<Duration> = OnceLock::new();
    *TIMEOUT.get_or_init(|| env_duration_ms("BOLT_BOOKMARK_REDIS_WRITE_TIMEOUT_MS", 1_000))
}

fn bookmark_redis_async_config() -> AsyncConnectionConfig {
    AsyncConnectionConfig::new()
        .set_connection_timeout(Some(bookmark_redis_connect_timeout()))
        .set_response_timeout(Some(
            bookmark_redis_read_timeout().max(bookmark_redis_write_timeout()),
        ))
}

impl BookmarkCounter {
    /// Create a Redis-backed counter if `GQL_REDIS_URL` or `REDIS_URL` is set;
    /// otherwise fall back to an in-process atomic.
    pub fn from_env() -> Self {
        let redis_url = std::env::var("GQL_REDIS_URL")
            .or_else(|_| std::env::var("REDIS_URL"))
            .ok();
        if let Some(url) = redis_url {
            match redis::Client::open(url) {
                Ok(client) => {
                    tracing::info!("bookmark counter: using Redis INCR __sd2_bookmark");
                    return BookmarkCounter::Redis(Box::new(RedisBookmarkStore::new(client)));
                }
                Err(e) => {
                    tracing::warn!(
                        "bookmark counter: Redis client failed ({e}), falling back to in-memory"
                    );
                }
            }
        }
        tracing::info!("bookmark counter: using in-memory AtomicU64");
        BookmarkCounter::InMemory(AtomicU64::new(0))
    }

    /// Increment and return the next bookmark value.
    ///
    /// Returns a `u64` (1-based). On Redis error, logs a warning and falls back
    /// to a local monotonic increment to avoid breaking the session.
    pub async fn next(&self) -> u64 {
        match self {
            BookmarkCounter::InMemory(counter) => counter.fetch_add(1, Ordering::SeqCst) + 1,
            BookmarkCounter::Redis(store) => match store.get_conn().await {
                Ok(mut conn) => {
                    let result: redis::RedisResult<i64> = redis::cmd("INCR")
                        .arg(BOOKMARK_KEY)
                        .query_async(&mut conn)
                        .await;
                    match result {
                        Ok(n) => n as u64,
                        Err(e) => {
                            tracing::warn!(
                                "bookmark Redis INCR failed ({e}), using local fallback"
                            );
                            store.clear_cached_conn().await;
                            next_fallback_bookmark_id()
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("bookmark Redis connection failed ({e}), using local fallback");
                    store.clear_cached_conn().await;
                    next_fallback_bookmark_id()
                }
            },
        }
    }

    pub async fn record_issued_at(&self, tx_id: u64) {
        {
            let mut local = local_bookmark_timestamps()
                .lock()
                .expect("bookmark timestamp mutex poisoned");
            local.retain(|_, issued_at| {
                issued_at.elapsed() <= Duration::from_secs(RECENT_BOOKMARK_TTL_SECS)
            });
            local.insert(tx_id, Instant::now());
        }

        if let BookmarkCounter::Redis(store) = self {
            let timestamp = now_unix_millis();
            match store.get_conn().await {
                Ok(mut conn) => {
                    let result: redis::RedisResult<()> = redis::cmd("SET")
                        .arg(bookmark_timestamp_key(tx_id))
                        .arg(timestamp)
                        .arg("EX")
                        .arg(RECENT_BOOKMARK_TTL_SECS)
                        .query_async(&mut conn)
                        .await;
                    if let Err(err) = result {
                        tracing::warn!(
                            tx_id,
                            error = %err,
                            "bookmark timestamp SET failed"
                        );
                        store.clear_cached_conn().await;
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        tx_id,
                        error = %err,
                        "bookmark timestamp Redis connection failed"
                    );
                    store.clear_cached_conn().await;
                }
            }
        }
    }

    pub async fn issued_age(&self, tx_id: u64) -> Option<Duration> {
        let local_age = {
            local_bookmark_timestamps()
                .lock()
                .expect("bookmark timestamp mutex poisoned")
                .get(&tx_id)
                .map(Instant::elapsed)
        };
        if let Some(age) = local_age {
            return Some(age);
        }

        if let BookmarkCounter::Redis(store) = self {
            match store.get_conn().await {
                Ok(mut conn) => {
                    let value: redis::RedisResult<Option<u64>> = redis::cmd("GET")
                        .arg(bookmark_timestamp_key(tx_id))
                        .query_async(&mut conn)
                        .await;
                    match value {
                        Ok(Some(issued_at_ms)) => {
                            let age_ms = now_unix_millis().saturating_sub(issued_at_ms);
                            return Some(Duration::from_millis(age_ms));
                        }
                        Ok(None) => {}
                        Err(err) => {
                            tracing::warn!(
                                tx_id,
                                error = %err,
                                "bookmark timestamp GET failed"
                            );
                            store.clear_cached_conn().await;
                        }
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        tx_id,
                        error = %err,
                        "bookmark timestamp Redis connection failed"
                    );
                    store.clear_cached_conn().await;
                }
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bookmark_timestamp_key_uses_prefix() {
        assert_eq!(bookmark_timestamp_key(0), "__sd2_bookmark_ts:0");
        assert_eq!(
            bookmark_timestamp_key(u64::MAX),
            format!("__sd2_bookmark_ts:{}", u64::MAX),
        );
    }

    #[test]
    fn now_unix_millis_is_positive_and_recent() {
        let a = now_unix_millis();
        // Very loose lower bound — any time after Jan 1 2020 in ms.
        assert!(a > 1_577_836_800_000);
        // Monotonic-ish — second call must not be earlier than the first.
        let b = now_unix_millis();
        assert!(b >= a);
    }

    #[test]
    fn next_fallback_bookmark_id_is_strictly_monotonic() {
        let a = next_fallback_bookmark_id();
        let b = next_fallback_bookmark_id();
        let c = next_fallback_bookmark_id();
        assert!(a < b && b < c);
        assert!(a >= FALLBACK_BOOKMARK_START);
    }

    #[test]
    fn env_duration_ms_uses_default_when_unset() {
        // Use a name that nothing else sets.
        let name = "BOLT_TEST_BOOKMARK_ENV_UNSET_KEY";
        std::env::remove_var(name);
        assert_eq!(env_duration_ms(name, 1234), Duration::from_millis(1234));
    }

    #[test]
    fn env_duration_ms_parses_valid_value() {
        let name = "BOLT_TEST_BOOKMARK_ENV_VALID_KEY";
        std::env::set_var(name, "9999");
        assert_eq!(env_duration_ms(name, 1), Duration::from_millis(9999));
        std::env::remove_var(name);
    }

    #[test]
    fn env_duration_ms_falls_back_on_invalid_value() {
        let name = "BOLT_TEST_BOOKMARK_ENV_INVALID_KEY";
        std::env::set_var(name, "not-a-number");
        assert_eq!(env_duration_ms(name, 42), Duration::from_millis(42));
        std::env::remove_var(name);
    }

    #[test]
    fn from_env_falls_back_to_in_memory_without_redis_url() {
        // Snapshot and clear any existing values; restore on drop.
        struct EnvGuard {
            gql: Option<String>,
            redis: Option<String>,
        }
        impl Drop for EnvGuard {
            fn drop(&mut self) {
                match self.gql.take() {
                    Some(v) => std::env::set_var("GQL_REDIS_URL", v),
                    None => std::env::remove_var("GQL_REDIS_URL"),
                }
                match self.redis.take() {
                    Some(v) => std::env::set_var("REDIS_URL", v),
                    None => std::env::remove_var("REDIS_URL"),
                }
            }
        }
        let _guard = EnvGuard {
            gql: std::env::var("GQL_REDIS_URL").ok(),
            redis: std::env::var("REDIS_URL").ok(),
        };
        std::env::remove_var("GQL_REDIS_URL");
        std::env::remove_var("REDIS_URL");

        let counter = BookmarkCounter::from_env();
        assert!(matches!(counter, BookmarkCounter::InMemory(_)));
    }

    #[test]
    fn from_env_uses_redis_when_url_is_valid() {
        // We don't connect, only construct the client.
        let prev = std::env::var("GQL_REDIS_URL").ok();
        std::env::set_var("GQL_REDIS_URL", "redis://127.0.0.1:0/");
        let counter = BookmarkCounter::from_env();
        match prev {
            Some(v) => std::env::set_var("GQL_REDIS_URL", v),
            None => std::env::remove_var("GQL_REDIS_URL"),
        }
        assert!(matches!(counter, BookmarkCounter::Redis(_)));
    }

    #[tokio::test]
    async fn in_memory_next_is_monotonic_starting_at_one() {
        let counter = BookmarkCounter::InMemory(AtomicU64::new(0));
        assert_eq!(counter.next().await, 1);
        assert_eq!(counter.next().await, 2);
        assert_eq!(counter.next().await, 3);
    }

    #[tokio::test]
    async fn record_issued_at_then_issued_age_returns_some() {
        let counter = BookmarkCounter::InMemory(AtomicU64::new(0));
        // Use an unlikely tx_id to avoid collisions with other tests.
        let tx_id = 9_876_543_210_u64;
        counter.record_issued_at(tx_id).await;
        let age = counter.issued_age(tx_id).await;
        assert!(age.is_some());
        assert!(age.unwrap() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn issued_age_is_none_for_unknown_tx_id() {
        let counter = BookmarkCounter::InMemory(AtomicU64::new(0));
        // Choose a tx_id that no test records.
        assert!(counter.issued_age(0xDEAD_BEEF_FEED_FACE).await.is_none());
    }

    #[tokio::test]
    async fn redis_store_clear_cached_conn_is_idempotent() {
        // Build a client with an unreachable URL; never connect.
        let client = redis::Client::open("redis://127.0.0.1:0/").unwrap();
        let store = RedisBookmarkStore::new(client);
        store.clear_cached_conn().await;
        store.clear_cached_conn().await;
    }
}
