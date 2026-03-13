//! Redis distributed lock with per-owner CAS-release.
//!
//! # Protocol
//!
//! Acquire: `SET key token NX PX ttl_ms`
//!   - Returns `Some(token)` on success (lock acquired)
//!   - Returns `None` if key already exists (lock contended)
//!
//! Release: Lua CAS script
//!   - Only deletes the key if its value equals our token
//!   - Returns 1 if released, 0 if someone else holds the lock
//!
//! # Token uniqueness
//!
//! Each acquire call generates a UUID v4 token. This ensures that a
//! slow lock holder that exceeds its TTL cannot accidentally release a
//! lock re-acquired by another holder (the new holder has a different token).

use redis::ConnectionLike;
use uuid::Uuid;

use gql_core::error::StorageError;

/// Lua script for atomic compare-and-release.
///
/// Only deletes the lock key if the stored value equals ARGV[1] (our token).
/// Returns 1 if the key was deleted, 0 if the token did not match or the key
/// did not exist.
const CAS_RELEASE_SCRIPT: &str = r#"
if redis.call("GET", KEYS[1]) == ARGV[1] then
    return redis.call("DEL", KEYS[1])
else
    return 0
end"#;

/// Try to acquire a distributed lock.
///
/// Uses `SET key token NX PX ttl_ms` — atomic lock acquisition with TTL.
/// Returns the unique token string on success, or `None` if the lock is
/// already held.
///
/// # Arguments
///
/// - `conn`   — mutable Redis connection
/// - `key`    — lock key (should be globally unique per resource)
/// - `ttl_ms` — lock TTL in milliseconds (auto-expiry on holder crash)
pub fn acquire(
    conn: &mut dyn ConnectionLike,
    key: &str,
    ttl_ms: u64,
) -> Result<Option<String>, StorageError> {
    let token = Uuid::new_v4().to_string();
    let result: Option<String> = redis::cmd("SET")
        .arg(key)
        .arg(&token)
        .arg("NX")
        .arg("PX")
        .arg(ttl_ms)
        .query(conn)
        .map_err(|e: redis::RedisError| StorageError::Backend { msg: e.to_string() })?;
    // SET with NX returns a bulk-string "OK" on success or nil (None) on contention.
    Ok(result.map(|_| token))
}

/// Release a distributed lock using CAS (compare-and-swap).
///
/// Only deletes the lock key if the stored token matches `token`.  This
/// prevents accidentally releasing a lock re-acquired by another holder
/// after TTL expiry.
///
/// Returns `true` if the lock was released, `false` if the token did not
/// match (the lock was already expired and re-acquired by someone else).
pub fn release(
    conn: &mut dyn ConnectionLike,
    key: &str,
    token: &str,
) -> Result<bool, StorageError> {
    let released: i64 = redis::Script::new(CAS_RELEASE_SCRIPT)
        .key(key)
        .arg(token)
        .invoke(conn)
        .map_err(|e: redis::RedisError| StorageError::Backend { msg: e.to_string() })?;
    Ok(released == 1)
}

/// Write-lock TTL used by all read-modify-write operations (milliseconds).
pub const WRITE_LOCK_TTL_MS: u64 = 5_000;

/// Acquire a distributed lock, retrying up to `max_retries` times with
/// `retry_delay_ms` milliseconds between attempts.
///
/// Returns `Ok(token)` if acquired, or `Err(StorageError::Backend)` if still
/// contended after all retries.
pub(crate) fn acquire_blocking(
    conn: &mut dyn ConnectionLike,
    key: &str,
    ttl_ms: u64,
    max_retries: u32,
    retry_delay_ms: u64,
) -> Result<String, StorageError> {
    for attempt in 0..=max_retries {
        if let Some(token) = acquire(conn, key, ttl_ms)? {
            return Ok(token);
        }
        if attempt < max_retries {
            std::thread::sleep(std::time::Duration::from_millis(retry_delay_ms));
        }
    }
    Err(StorageError::Backend {
        msg: format!("write lock '{key}' still contended after {max_retries} retries"),
    })
}
