//! sedandelivery-fabric-sync — async replication from Valkey to Microsoft Fabric Graph.
//!
//! Reads change events from the Valkey stream `__sd2:changelog` and replays them
//! against a `FabricGraphStore` via the Fabric Graph REST API.
//!
//! Exits cleanly when `GQL_FABRIC_ENDPOINT` is not set (no-op sidecar mode).
//! Uses `XREADGROUP` with a consumer group for at-least-once delivery.

mod apply;
mod config;
mod connection;
mod parse_reply;
mod sync_loop;

use std::sync::atomic::Ordering;

use gql_fabric_store::FabricGraphStore;
use redis::Client;
use tracing::{error, info};

use crate::config::{env_usize, redact_url_credentials, redis_timeouts, GROUP_NAME};
use crate::connection::{
    connect_with_retry, discover_changelog_streams, ensure_consumer_group, SHUTDOWN,
};

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
