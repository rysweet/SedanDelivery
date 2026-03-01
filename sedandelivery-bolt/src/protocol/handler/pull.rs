//! PULL and DISCARD message handlers.

use crate::message::types::ServerMessage;
use crate::metrics as bolt_metrics;
use crate::packstream::value::{BoltMap, BoltMapExt, BoltValue};
use crate::protocol::state::ServerState;

use super::{bolt_map, next_bookmark, ProtocolError, Session};

// -- PULL --------------------------------------------------------------------

/// Handle the PULL message (fetch pending result records).
///
/// Streams up to `n` records from the pending query identified by `qid`
/// (defaulting to the most recent query). Emits `RECORD` messages followed
/// by a `SUCCESS` (with `has_more`) or `FAILURE` (for trailing errors).
pub(crate) fn handle_pull(
    session: &mut Session,
    extra: BoltMap,
) -> Result<Vec<ServerMessage>, ProtocolError> {
    let n = extra.get_int("n").unwrap_or(-1);

    // Resolve qid: -1 (or absent) means the last query registered.
    let requested_qid = extra.get_int("qid").unwrap_or(-1);
    let qid = if requested_qid == -1 {
        // Find the highest qid in pending_results (last query).
        match session.pending_results.keys().max().copied() {
            Some(q) => q,
            None => {
                session.state = ServerState::Failed;
                return Ok(vec![ServerMessage::Failure(bolt_map! {
                    "code" => BoltValue::String("Neo.ClientError.Statement.SyntaxError".to_string()),
                    "message" => BoltValue::String("No result to pull from".to_string())
                })]);
            }
        }
    } else {
        requested_qid
    };

    let pending = match session.pending_results.get_mut(&qid) {
        Some(p) => p,
        None => {
            session.state = ServerState::Failed;
            return Ok(vec![ServerMessage::Failure(bolt_map! {
                "code" => BoltValue::String("Neo.ClientError.Statement.SyntaxError".to_string()),
                "message" => BoltValue::String("No result to pull from".to_string())
            })]);
        }
    };

    let mut messages = Vec::new();

    // For EXPLAIN, skip all RECORDs -- the plan goes in the SUCCESS metadata
    let is_explain = pending.explain_plan.is_some();
    let total = pending.records.len();

    if !is_explain {
        let end = if n < 0 {
            total
        } else {
            (pending.cursor + n as usize).min(total)
        };
        for i in pending.cursor..end {
            messages.push(ServerMessage::Record(pending.records[i].clone()));
        }
        pending.cursor = end;
    } else {
        pending.cursor = total; // skip all records for EXPLAIN
    }

    let has_more = pending.cursor < total;
    // Check for a trailing streaming error that should be emitted after
    // all buffered records have been sent.
    let trailing_error = if !has_more {
        pending.trailing_error.take()
    } else {
        None
    };

    if let Some((code, message)) = trailing_error {
        // All buffered records have been streamed. Now emit FAILURE for the
        // per-row error that terminated evaluation, and transition to Failed.
        bolt_metrics::error_recorded(&code);
        messages.push(ServerMessage::Failure(bolt_map! {
            "code" => BoltValue::String(code),
            "message" => BoltValue::String(message)
        }));
        session.pending_results.remove(&qid);
        session.state = ServerState::Failed;
    } else if has_more {
        messages.push(ServerMessage::Success(bolt_map! {
            "has_more" => BoltValue::Bool(true)
        }));
    } else {
        let bookmark = next_bookmark(&session.global_bookmark);
        let elapsed_ms = pending.start_time.elapsed().as_millis() as i64;
        let query_type = pending.query_type.clone();
        let stats = pending.stats.clone();
        let metadata = pending.metadata.clone();
        let explain_plan = pending.explain_plan.take();
        let profile_plan = pending.profile_plan.take();
        let db_name = pending.graph.clone();
        let query_notifications = std::mem::take(&mut pending.notifications);

        // Record query metrics on completion.
        bolt_metrics::query_completed(&query_type);
        bolt_metrics::query_duration(elapsed_ms as f64 / 1000.0);

        let mut meta = bolt_map! {
            "bookmark" => BoltValue::String(bookmark),
            "t_last" => BoltValue::Int(elapsed_ms),
            "type" => BoltValue::String(query_type),
            "has_more" => BoltValue::Bool(false)
        };
        if !stats.is_empty() {
            meta.push(("stats".to_string(), BoltValue::Map(stats)));
        }
        if !metadata.is_empty() {
            meta.push(("tx_metadata".to_string(), BoltValue::Map(metadata)));
        }
        // Include the database name from the query's effective graph.
        meta.push(("db".to_string(), BoltValue::String(db_name)));
        // Include notifications (cartesian product warnings, etc.).
        meta.push((
            "notifications".to_string(),
            BoltValue::List(query_notifications),
        ));
        if let Some(plan) = explain_plan {
            meta.push(("plan".to_string(), plan));
        }
        if let Some(profile) = profile_plan {
            meta.push(("profile".to_string(), profile));
        }
        messages.push(ServerMessage::Success(meta));

        // Remove the completed query from pending_results.
        session.pending_results.remove(&qid);
        // Only transition state if no more pending results.
        if session.pending_results.is_empty() {
            session.state = match session.state {
                ServerState::TxStreaming => ServerState::TxReady,
                _ => ServerState::Ready,
            };
        }
    }

    Ok(messages)
}

// -- DISCARD -----------------------------------------------------------------

/// Handle the DISCARD message (discard pending result records).
///
/// Removes the pending query identified by `qid` (defaulting to the most
/// recent query) without streaming its records.
pub(crate) fn handle_discard(
    session: &mut Session,
    extra: BoltMap,
) -> Result<Vec<ServerMessage>, ProtocolError> {
    // Resolve qid: -1 (or absent) means the last query registered.
    let requested_qid = extra.get_int("qid").unwrap_or(-1);
    if requested_qid == -1 {
        // Discard the last (highest qid) result.
        if let Some(&max_qid) = session.pending_results.keys().max() {
            session.pending_results.remove(&max_qid);
        }
    } else {
        session.pending_results.remove(&requested_qid);
    }

    // Only transition state if no more pending results.
    if session.pending_results.is_empty() {
        session.state = match session.state {
            ServerState::TxStreaming => ServerState::TxReady,
            _ => ServerState::Ready,
        };
    }
    Ok(vec![ServerMessage::Success(bolt_map! {})])
}
