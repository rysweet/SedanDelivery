//! BEGIN, COMMIT, ROLLBACK, and RESET message handlers.

use std::sync::Arc;

use crate::message::types::ServerMessage;
use crate::packstream::value::{BoltMap, BoltMapExt, BoltValue};
use crate::protocol::state::ServerState;

use super::{
    bolt_map, extract_tx_metadata, next_bookmark, validate_bookmarks, ProtocolError, Session,
};

// -- BEGIN -------------------------------------------------------------------

/// Handle the BEGIN message (start an explicit transaction).
///
/// Validates bookmarks, extracts transaction metadata and timeout, creates a
/// copy-on-write snapshot of the target graph, and transitions to `TxReady`.
pub(crate) fn handle_begin(
    session: &mut Session,
    extra: BoltMap,
) -> Result<Vec<ServerMessage>, ProtocolError> {
    if let Some(failure) = validate_bookmarks(&extra) {
        session.state = ServerState::Failed;
        return Ok(vec![failure]);
    }

    session.tx_metadata = extract_tx_metadata(&extra);
    session.tx_timeout = extra.get_int("tx_timeout").map(|v| v as u64);

    // If the BEGIN specifies a database, update the session's current graph so
    // that subsequent RUN messages within the TX use the correct graph.
    if let Some(db) = extra.get_str("db") {
        let db = db.trim_matches(|c: char| c == '\'' || c == '"');
        if !db.is_empty() {
            session.current_graph = db.to_string();
        }
    }

    // Create a copy-on-write snapshot of the target graph so that mutations
    // inside the transaction are isolated until COMMIT.
    let snapshot = session
        .store
        .snapshot(&[&session.current_graph])
        .map_err(|e| ProtocolError::Internal(e.to_string()))?;
    session.tx_store = Some(Arc::new(snapshot));

    session.state = ServerState::TxReady;
    Ok(vec![ServerMessage::Success(bolt_map! {})])
}

// -- COMMIT ------------------------------------------------------------------

/// Handle the COMMIT message (commit the current explicit transaction).
///
/// Merges the copy-on-write snapshot back into the main store, releases the
/// write lock, produces a bookmark, and transitions to `Ready`.
pub(crate) fn handle_commit(session: &mut Session) -> Result<Vec<ServerMessage>, ProtocolError> {
    // Merge the snapshot back into the main store so that committed mutations
    // become visible to other sessions.
    if let Some(ref tx_store) = session.tx_store {
        session
            .store
            .merge_from(tx_store)
            .map_err(|e| ProtocolError::Internal(e.to_string()))?;
    }
    session.tx_store = None;
    // Release the write lock if held.
    session.write_lock_guard = None;

    let bookmark = next_bookmark(&session.global_bookmark);
    session.state = ServerState::Ready;
    session.tx_metadata = Vec::new();
    session.tx_timeout = None;
    Ok(vec![ServerMessage::Success(bolt_map! {
        "bookmark" => BoltValue::String(bookmark)
    })])
}

// -- ROLLBACK ----------------------------------------------------------------

/// Handle ROLLBACK.
///
/// Discards the copy-on-write snapshot so that mutations performed inside the
/// transaction are never applied to the main store.
pub(crate) fn handle_rollback(session: &mut Session) -> Result<Vec<ServerMessage>, ProtocolError> {
    session.tx_store = None;
    session.pending_results.clear();
    session.write_lock_guard = None;
    session.state = ServerState::Ready;
    session.tx_metadata = Vec::new();
    session.tx_timeout = None;
    // No bookmark on rollback: the driver testkit expects that a rolled-back
    // transaction does not produce a bookmark.
    Ok(vec![ServerMessage::Success(bolt_map! {})])
}

// -- RESET -------------------------------------------------------------------

/// Handle RESET.
///
/// Clears all session-local state (pending results, TX metadata/timeout,
/// snapshot store) and returns to the Ready state. This is also the handler
/// invoked when a client exception causes the driver to send RESET.
pub(crate) fn handle_reset(session: &mut Session) -> Result<Vec<ServerMessage>, ProtocolError> {
    session.pending_results.clear();
    session.tx_metadata = Vec::new();
    session.tx_timeout = None;
    session.tx_store = None;
    session.write_lock_guard = None;
    session.state = ServerState::Ready;
    Ok(vec![ServerMessage::Success(bolt_map! {})])
}
