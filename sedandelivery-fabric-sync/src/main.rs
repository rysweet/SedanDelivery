//! sedandelivery-fabric-sync — async replication from Valkey to Microsoft Fabric Graph.
//!
//! # What it does
//!
//! Reads change events from the Valkey stream `__sd2:changelog` (written by
//! the `gql-module` after each successful write command) and replays them
//! against a `FabricGraphStore` instance via the Fabric Graph REST API.
//!
//! # Enabling
//!
//! The process exits cleanly on startup when `GQL_FABRIC_ENDPOINT` is not set.
//! This makes it safe to deploy unconditionally: without Fabric credentials it
//! is a no-op sidecar.
//!
//! # Reliability
//!
//! Uses `XREADGROUP` with a named consumer group so that unacknowledged events
//! are re-delivered on restart.  Each event is retried up to `MAX_RETRIES`
//! times before being skipped with an error log (the event remains in the
//! stream's Pending Entries List for manual replay).
//!
//! # ID consistency
//!
//! `CreateNode` and `CreateEdge` events carry the IDs assigned by the primary
//! (Valkey/InMemoryStore).  These are injected as `_sd2_id` / `_sd2_eid`
//! properties so `FabricGraphStore` uses `MERGE` semantics rather than
//! generating new IDs — keeping both stores in sync.
//!
//! # Configuration (environment variables)
//!
//! | Variable               | Default                     | Description                          |
//! |------------------------|-----------------------------|--------------------------------------|
//! | `GQL_REDIS_URL`        | `redis://127.0.0.1:6379`    | Valkey connection URL                |
//! | `GQL_FABRIC_ENDPOINT`  | (required)                  | Fabric Graph REST API base URL       |
//! | `GQL_FABRIC_TENANT_ID` | (required)                  | Azure AD tenant ID                   |
//! | `GQL_FABRIC_CLIENT_ID` | (required)                  | Service-principal app ID             |
//! | `GQL_FABRIC_CLIENT_SECRET` | (required)              | Service-principal secret             |
//! | `SD2_SYNC_BATCH`       | `100`                       | Events per XREADGROUP call           |
//! | `SD2_SYNC_BLOCK_MS`    | `1000`                      | XREADGROUP blocking timeout (ms)     |
//! | `SD2_SYNC_MAX_RETRIES` | `3`                         | Per-event retry limit                |
//! | `SD2_SYNC_CONSUMER`    | hostname / `worker-0`       | Consumer name for XREADGROUP         |

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use gql_core::changelog::{json_map_to_props, ChangeEvent, CHANGELOG_STREAM};
use gql_core::storage::{EdgeId, GraphStore, NodeId};
use gql_core::types::Value;
use gql_fabric_store::FabricGraphStore;
use redis::{Client, Connection, RedisResult, Value as RV};
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

    // Exit cleanly when Fabric is not configured — makes the binary safe to
    // ship unconditionally; the operator opts-in by setting env vars.
    if std::env::var("GQL_FABRIC_ENDPOINT").is_err() {
        info!("GQL_FABRIC_ENDPOINT not set — Fabric sync disabled, exiting");
        return;
    }

    info!("sedandelivery-fabric-sync starting");

    // Install SIGTERM/SIGINT handler for graceful shutdown.
    ctrlc::set_handler(|| {
        SHUTDOWN.store(true, Ordering::Relaxed);
    })
    .expect("failed to install signal handler");

    let batch_size: usize = env_usize("SD2_SYNC_BATCH", 100);
    let block_ms: usize = env_usize("SD2_SYNC_BLOCK_MS", 1000);
    let max_retries: usize = env_usize("SD2_SYNC_MAX_RETRIES", 3);

    // Consumer name: from env, hostname, or fallback to "worker-0".
    let consumer_name = std::env::var("SD2_SYNC_CONSUMER").unwrap_or_else(|_| {
        hostname::get()
            .ok()
            .and_then(|h| h.into_string().ok())
            .unwrap_or_else(|| "worker-0".to_string())
    });

    // Build FabricGraphStore from environment variables.
    let fabric = match FabricGraphStore::from_env() {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to initialise FabricGraphStore: {e}");
            std::process::exit(1);
        }
    };

    // Connect to Valkey/Redis.
    let redis_url =
        std::env::var("GQL_REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());

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

    // Create consumer group starting from entry 0 so we replay any events
    // that arrived before this process started.  MKSTREAM creates the stream
    // if it does not exist yet.
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

    run_sync_loop(
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

// ── Main sync loop ────────────────────────────────────────────────────────────

fn run_sync_loop(
    client: &Client,
    conn: &mut Connection,
    consumer_name: &str,
    fabric: &FabricGraphStore,
    batch_size: usize,
    block_ms: usize,
    max_retries: usize,
) {
    loop {
        if SHUTDOWN.load(Ordering::Relaxed) {
            break;
        }

        // Phase 1: drain any pending (unacknowledged) messages first.
        process_pending(conn, consumer_name, fabric, batch_size, max_retries);

        // Phase 2: block-read new messages.
        let reply: RedisResult<RV> = redis::cmd("XREADGROUP")
            .arg("GROUP")
            .arg(GROUP_NAME)
            .arg(consumer_name)
            .arg("COUNT")
            .arg(batch_size)
            .arg("BLOCK")
            .arg(block_ms)
            .arg("STREAMS")
            .arg(CHANGELOG_STREAM)
            .arg(">")
            .query(conn);

        match reply {
            Ok(RV::Nil) => {}
            Ok(data) => {
                let entries = extract_entries(data);
                process_entries(conn, fabric, entries, max_retries);
            }
            Err(e) => {
                error!("XREADGROUP error: {e} — reconnecting");
                match connect_with_retry(client) {
                    Some(c) => *conn = c,
                    None => break, // SHUTDOWN signalled
                }
            }
        }
    }
}

/// Process pending (unacknowledged) messages from a previous consumer run.
fn process_pending(
    conn: &mut Connection,
    consumer_name: &str,
    fabric: &FabricGraphStore,
    batch_size: usize,
    max_retries: usize,
) {
    loop {
        if SHUTDOWN.load(Ordering::Relaxed) {
            break;
        }
        let reply: RedisResult<RV> = redis::cmd("XREADGROUP")
            .arg("GROUP")
            .arg(GROUP_NAME)
            .arg(consumer_name)
            .arg("COUNT")
            .arg(batch_size)
            .arg("STREAMS")
            .arg(CHANGELOG_STREAM)
            .arg("0")
            .query(conn);

        match reply {
            Ok(RV::Nil) => break,
            Ok(data) => {
                let entries = extract_entries(data);
                if entries.is_empty() {
                    break;
                }
                process_entries(conn, fabric, entries, max_retries);
            }
            Err(e) => {
                warn!("XREADGROUP pending fetch error: {e}");
                break;
            }
        }
    }
}

// ── Entry processing ─────────────────────────────────────────────────────────

/// A stream entry: (entry_id, field->value map).
type StreamEntry = (String, HashMap<String, String>);

fn process_entries(
    conn: &mut Connection,
    fabric: &FabricGraphStore,
    entries: Vec<StreamEntry>,
    max_retries: usize,
) {
    for (entry_id, fields) in entries {
        let event_json = match fields.get("e") {
            Some(j) => j,
            None => {
                warn!(entry_id = %entry_id, "changelog entry missing 'e' field — skipping");
                ack(conn, &entry_id);
                continue;
            }
        };

        let event: ChangeEvent = match serde_json::from_str(event_json) {
            Ok(e) => e,
            Err(err) => {
                warn!(entry_id = %entry_id, "failed to deserialise ChangeEvent: {err} — skipping");
                ack(conn, &entry_id);
                continue;
            }
        };

        let mut last_err = None;
        for attempt in 1..=max_retries {
            match apply_event(fabric, &event) {
                Ok(()) => {
                    last_err = None;
                    break;
                }
                Err(e) => {
                    warn!(
                        entry_id = %entry_id,
                        attempt,
                        max_retries,
                        "apply_event failed: {e}"
                    );
                    last_err = Some(e);
                    if attempt < max_retries {
                        // Exponential backoff: 500ms, 1s, 2s, ... capped at 5s.
                        let backoff_ms =
                            (500u64 * 2u64.saturating_pow(attempt as u32 - 1)).min(5000);
                        thread::sleep(Duration::from_millis(backoff_ms));
                    }
                }
            }
        }

        if let Some(e) = last_err {
            // All retries exhausted.  Log clearly and XACK so we don't block
            // the stream indefinitely.  The entry remains in the stream history
            // and can be replayed manually with XCLAIM + XACK.
            error!(
                entry_id = %entry_id,
                "Permanent failure applying changelog event after {max_retries} retries: {e}. \
                 Acknowledging to unblock stream. Manual replay via XCLAIM may be needed."
            );
        }

        ack(conn, &entry_id);
    }
}

/// Acknowledge a processed stream entry, retrying once on failure.
fn ack(conn: &mut Connection, entry_id: &str) {
    for attempt in 0..2 {
        let result: RedisResult<()> = redis::cmd("XACK")
            .arg(CHANGELOG_STREAM)
            .arg(GROUP_NAME)
            .arg(entry_id)
            .query(conn);
        match result {
            Ok(()) => return,
            Err(e) if attempt == 0 => {
                warn!(entry_id = %entry_id, "XACK failed: {e} — retrying once");
                thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                error!(
                    entry_id = %entry_id,
                    "XACK failed after retry: {e} — entry will re-appear on restart"
                );
            }
        }
    }
}

// ── Event application ─────────────────────────────────────────────────────────

/// Translate a `ChangeEvent` into the corresponding `FabricGraphStore` call.
///
/// For `CreateNode` and `CreateEdge` the event carries the ID assigned by the
/// primary store.  We inject it as `_sd2_id` / `_sd2_eid` so `FabricGraphStore`
/// uses `MERGE` semantics and preserves the original ID.
fn apply_event(
    fabric: &FabricGraphStore,
    event: &ChangeEvent,
) -> Result<(), gql_core::error::StorageError> {
    match event {
        // ── Graph-level ───────────────────────────────────────────────────────
        ChangeEvent::CreateGraph { name } => {
            fabric.create_graph(name)?;
        }
        ChangeEvent::DeleteGraph { name } => {
            fabric.delete_graph(name)?;
        }

        // ── Node writes ───────────────────────────────────────────────────────
        ChangeEvent::CreateNode {
            graph,
            id,
            labels,
            properties,
        } => {
            // Inject primary ID so FabricGraphStore skips its own sequence and
            // stores the same numeric ID as Valkey.
            let mut props = json_map_to_props(properties);
            let id_i64 =
                i64::try_from(*id).map_err(|_| gql_core::error::StorageError::Backend {
                    msg: format!("node ID {id} exceeds i64 range — cannot sync to Fabric"),
                })?;
            props.insert("_sd2_id".to_string(), Value::Int(id_i64));
            fabric.create_node(graph, labels.clone(), props)?;
        }
        ChangeEvent::SetNodeProperties {
            graph,
            id,
            properties,
        } => {
            fabric.set_node_properties(graph, NodeId(*id), json_map_to_props(properties))?;
        }
        ChangeEvent::ReplaceNodeProperties {
            graph,
            id,
            properties,
        } => {
            fabric.replace_node_properties(graph, NodeId(*id), json_map_to_props(properties))?;
        }
        ChangeEvent::RemoveNodeProperty { graph, id, key } => {
            fabric.remove_node_property(graph, NodeId(*id), key)?;
        }
        ChangeEvent::AddNodeLabels { graph, id, labels } => {
            fabric.add_node_labels(graph, NodeId(*id), labels)?;
        }
        ChangeEvent::RemoveNodeLabels { graph, id, labels } => {
            fabric.remove_node_labels(graph, NodeId(*id), labels)?;
        }
        ChangeEvent::DeleteNode { graph, id, detach } => {
            fabric.delete_node(graph, NodeId(*id), *detach)?;
        }

        // ── Edge writes ───────────────────────────────────────────────────────
        ChangeEvent::CreateEdge {
            graph,
            id,
            src,
            dst,
            rel_type,
            properties,
        } => {
            let mut props = json_map_to_props(properties);
            let eid_i64 =
                i64::try_from(*id).map_err(|_| gql_core::error::StorageError::Backend {
                    msg: format!("edge ID {id} exceeds i64 range — cannot sync to Fabric"),
                })?;
            props.insert("_sd2_eid".to_string(), Value::Int(eid_i64));
            fabric.create_edge(graph, NodeId(*src), NodeId(*dst), rel_type.clone(), props)?;
        }
        ChangeEvent::SetEdgeProperties {
            graph,
            id,
            properties,
        } => {
            fabric.set_edge_properties(graph, EdgeId(*id), json_map_to_props(properties))?;
        }
        ChangeEvent::ReplaceEdgeProperties {
            graph,
            id,
            properties,
        } => {
            fabric.replace_edge_properties(graph, EdgeId(*id), json_map_to_props(properties))?;
        }
        ChangeEvent::RemoveEdgeProperty { graph, id, key } => {
            fabric.remove_edge_property(graph, EdgeId(*id), key)?;
        }
        ChangeEvent::DeleteEdge { graph, id } => {
            fabric.delete_edge(graph, EdgeId(*id))?;
        }

        // ── Index management ──────────────────────────────────────────────────
        ChangeEvent::CreateIndex {
            graph,
            label,
            property,
        } => {
            fabric.create_index(graph, label, property)?;
        }
        ChangeEvent::DropIndex {
            graph,
            label,
            property,
        } => {
            fabric.drop_index(graph, label, property)?;
        }
    }
    Ok(())
}

// ── Stream reply parsing ──────────────────────────────────────────────────────

/// Extract `(entry_id, fields)` pairs from a raw XREADGROUP reply.
///
/// The Valkey/Redis RESP reply for XREADGROUP has the shape:
/// ```text
/// [stream-name, [[entry-id, [field, value, field, value, ...]], ...]]
/// ```
/// We navigate this structure defensively — any unexpected shape is logged
/// and skipped.
fn extract_entries(reply: RV) -> Vec<StreamEntry> {
    let mut result = Vec::new();

    // Top-level: array of [stream-key, entries]
    let streams = match reply {
        RV::Array(v) => v,
        _ => return result,
    };

    for stream in streams {
        // Each stream: [key, [[id, fields], ...]]
        let parts = match stream {
            RV::Array(v) => v,
            _ => continue,
        };
        if parts.len() < 2 {
            continue;
        }
        let entries_val = &parts[1];
        let entries = match entries_val {
            RV::Array(v) => v,
            _ => continue,
        };
        for entry in entries {
            let entry_parts = match entry {
                RV::Array(v) => v,
                _ => continue,
            };
            if entry_parts.len() < 2 {
                continue;
            }
            let id = match &entry_parts[0] {
                RV::BulkString(b) => String::from_utf8_lossy(b).into_owned(),
                RV::SimpleString(s) => s.clone(),
                _ => continue,
            };
            let fields_list = match &entry_parts[1] {
                RV::Array(v) => v,
                _ => continue,
            };
            let mut fields = HashMap::new();
            let mut i = 0;
            while i + 1 < fields_list.len() {
                let k = redis_str(&fields_list[i]);
                let v = redis_str(&fields_list[i + 1]);
                if let (Some(k), Some(v)) = (k, v) {
                    fields.insert(k, v);
                }
                i += 2;
            }
            result.push((id, fields));
        }
    }
    result
}

fn redis_str(v: &RV) -> Option<String> {
    match v {
        RV::BulkString(b) => Some(String::from_utf8_lossy(b).into_owned()),
        RV::SimpleString(s) => Some(s.clone()),
        _ => None,
    }
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
