//! PULL and DISCARD message handlers.

use std::sync::Arc;

use crate::bookmark::BookmarkCounter;
use crate::message::types::ServerMessage;
use crate::metrics as bolt_metrics;
use crate::packstream::value::{BoltMap, BoltMapExt, BoltValue};
use crate::protocol::state::ServerState;

use super::{bolt_map, next_bookmark, record_write_bookmark, ProtocolError, Session};

fn failure_message(message: &str) -> ServerMessage {
    ServerMessage::Failure(bolt_map! {
        "code" => BoltValue::String("Neo.ClientError.Statement.SyntaxError".to_string()),
        "message" => BoltValue::String(message.to_string())
    })
}

fn has_regular_updates(stats: &BoltMap) -> bool {
    stats.iter().any(|(k, _)| {
        matches!(
            k.as_str(),
            "nodes-created"
                | "nodes-deleted"
                | "relationships-created"
                | "relationships-deleted"
                | "properties-set"
                | "labels-added"
                | "labels-removed"
                | "indexes-added"
                | "indexes-removed"
                | "constraints-added"
                | "constraints-removed"
        )
    })
}

async fn complete_pending_query(session: &mut Session, qid: i64) -> Result<ServerMessage, ProtocolError> {
    let mut pending = session.pending_results.remove(&qid).ok_or_else(|| {
        ProtocolError::Internal(format!("pending query {qid} missing during completion"))
    })?;

    let bookmark = next_bookmark(&session.global_bookmark).await;
    let elapsed_ms = pending.start_time.elapsed().as_millis() as i64;
    let query_type = pending.query_type;
    let mut stats = pending.stats;
    let admin_stats = pending.admin_stats.take();
    let metadata = pending.metadata;
    let explain_plan = pending.explain_plan.take();
    let profile_plan = pending.profile_plan.take();
    let db_name = pending.graph;
    let query_notifications = pending.notifications;
    let write_backed_bookmark = pending.write_backed_bookmark;

    bolt_metrics::query_completed(&query_type);
    bolt_metrics::query_duration(elapsed_ms as f64 / 1000.0);

    if let Some(ref as_) = admin_stats {
        if as_.indexes_added > 0 {
            stats.push((
                "indexes-added".to_string(),
                BoltValue::Int(as_.indexes_added as i64),
            ));
        }
        if as_.indexes_removed > 0 {
            stats.push((
                "indexes-removed".to_string(),
                BoltValue::Int(as_.indexes_removed as i64),
            ));
        }
        if as_.constraints_added > 0 {
            stats.push((
                "constraints-added".to_string(),
                BoltValue::Int(as_.constraints_added as i64),
            ));
        }
        if as_.constraints_removed > 0 {
            stats.push((
                "constraints-removed".to_string(),
                BoltValue::Int(as_.constraints_removed as i64),
            ));
        }
        if as_.system_updates > 0 {
            stats.push((
                "system-updates".to_string(),
                BoltValue::Int(as_.system_updates as i64),
            ));
            stats.push(("contains-system-updates".to_string(), BoltValue::Bool(true)));
        }
    }

    let mut meta = bolt_map! {
        "bookmark" => BoltValue::String(bookmark),
        "t_last" => BoltValue::Int(elapsed_ms),
        "type" => BoltValue::String(query_type),
        "has_more" => BoltValue::Bool(false)
    };
    if write_backed_bookmark {
        record_write_bookmark(
            &session.global_bookmark,
            meta.get_str("bookmark").unwrap_or(""),
        )
        .await;
    }
    if has_regular_updates(&stats) && !stats.iter().any(|(k, _)| k == "contains-updates") {
        stats.push(("contains-updates".to_string(), BoltValue::Bool(true)));
    }
    if !stats.is_empty() {
        meta.push(("stats".to_string(), BoltValue::Map(stats)));
    }
    if !metadata.is_empty() {
        meta.push(("tx_metadata".to_string(), BoltValue::Map(metadata)));
    }
    meta.push(("db".to_string(), BoltValue::String(db_name)));
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

    if session.pending_results.is_empty() {
        session.state = match session.state {
            ServerState::TxStreaming => ServerState::TxReady,
            _ => ServerState::Ready,
        };
    }

    tracing::debug!(
        qid,
        query_type = meta.get_str("type").unwrap_or(""),
        bookmark = meta.get_str("bookmark").unwrap_or(""),
        db = meta.get_str("db").unwrap_or(""),
        "completed pending query summary"
    );

    Ok(ServerMessage::Success(meta))
}

// -- PULL --------------------------------------------------------------------

/// Handle the PULL message (fetch pending result records).
///
/// Streams up to `n` records from the pending query identified by `qid`
/// (defaulting to the most recent query). Emits `RECORD` messages followed
/// by a `SUCCESS` (with `has_more`) or `FAILURE` (for trailing errors).
#[tracing::instrument(name = "bolt.pull", skip_all)]
pub(crate) async fn handle_pull(
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
                return Ok(vec![failure_message("No result to pull from")]);
            }
        }
    } else {
        requested_qid
    };

    let pending = match session.pending_results.get_mut(&qid) {
        Some(p) => p,
        None => {
            session.state = ServerState::Failed;
            return Ok(vec![failure_message("No result to pull from")]);
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
            pending.cursor.saturating_add(n as usize).min(total)
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
        messages.push(complete_pending_query(session, qid).await?);
    }

    Ok(messages)
}

// -- DISCARD -----------------------------------------------------------------

/// Handle the DISCARD message (discard pending result records).
///
/// Removes the pending query identified by `qid` (defaulting to the most
/// recent query) without streaming its records.
pub(crate) async fn handle_discard(
    session: &mut Session,
    extra: BoltMap,
) -> Result<Vec<ServerMessage>, ProtocolError> {
    // Resolve qid: -1 (or absent) means the last query registered.
    let requested_qid = extra.get_int("qid").unwrap_or(-1);
    let target_qid = if requested_qid == -1 {
        session.pending_results.keys().max().copied()
    } else {
        Some(requested_qid)
    };
    let Some(qid) = target_qid else {
        return Ok(vec![ServerMessage::Success(bolt_map! {})]);
    };

    let trailing_error = match session.pending_results.get_mut(&qid) {
        Some(pending) => pending.trailing_error.take(),
        None => {
            session.state = ServerState::Failed;
            return Ok(vec![failure_message("No result to discard")]);
        }
    };

    if let Some((code, message)) = trailing_error {
        bolt_metrics::error_recorded(&code);
        session.pending_results.remove(&qid);
        session.state = ServerState::Failed;
        return Ok(vec![ServerMessage::Failure(bolt_map! {
            "code" => BoltValue::String(code),
            "message" => BoltValue::String(message)
        })]);
    }

    Ok(vec![complete_pending_query(session, qid).await?])
}
