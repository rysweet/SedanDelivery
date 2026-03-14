//! Redis-backed global bookmark counter for cross-replica consistency.
//!
//! When Redis is configured (`GQL_REDIS_URL` or `REDIS_URL`),
//! the counter uses `INCR __sd2_bookmark` so all replicas share the same
//! monotonic sequence.  When Redis is not configured (local dev / tests),
//! falls back to an in-process `AtomicU64`.

use std::sync::atomic::{AtomicU64, Ordering};

/// Global bookmark counter, backed by Redis or an in-process atomic.
pub enum BookmarkCounter {
    /// In-process counter (no Redis).
    InMemory(AtomicU64),
    /// Redis-backed counter using `INCR __sd2_bookmark`.
    Redis(redis::Client),
}

/// Redis key for the global bookmark counter.
const BOOKMARK_KEY: &str = "__sd2_bookmark";

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
}

// BookmarkCounter is not Clone, but it can be put behind Arc<BookmarkCounter>.
// Safety: AtomicU64 is Send+Sync; redis::Client is Send+Sync.
unsafe impl Send for BookmarkCounter {}
unsafe impl Sync for BookmarkCounter {}
