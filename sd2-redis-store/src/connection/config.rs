// config — Pool-size constants, socket timeout configuration, and pool stats.

use std::time::Duration;

use redis::RedisError;

/// Snapshot of connection pool utilization.
#[derive(Debug, Clone, Copy)]
pub struct PoolStats {
    /// Total connections managed by the pool (idle + active).
    pub size: u32,
    /// Connections sitting idle in the pool.
    pub idle: u32,
    /// Connections currently checked out by application code.
    pub active: u32,
}

/// Configurable stats reporting interval, read from `GQL_REDIS_STATS_INTERVAL_SECS`
/// env var (default 15, minimum 1, maximum 3600).
pub(crate) fn stats_interval_secs() -> u64 {
    static INTERVAL: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *INTERVAL.get_or_init(|| {
        cypher_core::config::parse_env_range("GQL_REDIS_STATS_INTERVAL_SECS", 15u64, 1, 3600)
    })
}

/// Default connection pool size. Set high enough for concurrent bulk writes
/// (each Bolt session may hold a pool connection during query execution).
pub(crate) const DEFAULT_POOL_SIZE: u32 = 64;
/// Default pool checkout timeout in seconds.
pub(crate) const DEFAULT_POOL_TIMEOUT_SECS: u64 = 5;
/// Default idle timeout in seconds.  Connections idle longer than this are
/// reaped, preventing stale TCP sockets after transient network events.
pub(crate) const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 300;
/// Default minimum idle connections.  Keeps at least this many connections
/// warm so the first requests after an idle period avoid cold-connect latency.
pub(crate) const DEFAULT_MIN_IDLE: u32 = 1;
/// Default Redis TCP connect timeout in milliseconds.
pub(crate) const DEFAULT_CONNECT_TIMEOUT_MS: u64 = 1000;
/// Default Redis socket read timeout in milliseconds.
pub(crate) const DEFAULT_READ_TIMEOUT_MS: u64 = 5000;
/// Default Redis socket write timeout in milliseconds.
pub(crate) const DEFAULT_WRITE_TIMEOUT_MS: u64 = 5000;

#[derive(Debug, Clone, Copy)]
pub(crate) struct RedisSocketTimeouts {
    pub(crate) connect_timeout: Duration,
    pub(crate) read_timeout: Duration,
    pub(crate) write_timeout: Duration,
}

fn socket_connect_timeout_ms() -> u64 {
    static CONNECT_TIMEOUT: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *CONNECT_TIMEOUT.get_or_init(|| {
        cypher_core::config::parse_env_range(
            "GQL_REDIS_CONNECT_TIMEOUT_MS",
            DEFAULT_CONNECT_TIMEOUT_MS,
            1,
            300_000,
        )
    })
}

fn socket_read_timeout_ms() -> u64 {
    static READ_TIMEOUT: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *READ_TIMEOUT.get_or_init(|| {
        cypher_core::config::parse_env_range(
            "GQL_REDIS_READ_TIMEOUT_MS",
            DEFAULT_READ_TIMEOUT_MS,
            1,
            300_000,
        )
    })
}

fn socket_write_timeout_ms() -> u64 {
    static WRITE_TIMEOUT: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *WRITE_TIMEOUT.get_or_init(|| {
        cypher_core::config::parse_env_range(
            "GQL_REDIS_WRITE_TIMEOUT_MS",
            DEFAULT_WRITE_TIMEOUT_MS,
            1,
            300_000,
        )
    })
}

pub(crate) fn socket_timeouts() -> RedisSocketTimeouts {
    RedisSocketTimeouts {
        connect_timeout: Duration::from_millis(socket_connect_timeout_ms()),
        read_timeout: Duration::from_millis(socket_read_timeout_ms()),
        write_timeout: Duration::from_millis(socket_write_timeout_ms()),
    }
}

pub(crate) fn closed_connection_error() -> RedisError {
    RedisError::from(std::io::Error::from(std::io::ErrorKind::BrokenPipe))
}
