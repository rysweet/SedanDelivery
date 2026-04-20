//! Redis memory metrics polling.
//!
//! [`poll_once`] issues `INFO memory` and `INFO stats` to a Redis connection
//! and parses the key/value pairs into a [`MemoryInfo`] struct.  The caller
//! is responsible for scheduling periodic polls (e.g. via a tokio interval).

use redis::ConnectionLike;

use cypher_core::error::StorageError;

/// Parsed Redis memory usage snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryInfo {
    /// Bytes currently allocated by Redis (`used_memory` field).
    pub used_memory: u64,
    /// Maximum memory limit configured in Redis (0 = unlimited).
    pub maxmemory: u64,
    /// Total number of evicted keys since server start (`evicted_keys` field).
    pub evicted_keys: u64,
}

/// Poll `INFO memory` (and `INFO stats`) from a Redis connection.
///
/// Parses the colon-separated key/value output.  Unknown lines are silently
/// skipped, so this is forward-compatible with new Redis INFO fields.
pub fn poll_once(conn: &mut dyn ConnectionLike) -> Result<MemoryInfo, StorageError> {
    let memory_raw: String = redis::cmd("INFO")
        .arg("memory")
        .query(conn)
        .map_err(crate::backend_storage_error)?;

    let mut used_memory: u64 = 0;
    let mut maxmemory: u64 = 0;

    for line in memory_raw.lines() {
        if let Some((key, val)) = line.split_once(':') {
            match key.trim() {
                "used_memory" => {
                    used_memory = val.trim().parse().unwrap_or(0);
                }
                "maxmemory" => {
                    maxmemory = val.trim().parse().unwrap_or(0);
                }
                _ => {}
            }
        }
    }

    let evicted_keys = poll_evicted_keys(conn)?;

    Ok(MemoryInfo {
        used_memory,
        maxmemory,
        evicted_keys,
    })
}

/// Read `evicted_keys` from `INFO stats`.
fn poll_evicted_keys(conn: &mut dyn ConnectionLike) -> Result<u64, StorageError> {
    let stats_raw: String = redis::cmd("INFO")
        .arg("stats")
        .query(conn)
        .map_err(crate::backend_storage_error)?;

    for line in stats_raw.lines() {
        if let Some((key, val)) = line.split_once(':') {
            if key.trim() == "evicted_keys" {
                return Ok(val.trim().parse().unwrap_or(0));
            }
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    //! Unit tests for [`poll_once`] using an in-process `ConnectionLike` mock
    //! that returns canned bodies for `INFO memory` / `INFO stats`.

    use super::*;
    use cypher_core::error::StorageError;
    use redis::{Arg, Cmd, RedisError, RedisResult, Value as RedisValue};

    /// A minimal [`ConnectionLike`] that returns the next queued response for
    /// each `INFO <section>` command. Panics on unexpected commands.
    struct ScriptedInfoConn {
        /// Response for `INFO memory`.
        memory: RedisResult<RedisValue>,
        /// Response for `INFO stats`.
        stats: RedisResult<RedisValue>,
    }

    impl ScriptedInfoConn {
        fn new_ok(memory_body: &str, stats_body: &str) -> Self {
            Self {
                memory: Ok(RedisValue::BulkString(memory_body.as_bytes().to_vec())),
                stats: Ok(RedisValue::BulkString(stats_body.as_bytes().to_vec())),
            }
        }

        fn section(cmd: &Cmd) -> String {
            let args: Vec<String> = cmd
                .args_iter()
                .filter_map(|a| match a {
                    Arg::Simple(b) => Some(String::from_utf8_lossy(b).to_string()),
                    _ => None,
                })
                .collect();
            args.get(1).cloned().unwrap_or_default()
        }
    }

    impl ConnectionLike for ScriptedInfoConn {
        fn req_packed_command(&mut self, _cmd: &[u8]) -> RedisResult<RedisValue> {
            panic!("packed_command not expected");
        }
        fn req_packed_commands(
            &mut self,
            _cmd: &[u8],
            _offset: usize,
            _count: usize,
        ) -> RedisResult<Vec<RedisValue>> {
            panic!("packed_commands not expected");
        }
        fn req_command(&mut self, cmd: &Cmd) -> RedisResult<RedisValue> {
            let args: Vec<String> = cmd
                .args_iter()
                .filter_map(|a| match a {
                    Arg::Simple(b) => Some(String::from_utf8_lossy(b).to_string()),
                    _ => None,
                })
                .collect();
            assert_eq!(args.first().map(String::as_str), Some("INFO"));
            match Self::section(cmd).as_str() {
                "memory" => self.memory.clone(),
                "stats" => self.stats.clone(),
                other => panic!("unexpected INFO section: {other}"),
            }
        }
        fn get_db(&self) -> i64 {
            0
        }
        fn check_connection(&mut self) -> bool {
            true
        }
        fn is_open(&self) -> bool {
            true
        }
    }

    #[test]
    fn poll_once_parses_memory_and_stats() {
        let memory = "# Memory\nused_memory:2048\nmaxmemory:4096\nsome_other:ignored\n";
        let stats = "# Stats\nevicted_keys:7\nunrelated:99\n";
        let mut conn = ScriptedInfoConn::new_ok(memory, stats);
        let info = poll_once(&mut conn).unwrap();
        assert_eq!(
            info,
            MemoryInfo {
                used_memory: 2048,
                maxmemory: 4096,
                evicted_keys: 7,
            }
        );
    }

    #[test]
    fn poll_once_missing_fields_default_to_zero() {
        // No used_memory / maxmemory / evicted_keys lines at all.
        let mut conn = ScriptedInfoConn::new_ok("# Memory\n", "# Stats\n");
        let info = poll_once(&mut conn).unwrap();
        assert_eq!(info.used_memory, 0);
        assert_eq!(info.maxmemory, 0);
        assert_eq!(info.evicted_keys, 0);
    }

    #[test]
    fn poll_once_unparseable_values_become_zero() {
        // Values that fail u64::parse must fall through to the `unwrap_or(0)`
        // branches instead of propagating as errors.
        let memory = "used_memory:not-a-number\nmaxmemory:also-bad\n";
        let stats = "evicted_keys:NaN\n";
        let mut conn = ScriptedInfoConn::new_ok(memory, stats);
        let info = poll_once(&mut conn).unwrap();
        assert_eq!(info.used_memory, 0);
        assert_eq!(info.maxmemory, 0);
        assert_eq!(info.evicted_keys, 0);
    }

    #[test]
    fn poll_once_ignores_lines_without_colon() {
        // Lines without a `:` separator (comments / blanks) are silently skipped.
        let memory = "# a Redis INFO comment\n\nused_memory:100\n";
        let stats = "# comment\nevicted_keys:3\n";
        let mut conn = ScriptedInfoConn::new_ok(memory, stats);
        let info = poll_once(&mut conn).unwrap();
        assert_eq!(info.used_memory, 100);
        assert_eq!(info.evicted_keys, 3);
    }

    #[test]
    fn poll_once_surfaces_backend_error_on_memory() {
        let mut conn = ScriptedInfoConn {
            memory: Err(RedisError::from((redis::ErrorKind::Io, "boom"))),
            stats: Ok(RedisValue::BulkString(b"evicted_keys:0\n".to_vec())),
        };
        let err = poll_once(&mut conn).unwrap_err();
        assert!(matches!(err, StorageError::Backend { .. }));
    }

    #[test]
    fn poll_once_surfaces_backend_error_on_stats() {
        let mut conn = ScriptedInfoConn {
            memory: Ok(RedisValue::BulkString(b"used_memory:1\n".to_vec())),
            stats: Err(RedisError::from((redis::ErrorKind::Io, "stats failed"))),
        };
        let err = poll_once(&mut conn).unwrap_err();
        assert!(matches!(err, StorageError::Backend { .. }));
    }

    #[test]
    fn memory_info_derives_work() {
        // Exercise the Debug / Clone / PartialEq impls so the struct is fully
        // covered even if users don't read fields directly.
        let a = MemoryInfo {
            used_memory: 1,
            maxmemory: 2,
            evicted_keys: 3,
        };
        let b = a.clone();
        assert_eq!(a, b);
        let _ = format!("{a:?}");
    }
}
