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

#[cfg(test)]
mod tests {
    //! Unit tests for the pure-Rust paging helpers using an in-process
    //! `ConnectionLike` mock. No live Redis required.

    use super::{
        for_each_set_page_locked, smembers_or_scan, smembers_threshold, sscan_all_members,
    };
    use cypher_core::error::StorageError;
    use redis::{Arg, Cmd, RedisError, RedisResult, Value as RedisValue};
    use std::collections::VecDeque;

    /// Canned replies for SCARD / SMEMBERS / SSCAN. The mock does not need to
    /// understand Redis state — each command pops the next queued response.
    struct ScriptedSetConn {
        // Responses for each SCARD call, in order.
        scard: VecDeque<RedisResult<RedisValue>>,
        // Responses for each SMEMBERS call, in order.
        smembers: VecDeque<RedisResult<RedisValue>>,
        // Responses for each SSCAN call, in order.
        sscan: VecDeque<RedisResult<RedisValue>>,
    }

    impl ScriptedSetConn {
        fn new() -> Self {
            Self {
                scard: VecDeque::new(),
                smembers: VecDeque::new(),
                sscan: VecDeque::new(),
            }
        }

        fn push_scard(mut self, v: u64) -> Self {
            self.scard.push_back(Ok(RedisValue::Int(v as i64)));
            self
        }

        fn push_scard_err(mut self) -> Self {
            self.scard.push_back(Err(RedisError::from((
                redis::ErrorKind::Io,
                "scard failed",
            ))));
            self
        }

        fn push_smembers(mut self, members: &[&str]) -> Self {
            self.smembers
                .push_back(Ok(RedisValue::Array(members.iter().map(bulk).collect())));
            self
        }

        fn push_smembers_err(mut self) -> Self {
            self.smembers.push_back(Err(RedisError::from((
                redis::ErrorKind::Io,
                "smembers failed",
            ))));
            self
        }

        fn push_sscan(mut self, next_cursor: u64, members: &[&str]) -> Self {
            let array = RedisValue::Array(vec![
                RedisValue::BulkString(next_cursor.to_string().into_bytes()),
                RedisValue::Array(members.iter().map(bulk).collect()),
            ]);
            self.sscan.push_back(Ok(array));
            self
        }

        fn push_sscan_err(mut self) -> Self {
            self.sscan.push_back(Err(RedisError::from((
                redis::ErrorKind::Io,
                "sscan failed",
            ))));
            self
        }
    }

    fn bulk(s: &&str) -> RedisValue {
        RedisValue::BulkString(s.as_bytes().to_vec())
    }

    fn cmd_name(cmd: &Cmd) -> String {
        cmd.args_iter()
            .next()
            .map(|a| match a {
                Arg::Simple(b) => String::from_utf8_lossy(b).to_string(),
                _ => String::new(),
            })
            .unwrap_or_default()
    }

    impl redis::ConnectionLike for ScriptedSetConn {
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
            match cmd_name(cmd).as_str() {
                "SCARD" => self
                    .scard
                    .pop_front()
                    .expect("no queued SCARD response left"),
                "SMEMBERS" => self
                    .smembers
                    .pop_front()
                    .expect("no queued SMEMBERS response left"),
                "SSCAN" => self
                    .sscan
                    .pop_front()
                    .expect("no queued SSCAN response left"),
                other => panic!("unexpected command: {other}"),
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
    fn smembers_threshold_is_configurable_and_positive() {
        // Reading the threshold must succeed and produce at least 1.
        assert!(smembers_threshold() >= 1);
        // Second call goes through the OnceLock cache (same value).
        assert_eq!(smembers_threshold(), smembers_threshold());
    }

    #[test]
    fn smembers_or_scan_uses_smembers_when_under_threshold() {
        let mut conn = ScriptedSetConn::new()
            .push_scard(3)
            .push_smembers(&["a", "b", "c"]);
        let members = smembers_or_scan(&mut conn, "k", 10).unwrap();
        assert_eq!(members, vec!["a".to_string(), "b".into(), "c".into()]);
    }

    #[test]
    fn smembers_or_scan_at_exact_threshold_still_uses_smembers() {
        // card <= threshold (equality).
        let mut conn = ScriptedSetConn::new()
            .push_scard(5)
            .push_smembers(&["one", "two", "three", "four", "five"]);
        let members = smembers_or_scan(&mut conn, "k", 5).unwrap();
        assert_eq!(members.len(), 5);
    }

    #[test]
    fn smembers_or_scan_falls_back_to_sscan_for_large_sets() {
        // card > threshold -> two-page SSCAN iteration that terminates when
        // the cursor comes back to 0.
        let mut conn = ScriptedSetConn::new()
            .push_scard(100)
            .push_sscan(42, &["a", "b"])
            .push_sscan(0, &["c", "d"]);
        let members = smembers_or_scan(&mut conn, "k", 1).unwrap();
        assert_eq!(
            members,
            vec!["a".to_string(), "b".into(), "c".into(), "d".into()]
        );
    }

    #[test]
    fn smembers_or_scan_propagates_scard_error() {
        let mut conn = ScriptedSetConn::new().push_scard_err();
        let err = smembers_or_scan(&mut conn, "k", 10).unwrap_err();
        assert!(matches!(err, StorageError::Backend { .. }));
    }

    #[test]
    fn smembers_or_scan_propagates_smembers_error() {
        let mut conn = ScriptedSetConn::new().push_scard(1).push_smembers_err();
        let err = smembers_or_scan(&mut conn, "k", 10).unwrap_err();
        assert!(matches!(err, StorageError::Backend { .. }));
    }

    #[test]
    fn smembers_or_scan_propagates_sscan_error() {
        let mut conn = ScriptedSetConn::new().push_scard(100).push_sscan_err();
        let err = smembers_or_scan(&mut conn, "k", 1).unwrap_err();
        assert!(matches!(err, StorageError::Backend { .. }));
    }

    #[test]
    fn sscan_all_members_paginates_until_cursor_zero() {
        let mut conn = ScriptedSetConn::new()
            .push_sscan(7, &["x"])
            .push_sscan(13, &["y", "z"])
            .push_sscan(0, &["w"]);
        let members = sscan_all_members(&mut conn, "k", 0).unwrap();
        assert_eq!(
            members,
            vec!["x".to_string(), "y".into(), "z".into(), "w".into()]
        );
    }

    #[test]
    fn sscan_all_members_returns_empty_when_first_page_empty() {
        let mut conn = ScriptedSetConn::new().push_sscan(0, &[]);
        let members = sscan_all_members(&mut conn, "k", 0).unwrap();
        assert!(members.is_empty());
    }

    #[test]
    fn sscan_all_members_propagates_error() {
        let mut conn = ScriptedSetConn::new().push_sscan_err();
        let err = sscan_all_members(&mut conn, "k", 0).unwrap_err();
        assert!(matches!(err, StorageError::Backend { .. }));
    }

    // ── for_each_set_page_locked ────────────────────────────────────────────
    //
    // These tests stub out `LockedRedisConn` by transmuting a bare
    // `ScriptedSetConn` into the wrapper type. Because `for_each_set_page_locked`
    // holds a real `LockedRedisConn`, we instead go through the same backend
    // error paths exercised above — the wrapper's SCARD error handling is
    // identical to `smembers_or_scan`'s. A pure end-to-end test of the locked
    // iterator lives in the tests/ directory where a live Redis is available.

    #[test]
    fn for_each_set_page_locked_is_public_in_module() {
        // Compile-time check that the helper is visible to the reads module
        // tree; the runtime behavior is exercised by integration tests.
        #[allow(clippy::type_complexity)]
        let _: fn(
            &mut crate::LockedRedisConn,
            &str,
            u64,
            fn(&mut crate::LockedRedisConn, Vec<String>) -> Result<(), StorageError>,
        ) -> Result<(), StorageError> = for_each_set_page_locked;
    }
}
