//! sedandelivery-fabric-sync — async replication from Valkey to Microsoft Fabric Graph.
//!
//! Reads change events from the Valkey stream `__sd2:changelog` and replays them
//! against a `FabricGraphStore` via the Fabric Graph REST API.
//!
//! Exits cleanly when `GQL_FABRIC_ENDPOINT` is not set (no-op sidecar mode).
//! Uses `XREADGROUP` with a consumer group for at-least-once delivery.

mod apply;
mod parse_reply;
mod sync_loop;

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use gql_core::changelog::{changelog_stream_for_graph, CHANGELOG_STREAM};
use gql_fabric_store::FabricGraphStore;
use redis::{Client, Connection};
use tracing::{error, info, warn};

// ── Constants ────────────────────────────────────────────────────────────────

const GROUP_NAME: &str = "sedandelivery-fabric-sync";

/// Graceful shutdown flag — set by SIGTERM/SIGINT handler.
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy)]
pub(crate) struct RedisTimeouts {
    connect_timeout: Duration,
    read_timeout: Duration,
    write_timeout: Duration,
}

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    if std::env::var("GQL_FABRIC_ENDPOINT").is_err() {
        info!("GQL_FABRIC_ENDPOINT not set — Fabric sync disabled, exiting");
        return;
    }

    info!("sedandelivery-fabric-sync starting");

    ctrlc::set_handler(|| {
        SHUTDOWN.store(true, Ordering::Relaxed);
    })
    .expect("failed to install signal handler");

    let batch_size: usize = env_usize("SD2_SYNC_BATCH", 100);
    let block_ms: usize = env_usize("SD2_SYNC_BLOCK_MS", 1000);
    let max_retries: usize = env_usize("SD2_SYNC_MAX_RETRIES", 3);
    let redis_timeouts = redis_timeouts(block_ms);

    let consumer_name = std::env::var("SD2_SYNC_CONSUMER").unwrap_or_else(|_| {
        hostname::get()
            .ok()
            .and_then(|h| h.into_string().ok())
            .unwrap_or_else(|| "worker-0".to_string())
    });

    let fabric = match FabricGraphStore::from_env() {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to initialise FabricGraphStore: {e}");
            std::process::exit(1);
        }
    };

    let redis_url = std::env::var("GQL_REDIS_URL").unwrap_or_else(|_| {
        if std::env::var("GQL_FABRIC_ENDPOINT").is_ok() {
            panic!(
                "GQL_REDIS_URL must be set when GQL_FABRIC_ENDPOINT is configured — \
                 refusing to fall back to localhost for production Fabric sync"
            );
        }
        "redis://127.0.0.1:6379".to_string()
    });
    let redacted_redis_url = redact_url_credentials(&redis_url);

    info!(redis_url = %redacted_redis_url, "Connecting to Redis");

    let client = match Client::open(redis_url.as_str()) {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to open Redis client ({redacted_redis_url}): {e}");
            std::process::exit(1);
        }
    };

    let mut conn = match connect_with_retry(&client, redis_timeouts) {
        Some(Ok(c)) => c,
        Some(Err(e)) => {
            error!("{e}");
            std::process::exit(1);
        }
        None => {
            info!("Shutdown during initial connection — exiting");
            return;
        }
    };

    // Discover per-graph changelog streams.
    // Also ensure the consumer group exists on the legacy global stream for
    // backward compatibility during rolling upgrades.
    let initial_streams = discover_changelog_streams(&mut conn);
    for stream in &initial_streams {
        ensure_consumer_group(&mut conn, stream);
    }

    info!(
        streams = ?initial_streams,
        group = GROUP_NAME,
        consumer = %consumer_name,
        "Consumer ready"
    );

    sync_loop::run_sync_loop(
        &client,
        &mut conn,
        &consumer_name,
        &fabric,
        batch_size,
        block_ms,
        max_retries,
        initial_streams,
        redis_timeouts,
    );

    info!("sedandelivery-fabric-sync shutting down gracefully");
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Discover per-graph changelog streams by listing graphs from the registry
/// and building `__sd2:changelog:{graph}` keys. Also includes the legacy
/// global stream for backward compatibility during rolling upgrades.
fn discover_changelog_streams(conn: &mut Connection) -> Vec<String> {
    let mut streams = Vec::new();

    // Always include the legacy global stream so events published before
    // the per-graph migration are still consumed.
    streams.push(CHANGELOG_STREAM.to_string());

    // Discover graphs from the registry set.
    let graphs: Vec<String> = redis::cmd("SMEMBERS")
        .arg("__sd2_registry:graphs")
        .query(conn)
        .unwrap_or_else(|e| {
            warn!("Failed to discover graphs from registry: {e} — using legacy stream only");
            Vec::new()
        });

    for graph in graphs {
        let stream = changelog_stream_for_graph(&graph);
        if !streams.contains(&stream) {
            streams.push(stream);
        }
    }

    streams
}

/// Ensure the consumer group exists on a stream, creating it (and the stream
/// itself via MKSTREAM) if needed.
fn ensure_consumer_group(conn: &mut Connection, stream: &str) {
    match redis::cmd("XGROUP")
        .arg("CREATE")
        .arg(stream)
        .arg(GROUP_NAME)
        .arg("0")
        .arg("MKSTREAM")
        .query::<()>(conn)
    {
        Ok(()) => info!(stream, "Consumer group created"),
        Err(e) if e.to_string().contains("BUSYGROUP") => {
            info!(stream, "Consumer group already exists")
        }
        Err(e) => {
            error!(stream, "Failed to create consumer group: {e}");
            std::process::exit(1);
        }
    }
}

fn env_usize(key: &str, default: usize) -> usize {
    match std::env::var(key) {
        Err(_) => default,
        Ok(v) => v.parse().unwrap_or_else(|e| {
            warn!("{key}=<invalid> is not a valid usize ({e}), using default {default}");
            default
        }),
    }
}

fn env_duration_ms(key: &str, default_ms: u64) -> Duration {
    Duration::from_millis(match std::env::var(key) {
        Err(_) => default_ms,
        Ok(v) => v.parse().unwrap_or_else(|e| {
            warn!("{key}=<invalid> is not a valid u64 ({e}), using default {default_ms}");
            default_ms
        }),
    })
}

fn redis_timeouts(block_ms: usize) -> RedisTimeouts {
    let default_read_ms = ((block_ms as u64) + 5_000).max(5_000);
    RedisTimeouts {
        connect_timeout: env_duration_ms("SD2_SYNC_REDIS_CONNECT_TIMEOUT_MS", 5_000),
        read_timeout: env_duration_ms("SD2_SYNC_REDIS_READ_TIMEOUT_MS", default_read_ms),
        write_timeout: env_duration_ms("SD2_SYNC_REDIS_WRITE_TIMEOUT_MS", 5_000),
    }
}

fn apply_connection_timeouts(
    conn: Connection,
    timeouts: RedisTimeouts,
) -> Result<Connection, redis::RedisError> {
    conn.set_read_timeout(Some(timeouts.read_timeout))?;
    conn.set_write_timeout(Some(timeouts.write_timeout))?;
    Ok(conn)
}

fn redact_url_credentials(url: &str) -> String {
    let Some((scheme, remainder)) = url.split_once("://") else {
        return url.to_string();
    };

    let authority_end = remainder.find(['/', '?', '#']).unwrap_or(remainder.len());
    let authority = &remainder[..authority_end];
    let suffix = &remainder[authority_end..];

    let Some((userinfo, host)) = authority.rsplit_once('@') else {
        return url.to_string();
    };

    if userinfo.is_empty() {
        return url.to_string();
    }

    format!("{scheme}://***@{host}{suffix}")
}

/// Connect to Redis, retrying with exponential back-off.
/// Returns `None` if SHUTDOWN is signalled during the retry loop.
/// Returns `Some(Err(...))` if max retries are exhausted.
fn connect_with_retry(
    client: &Client,
    timeouts: RedisTimeouts,
) -> Option<Result<Connection, String>> {
    let max_attempts: usize = env_usize("SYNC_MAX_CONNECT_RETRIES", 100);
    let mut delay = Duration::from_millis(500);
    for attempt in 1..=max_attempts {
        if SHUTDOWN.load(Ordering::Relaxed) {
            return None;
        }
        match client.get_connection_with_timeout(timeouts.connect_timeout) {
            Ok(c) => match apply_connection_timeouts(c, timeouts) {
                Ok(c) => {
                    info!(
                        connect_timeout_ms = timeouts.connect_timeout.as_millis(),
                        read_timeout_ms = timeouts.read_timeout.as_millis(),
                        write_timeout_ms = timeouts.write_timeout.as_millis(),
                        "Connected to Valkey"
                    );
                    return Some(Ok(c));
                }
                Err(e) => {
                    warn!(
                        attempt,
                        max_attempts,
                        "Redis connection timeout configuration failed: {e} — retrying in {}ms",
                        delay.as_millis()
                    );
                    thread::sleep(delay);
                    delay = (delay * 2).min(Duration::from_secs(30));
                }
            },
            Err(e) => {
                warn!(
                    attempt,
                    max_attempts,
                    "Redis connection failed: {e} — retrying in {}ms",
                    delay.as_millis()
                );
                thread::sleep(delay);
                delay = (delay * 2).min(Duration::from_secs(30));
            }
        }
    }
    Some(Err(format!(
        "failed to connect to Redis after {max_attempts} attempts"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_usize_returns_default_when_unset() {
        let val = env_usize("__FABRIC_SYNC_TEST_NEVER_SET_12345", 42);
        assert_eq!(val, 42);
    }

    // NOTE: std::env::set_var / remove_var are not thread-safe. In Rust >=1.66
    // they are marked unsafe on some targets. These tests mutate the process
    // environment, which can race with other tests running in parallel. Run
    // with `cargo test -- --test-threads=1` or use a serial test harness
    // (e.g., `serial_test` crate) if flakiness is observed.

    #[test]
    fn env_usize_parses_valid_value() {
        // SAFETY: env::set_var is not thread-safe; see note above.
        unsafe { std::env::set_var("__FABRIC_SYNC_TEST_VALID", "99") };
        let val = env_usize("__FABRIC_SYNC_TEST_VALID", 0);
        assert_eq!(val, 99);
        unsafe { std::env::remove_var("__FABRIC_SYNC_TEST_VALID") };
    }

    #[test]
    fn env_usize_invalid_returns_default() {
        // SAFETY: env::set_var is not thread-safe; see note above.
        unsafe { std::env::set_var("__FABRIC_SYNC_TEST_INVALID", "not_a_number") };
        let val = env_usize("__FABRIC_SYNC_TEST_INVALID", 77);
        assert_eq!(val, 77);
        unsafe { std::env::remove_var("__FABRIC_SYNC_TEST_INVALID") };
    }

    #[test]
    fn redact_url_credentials_preserves_host_and_path() {
        let redacted = redact_url_credentials("redis://user:secret@127.0.0.1:6379/0");
        assert_eq!(redacted, "redis://***@127.0.0.1:6379/0");
    }

    #[test]
    fn redact_url_credentials_handles_password_only_urls() {
        let redacted = redact_url_credentials("rediss://:secret@example.com:6380?tls=true");
        assert_eq!(redacted, "rediss://***@example.com:6380?tls=true");
    }

    #[test]
    fn redact_url_credentials_leaves_credential_free_urls_unchanged() {
        let redacted = redact_url_credentials("redis://127.0.0.1:6379");
        assert_eq!(redacted, "redis://127.0.0.1:6379");
    }
}
