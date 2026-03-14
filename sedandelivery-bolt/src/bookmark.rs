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

/// Global bookmark counter, backed by Redis or an in-process atomic.
pub enum BookmarkCounter {
    /// In-process counter (no Redis).
    InMemory(AtomicU64),
    /// Redis-backed counter using `INCR __sd2_bookmark`.
    Redis(redis::Client),
}

/// Redis key for the global bookmark counter.
const BOOKMARK_KEY: &str = "__sd2_bookmark";
const BOOKMARK_TS_KEY_PREFIX: &str = "__sd2_bookmark_ts:";
const RECENT_BOOKMARK_TTL_SECS: u64 = 30;

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
                    return BookmarkCounter::Redis(client);
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
    pub fn next(&self) -> u64 {
        match self {
            BookmarkCounter::InMemory(counter) => counter.fetch_add(1, Ordering::SeqCst) + 1,
            BookmarkCounter::Redis(client) => match client.get_connection() {
                Ok(mut conn) => {
                    match redis::cmd("INCR").arg(BOOKMARK_KEY).query::<i64>(&mut conn) {
                        Ok(n) => n as u64,
                        Err(e) => {
                            tracing::warn!(
                                "bookmark Redis INCR failed ({e}), using local fallback"
                            );
                            static FALLBACK: AtomicU64 = AtomicU64::new(0);
                            FALLBACK.fetch_add(1, Ordering::SeqCst) + 1
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("bookmark Redis connection failed ({e}), using local fallback");
                    static FALLBACK: AtomicU64 = AtomicU64::new(0);
                    FALLBACK.fetch_add(1, Ordering::SeqCst) + 1
                }
            },
        }
    }

    pub fn record_issued_at(&self, tx_id: u64) {
        let mut local = local_bookmark_timestamps()
            .lock()
            .expect("bookmark timestamp mutex poisoned");
        local.retain(|_, issued_at| {
            issued_at.elapsed() <= Duration::from_secs(RECENT_BOOKMARK_TTL_SECS)
        });
        local.insert(tx_id, Instant::now());
        drop(local);

        if let BookmarkCounter::Redis(client) = self {
            let timestamp = now_unix_millis();
            match client.get_connection() {
                Ok(mut conn) => {
                    let result: redis::RedisResult<()> = redis::cmd("SET")
                        .arg(bookmark_timestamp_key(tx_id))
                        .arg(timestamp)
                        .arg("EX")
                        .arg(RECENT_BOOKMARK_TTL_SECS)
                        .query(&mut conn);
                    if let Err(err) = result {
                        tracing::warn!(
                            tx_id,
                            error = %err,
                            "bookmark timestamp SET failed"
                        );
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        tx_id,
                        error = %err,
                        "bookmark timestamp Redis connection failed"
                    );
                }
            }
        }
    }

    pub fn issued_age(&self, tx_id: u64) -> Option<Duration> {
        if let Some(age) = local_bookmark_timestamps()
            .lock()
            .expect("bookmark timestamp mutex poisoned")
            .get(&tx_id)
            .map(Instant::elapsed)
        {
            return Some(age);
        }

        if let BookmarkCounter::Redis(client) = self {
            match client.get_connection() {
                Ok(mut conn) => {
                    let value: redis::RedisResult<Option<u64>> = redis::cmd("GET")
                        .arg(bookmark_timestamp_key(tx_id))
                        .query(&mut conn);
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
                        }
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        tx_id,
                        error = %err,
                        "bookmark timestamp Redis connection failed"
                    );
                }
            }
        }

        None
    }
}

// BookmarkCounter is not Clone, but it can be put behind Arc<BookmarkCounter>.
// Safety: AtomicU64 is Send+Sync; redis::Client is Send+Sync.
unsafe impl Send for BookmarkCounter {}
unsafe impl Sync for BookmarkCounter {}
