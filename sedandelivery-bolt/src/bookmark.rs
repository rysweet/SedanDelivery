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
        .unwrap_or_default()
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
