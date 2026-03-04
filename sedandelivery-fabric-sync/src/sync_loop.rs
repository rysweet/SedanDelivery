//! XREADGROUP sync loop and entry processing logic.

use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

use gql_core::changelog::CHANGELOG_STREAM;
use gql_fabric_store::FabricGraphStore;
use redis::{Client, Connection, RedisResult, Value as RV};
use tracing::{error, info, warn};

use crate::apply::apply_event;
use crate::parse_reply::{extract_entries, StreamEntry};
use crate::{connect_with_retry, GROUP_NAME, SHUTDOWN};

pub(crate) fn run_sync_loop(
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

        let event: gql_core::changelog::ChangeEvent = match serde_json::from_str(event_json) {
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
