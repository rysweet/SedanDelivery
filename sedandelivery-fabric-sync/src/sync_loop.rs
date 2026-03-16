//! XREADGROUP sync loop and entry processing logic.

use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

// Per-graph stream keys are discovered in main.rs and passed as `streams` parameter.
use gql_fabric_store::FabricGraphStore;
use metrics::counter;
use redis::{Client, Connection, RedisResult, Value as RV};
use tracing::{error, warn};

use crate::apply::apply_event;
use crate::parse_reply::{extract_entries, StreamEntry};
use crate::{connect_with_retry, GROUP_NAME, SHUTDOWN};

fn dlq_stream() -> String {
    std::env::var("SYNC_DLQ_STREAM").unwrap_or_else(|_| "__sd2:changelog:dlq".to_string())
}
const PERMANENT_FAILURES_TOTAL: &str = "fabric_sync_permanent_failures_total";
const MAX_CONSECUTIVE_UNRESOLVED_BATCHES: usize = 3;

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_sync_loop(
    client: &Client,
    conn: &mut Connection,
    consumer_name: &str,
    fabric: &FabricGraphStore,
    batch_size: usize,
    block_ms: usize,
    max_retries: usize,
    streams: Vec<String>,
) {
    let mut consecutive_unresolved_batches = 0usize;

    loop {
        if SHUTDOWN.load(Ordering::Relaxed) {
            break;
        }

        // Phase 1: drain any pending (unacknowledged) messages from all streams.
        let mut pending_ok = true;
        for stream in &streams {
            if !process_pending(conn, consumer_name, fabric, batch_size, max_retries, stream) {
                pending_ok = false;
                break;
            }
        }
        if !pending_ok {
            consecutive_unresolved_batches += 1;
            if consecutive_unresolved_batches >= MAX_CONSECUTIVE_UNRESOLVED_BATCHES {
                error!(
                    consecutive_unresolved_batches,
                    "Stopping sync loop after repeated unresolved batches caused by DLQ persistence failure"
                );
                break;
            }
            if !reconnect_after_unresolved_batch(client, conn, "pending") {
                break;
            }
            continue;
        }
        consecutive_unresolved_batches = 0;

        // Phase 2: block-read new messages from all streams.
        let mut cmd = redis::cmd("XREADGROUP");
        cmd.arg("GROUP")
            .arg(GROUP_NAME)
            .arg(consumer_name)
            .arg("COUNT")
            .arg(batch_size)
            .arg("BLOCK")
            .arg(block_ms)
            .arg("STREAMS");
        // XREADGROUP requires all stream keys first, then all IDs.
        for stream in &streams {
            cmd.arg(stream.as_str());
        }
        for _ in &streams {
            cmd.arg(">");
        }
        let reply: RedisResult<RV> = cmd.query(conn);

        match reply {
            Ok(RV::Nil) => {}
            Ok(data) => {
                let entries = extract_entries(data);
                if !process_entries(conn, fabric, entries, max_retries) {
                    consecutive_unresolved_batches += 1;
                    if consecutive_unresolved_batches >= MAX_CONSECUTIVE_UNRESOLVED_BATCHES {
                        error!(
                            consecutive_unresolved_batches,
                            "Stopping sync loop after repeated unresolved batches caused by DLQ persistence failure"
                        );
                        break;
                    }
                    if !reconnect_after_unresolved_batch(client, conn, "new-message") {
                        break;
                    }
                    continue;
                }
                consecutive_unresolved_batches = 0;
            }
            Err(e) => {
                error!("XREADGROUP error: {e} — reconnecting");
                match connect_with_retry(client) {
                    Some(Ok(c)) => *conn = c,
                    Some(Err(e)) => {
                        error!("Giving up reconnection: {e}");
                        break;
                    }
                    None => break, // SHUTDOWN signalled
                }
            }
        }
    }
}

fn reconnect_after_unresolved_batch(client: &Client, conn: &mut Connection, phase: &str) -> bool {
    warn!(
        phase = phase,
        "Batch left unresolved after DLQ persistence failure — reconnecting before retry"
    );
    match connect_with_retry(client) {
        Some(Ok(c)) => {
            *conn = c;
            true
        }
        Some(Err(e)) => {
            error!(
                phase = phase,
                "Giving up reconnection after unresolved batch: {e}"
            );
            false
        }
        None => false, // SHUTDOWN signalled
    }
}

/// Process pending (unacknowledged) messages from a previous consumer run.
fn process_pending(
    conn: &mut Connection,
    consumer_name: &str,
    fabric: &FabricGraphStore,
    batch_size: usize,
    max_retries: usize,
    stream: &str,
) -> bool {
    loop {
        if SHUTDOWN.load(Ordering::Relaxed) {
            return true;
        }
        let reply: RedisResult<RV> = redis::cmd("XREADGROUP")
            .arg("GROUP")
            .arg(GROUP_NAME)
            .arg(consumer_name)
            .arg("COUNT")
            .arg(batch_size)
            .arg("STREAMS")
            .arg(stream)
            .arg("0")
            .query(conn);

        match reply {
            Ok(RV::Nil) => return true,
            Ok(data) => {
                let entries = extract_entries(data);
                if entries.is_empty() {
                    return true;
                }
                let batch_resolved = process_entries(conn, fabric, entries, max_retries);
                if !batch_resolved {
                    return false;
                }
            }
            Err(e) => {
                warn!("XREADGROUP pending fetch error: {e}");
                return true;
            }
        }
    }
}

fn process_entries(
    conn: &mut Connection,
    fabric: &FabricGraphStore,
    entries: Vec<StreamEntry>,
    max_retries: usize,
) -> bool {
    let attempt_budget = max_retries.max(1);

    for (stream_key, entry_id, fields) in entries {
        let event_json = match fields.get("e") {
            Some(j) => j,
            None => {
                warn!(entry_id = %entry_id, "changelog entry missing 'e' field — skipping");
                ack(conn, &stream_key, &entry_id);
                continue;
            }
        };

        let event: gql_core::changelog::ChangeEvent = match serde_json::from_str(event_json) {
            Ok(e) => e,
            Err(err) => {
                warn!(entry_id = %entry_id, "failed to deserialise ChangeEvent: {err} — skipping");
                ack(conn, &stream_key, &entry_id);
                continue;
            }
        };

        let mut last_err = None;
        for attempt in 1..=attempt_budget {
            match apply_event(fabric, &event) {
                Ok(()) => {
                    last_err = None;
                    break;
                }
                Err(e) => {
                    warn!(
                        entry_id = %entry_id,
                        attempt,
                        attempt_budget,
                        "apply_event failed: {e}"
                    );
                    last_err = Some(e);
                    if attempt < attempt_budget {
                        // Exponential backoff: 500ms, 1s, 2s, ... capped at 5s.
                        let backoff_ms =
                            (500u64 * 2u64.saturating_pow(attempt as u32 - 1)).min(5000);
                        thread::sleep(Duration::from_millis(backoff_ms));
                    }
                }
            }
        }

        let should_ack = if let Some(e) = last_err {
            error!(
                entry_id = %entry_id,
                "Permanent failure applying changelog event after {attempt_budget} attempt(s): {e}. \
                 Writing to DLQ before acknowledging."
            );
            counter!(PERMANENT_FAILURES_TOTAL).increment(1);
            send_to_dlq(conn, &entry_id, event_json, &e.to_string())
        } else {
            true
        };

        if should_ack {
            ack(conn, &stream_key, &entry_id);
        } else {
            return false;
        }
    }

    true
}

/// Write a permanently failed event to the dead-letter queue stream.
/// Returns `true` when persistence succeeded and the caller may safely ACK the
/// original event. Returns `false` when DLQ persistence failed, leaving the
/// original event pending for retry.
fn send_to_dlq(conn: &mut Connection, entry_id: &str, event_json: &str, error_msg: &str) -> bool {
    let dlq = dlq_stream();
    let result: RedisResult<String> = redis::cmd("XADD")
        .arg(&dlq)
        .arg("*")
        .arg("original_id")
        .arg(entry_id)
        .arg("event")
        .arg(event_json)
        .arg("error")
        .arg(error_msg)
        .query(conn);
    match result {
        Ok(dlq_id) => {
            warn!(
                entry_id = %entry_id,
                dlq_id = %dlq_id,
                "Event written to DLQ stream"
            );
            true
        }
        Err(e) => {
            error!(
                entry_id = %entry_id,
                "Failed to write event to DLQ stream: {e} — leaving original event pending"
            );
            false
        }
    }
}

/// Acknowledge a processed stream entry, retrying once on failure.
fn ack(conn: &mut Connection, stream_key: &str, entry_id: &str) {
    for attempt in 0..2 {
        let result: RedisResult<()> = redis::cmd("XACK")
            .arg(stream_key)
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

#[cfg(test)]
mod tests;
