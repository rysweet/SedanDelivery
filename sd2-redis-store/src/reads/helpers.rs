// Helpers shared across all read submodules: SMEMBERS/SSCAN paging,
// thresholds, and lock-renewing iteration.

use cypher_core::error::StorageError;

use crate::LockedRedisConn;

/// Default member count threshold above which SSCAN is used instead of SMEMBERS.
/// Sets with fewer members than this use the faster SMEMBERS. Configurable via
/// `GQL_REDIS_SMEMBERS_THRESHOLD` environment variable.
pub(crate) fn smembers_threshold() -> u64 {
    static THRESHOLD: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *THRESHOLD.get_or_init(|| {
        cypher_core::config::parse_env_min("GQL_REDIS_SMEMBERS_THRESHOLD", 10_000u64, 1)
    })
}

/// SSCAN page size for large-set iteration.
pub(super) const SSCAN_COUNT: u64 = 1000;

/// Read all members of a Redis set, using SMEMBERS for small sets and SSCAN
/// for large ones to avoid blocking the Redis shard.
///
/// Checks `SCARD` first; if the set has more members than `threshold`, iterates
/// with SSCAN using COUNT 1000. Otherwise uses a single SMEMBERS call.
pub(crate) fn smembers_or_scan(
    conn: &mut dyn redis::ConnectionLike,
    key: &str,
    threshold: u64,
) -> Result<Vec<String>, StorageError> {
    let card: u64 = redis::cmd("SCARD")
        .arg(key)
        .query(conn)
        .map_err(crate::backend_storage_error)?;

    if card <= threshold {
        let members: Vec<String> = redis::cmd("SMEMBERS")
            .arg(key)
            .query(conn)
            .map_err(crate::backend_storage_error)?;
        return Ok(members);
    }

    // Large set: iterate with SSCAN to avoid blocking.
    let mut cursor: u64 = 0;
    let mut members = Vec::with_capacity(card as usize);
    loop {
        let (next_cursor, batch): (u64, Vec<String>) = redis::cmd("SSCAN")
            .arg(key)
            .arg(cursor)
            .arg("COUNT")
            .arg(SSCAN_COUNT)
            .query(conn)
            .map_err(crate::backend_storage_error)?;
        members.extend(batch);
        cursor = next_cursor;
        if cursor == 0 {
            break;
        }
    }
    Ok(members)
}

pub(super) fn sscan_all_members(
    conn: &mut dyn redis::ConnectionLike,
    key: &str,
    capacity: usize,
) -> Result<Vec<String>, StorageError> {
    let mut cursor: u64 = 0;
    let mut members = Vec::with_capacity(capacity);
    loop {
        let (next_cursor, batch): (u64, Vec<String>) = redis::cmd("SSCAN")
            .arg(key)
            .arg(cursor)
            .arg("COUNT")
            .arg(SSCAN_COUNT)
            .query(conn)
            .map_err(crate::backend_storage_error)?;
        members.extend(batch);
        cursor = next_cursor;
        if cursor == 0 {
            break;
        }
    }
    Ok(members)
}

/// Visit set members in bounded pages while renewing the caller's write lock
/// between SSCAN batches.
pub(crate) fn for_each_set_page_locked<F>(
    conn: &mut LockedRedisConn,
    key: &str,
    threshold: u64,
    mut visit: F,
) -> Result<(), StorageError>
where
    F: FnMut(&mut LockedRedisConn, Vec<String>) -> Result<(), StorageError>,
{
    let card: u64 = redis::cmd("SCARD")
        .arg(key)
        .query(conn.as_mut())
        .map_err(crate::backend_storage_error)?;

    if card <= threshold {
        let members: Vec<String> = redis::cmd("SMEMBERS")
            .arg(key)
            .query(conn.as_mut())
            .map_err(crate::backend_storage_error)?;
        return visit(conn, members);
    }

    let mut cursor: u64 = 0;
    loop {
        conn.renew_lock()?;
        let (next_cursor, batch): (u64, Vec<String>) = redis::cmd("SSCAN")
            .arg(key)
            .arg(cursor)
            .arg("COUNT")
            .arg(SSCAN_COUNT)
            .query(conn.as_mut())
            .map_err(crate::backend_storage_error)?;
        visit(conn, batch)?;
        cursor = next_cursor;
        if cursor == 0 {
            break;
        }
    }
    Ok(())
}
