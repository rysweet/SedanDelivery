//! Redis connection management — connect-with-retry, timeout application,
//! changelog stream discovery, and consumer group bootstrap.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use cypher_core::changelog::{changelog_stream_for_graph, CHANGELOG_STREAM};
use redis::{Client, Connection};
use tracing::{error, info, warn};

use crate::config::{env_usize, RedisTimeouts, GROUP_NAME};

/// Graceful shutdown flag — set by SIGTERM/SIGINT handler.
pub(crate) static SHUTDOWN: AtomicBool = AtomicBool::new(false);

/// Discover per-graph changelog streams by listing graphs from the registry
/// and building `__sd2:changelog:{graph}` keys. Also includes the legacy
/// global stream for backward compatibility during rolling upgrades.
pub(crate) fn discover_changelog_streams(conn: &mut Connection) -> Vec<String> {
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

    let mut seen: HashSet<String> = streams.iter().cloned().collect();
    for graph in graphs {
        let stream = changelog_stream_for_graph(&graph);
        if seen.insert(stream.clone()) {
            streams.push(stream);
        }
    }

    streams
}

/// Ensure the consumer group exists on a stream, creating it (and the stream
/// itself via MKSTREAM) if needed.
pub(crate) fn ensure_consumer_group(conn: &mut Connection, stream: &str) {
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

fn apply_connection_timeouts(
    conn: Connection,
    timeouts: RedisTimeouts,
) -> Result<Connection, redis::RedisError> {
    conn.set_read_timeout(Some(timeouts.read_timeout))?;
    conn.set_write_timeout(Some(timeouts.write_timeout))?;
    Ok(conn)
}

/// Connect to Redis, retrying with exponential back-off.
/// Returns `None` if SHUTDOWN is signalled during the retry loop.
/// Returns `Some(Err(...))` if max retries are exhausted.
pub(crate) fn connect_with_retry(
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
