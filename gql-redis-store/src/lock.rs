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

use cypher_core::error::StorageError;

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

/// Lua script for atomic compare-and-renew.
///
/// Only refreshes the lock TTL if the stored value equals ARGV[1] (our token).
/// Returns 1 if the TTL was refreshed, 0 if the token did not match or the key
/// did not exist.
const CAS_RENEW_SCRIPT: &str = r#"
if redis.call("GET", KEYS[1]) == ARGV[1] then
    return redis.call("PEXPIRE", KEYS[1], ARGV[2])
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
        .map_err(|e: redis::RedisError| crate::backend_storage_error(e))?;
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
        .map_err(|e: redis::RedisError| crate::backend_storage_error(e))?;
    Ok(released == 1)
}

/// Renew a distributed lock using CAS (compare-and-renew).
///
/// Only refreshes the TTL if the stored token matches `token`. This prevents a
/// slow or expired lock holder from extending a lock that another writer has
/// already acquired after TTL expiry.
///
/// Returns `true` if the TTL was refreshed, `false` if the token did not
/// match (the lock was already expired and re-acquired by someone else).
pub fn renew(
    conn: &mut dyn ConnectionLike,
    key: &str,
    token: &str,
    ttl_ms: u64,
) -> Result<bool, StorageError> {
    let renewed: i64 = redis::Script::new(CAS_RENEW_SCRIPT)
        .key(key)
        .arg(token)
        .arg(ttl_ms)
        .invoke(conn)
        .map_err(|e: redis::RedisError| crate::backend_storage_error(e))?;
    Ok(renewed == 1)
}

/// Write-lock TTL used by all read-modify-write operations (milliseconds).
pub const WRITE_LOCK_TTL_MS: u64 = 5_000;

/// Batch writes keep graph-level serialization but tolerate a longer lock wait.
///
/// Concurrent UNWIND CREATE requests can hold the same-graph write lock long
/// enough that the default 3x50 ms retry window fails under normal load.
pub const BATCH_WRITE_MAX_RETRIES: u32 = 12;
pub const BATCH_WRITE_RETRY_DELAY_MS: u64 = 100;

#[cfg(test)]
mod tests {
    use super::*;

    use redis::{Arg, Cmd, RedisResult, Value};

    #[derive(Debug, Default)]
    struct MockConnection {
        owner: Option<String>,
        open: bool,
    }

    impl MockConnection {
        fn force_owner(&mut self, owner: &str) {
            self.owner = Some(owner.to_string());
        }

        fn command_args(cmd: &Cmd) -> Vec<String> {
            cmd.args_iter()
                .map(|arg| match arg {
                    Arg::Simple(bytes) => String::from_utf8(bytes.to_vec()).unwrap(),
                    Arg::Cursor => "<cursor>".to_string(),
                    _ => unreachable!("redis::Arg only exposes simple and cursor variants here"),
                })
                .collect()
        }

        fn handle_set(&mut self, args: &[String]) -> RedisResult<Value> {
            assert_eq!(args[0], "SET");
            assert_eq!(args[3], "NX");
            assert_eq!(args[4], "PX");
            let token = args[2].clone();
            if self.owner.is_none() {
                self.owner = Some(token);
                Ok(Value::Okay)
            } else {
                Ok(Value::Nil)
            }
        }

        fn handle_evalsha(&mut self, args: &[String]) -> RedisResult<Value> {
            assert_eq!(args[0], "EVALSHA");
            match args.len() {
                5 => {
                    let token = &args[4];
                    let matches = self.owner.as_deref() == Some(token);
                    if matches {
                        self.owner = None;
                    }
                    Ok(Value::Int(i64::from(matches)))
                }
                6 => {
                    let token = &args[4];
                    Ok(Value::Int(i64::from(self.owner.as_deref() == Some(token))))
                }
                _ => panic!("unexpected script args: {args:?}"),
            }
        }
    }

    impl ConnectionLike for MockConnection {
        fn req_packed_command(&mut self, _cmd: &[u8]) -> RedisResult<Value> {
            panic!("packed command path should not be used in lock tests")
        }

        fn req_packed_commands(
            &mut self,
            _cmd: &[u8],
            _offset: usize,
            _count: usize,
        ) -> RedisResult<Vec<Value>> {
            panic!("pipeline path should not be used in lock tests")
        }

        fn req_command(&mut self, cmd: &Cmd) -> RedisResult<Value> {
            let args = Self::command_args(cmd);
            match args.first().map(String::as_str) {
                Some("SET") => self.handle_set(&args),
                Some("EVALSHA") => self.handle_evalsha(&args),
                Some("SCRIPT") if args.get(1).map(String::as_str) == Some("LOAD") => {
                    Ok(Value::BulkString(b"mock-sha".to_vec()))
                }
                other => panic!("unexpected redis command in test: {other:?} ({args:?})"),
            }
        }

        fn get_db(&self) -> i64 {
            0
        }

        fn check_connection(&mut self) -> bool {
            self.open
        }

        fn is_open(&self) -> bool {
            self.open
        }
    }

    #[test]
    fn renew_returns_false_after_lock_owner_changes() {
        let mut conn = MockConnection {
            owner: None,
            open: true,
        };
        let token = acquire(&mut conn, "G:{graph}:lock", WRITE_LOCK_TTL_MS)
            .unwrap()
            .expect("lock should be acquired");
        conn.force_owner("someone-else");

        let renewed = renew(&mut conn, "G:{graph}:lock", &token, WRITE_LOCK_TTL_MS).unwrap();

        assert!(!renewed, "renew must fail closed after ownership changes");
    }

    #[test]
    fn release_returns_false_after_lock_owner_changes() {
        let mut conn = MockConnection {
            owner: None,
            open: true,
        };
        let token = acquire(&mut conn, "G:{graph}:lock", WRITE_LOCK_TTL_MS)
            .unwrap()
            .expect("lock should be acquired");
        conn.force_owner("someone-else");

        let released = release(&mut conn, "G:{graph}:lock", &token).unwrap();

        assert!(
            !released,
            "release must fail closed after ownership changes"
        );
    }

    #[test]
    fn release_clears_owner_for_reacquire() {
        let mut conn = MockConnection {
            owner: None,
            open: true,
        };
        let token = acquire(&mut conn, "G:{graph}:lock", WRITE_LOCK_TTL_MS)
            .unwrap()
            .expect("lock should be acquired");

        let released = release(&mut conn, "G:{graph}:lock", &token).unwrap();
        assert!(
            released,
            "release should succeed while the token still owns the lock"
        );

        let reacquired = acquire(&mut conn, "G:{graph}:lock", WRITE_LOCK_TTL_MS)
            .unwrap()
            .expect("lock should be reacquirable after a successful release");
        assert!(
            !reacquired.is_empty(),
            "reacquire should return a fresh owner token"
        );
    }
}
