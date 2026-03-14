//! Redis memory metrics polling.
//!
//! [`poll_once`] issues `INFO memory` and `INFO stats` to a Redis connection
//! and parses the key/value pairs into a [`MemoryInfo`] struct.  The caller
//! is responsible for scheduling periodic polls (e.g. via a tokio interval).

use redis::ConnectionLike;

use gql_core::error::StorageError;

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
