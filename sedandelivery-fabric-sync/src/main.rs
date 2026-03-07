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

use gql_core::changelog::CHANGELOG_STREAM;
use gql_fabric_store::FabricGraphStore;
use redis::{Client, Connection};
use tracing::{error, info, warn};

// ── Constants ────────────────────────────────────────────────────────────────

const GROUP_NAME: &str = "sedandelivery-fabric-sync";

/// Graceful shutdown flag — set by SIGTERM/SIGINT handler.
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

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

    info!(redis_url = %redis_url, "Connecting to Redis");

    let client = match Client::open(redis_url.as_str()) {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to open Redis client ({redis_url}): {e}");
            std::process::exit(1);
        }
    };

    let mut conn = match connect_with_retry(&client) {
        Some(c) => c,
        None => {
            info!("Shutdown during initial connection — exiting");
            return;
        }
    };

    match redis::cmd("XGROUP")
        .arg("CREATE")
        .arg(CHANGELOG_STREAM)
        .arg(GROUP_NAME)
        .arg("0")
        .arg("MKSTREAM")
        .query::<()>(&mut conn)
    {
        Ok(()) => info!("Consumer group created"),
        Err(e) if e.to_string().contains("BUSYGROUP") => {
            info!("Consumer group already exists")
        }
        Err(e) => {
            error!("Failed to create consumer group: {e}");
            std::process::exit(1);
        }
    }

    info!(
        stream = CHANGELOG_STREAM,
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
    );

    info!("sedandelivery-fabric-sync shutting down gracefully");
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn env_usize(key: &str, default: usize) -> usize {
    match std::env::var(key) {
        Err(_) => default,
        Ok(v) => v.parse().unwrap_or_else(|e| {
            warn!("{key}=<invalid> is not a valid usize ({e}), using default {default}");
            default
        }),
    }
}

/// Connect to Redis, retrying with exponential back-off.
/// Returns `None` if SHUTDOWN is signalled during the retry loop.
fn connect_with_retry(client: &Client) -> Option<Connection> {
    let mut delay = Duration::from_millis(500);
    loop {
        if SHUTDOWN.load(Ordering::Relaxed) {
            return None;
        }
        match client.get_connection() {
            Ok(c) => {
                info!("Connected to Valkey");
                return Some(c);
            }
            Err(e) => {
                warn!(
                    "Redis connection failed: {e} — retrying in {}ms",
                    delay.as_millis()
                );
                thread::sleep(delay);
                delay = (delay * 2).min(Duration::from_secs(30));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_usize_returns_default_when_unset() {
        let val = env_usize("__FABRIC_SYNC_TEST_NEVER_SET_12345", 42);
        assert_eq!(val, 42);
    }

    #[test]
    fn env_usize_parses_valid_value() {
        std::env::set_var("__FABRIC_SYNC_TEST_VALID", "99");
        let val = env_usize("__FABRIC_SYNC_TEST_VALID", 0);
        assert_eq!(val, 99);
        std::env::remove_var("__FABRIC_SYNC_TEST_VALID");
    }

    #[test]
    fn env_usize_invalid_returns_default() {
        std::env::set_var("__FABRIC_SYNC_TEST_INVALID", "not_a_number");
        let val = env_usize("__FABRIC_SYNC_TEST_INVALID", 77);
        assert_eq!(val, 77);
        std::env::remove_var("__FABRIC_SYNC_TEST_INVALID");
    }
}
