//! BEGIN, COMMIT, ROLLBACK, and RESET message handlers.

use std::sync::Arc;

use crate::message::types::ServerMessage;
use crate::packstream::value::{BoltMap, BoltMapExt, BoltValue};
use crate::protocol::state::ServerState;

use super::{
    bolt_map, extract_tx_metadata, next_bookmark, recent_bookmark_age, record_write_bookmark,
    validate_bookmarks, ProtocolError, Session,
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
    if let Some(age) = recent_bookmark_age(&extra, &session.global_bookmark) {
        session.apply_recent_bookmark_age(age);
    }

    session.tx_metadata = extract_tx_metadata(&extra);
    session.tx_timeout = extra
        .get_int("tx_timeout")
        .filter(|&v| v > 0)
        .map(|v| v as u64);

    // If the BEGIN specifies a database, save the previous graph so that
    // ROLLBACK / RESET can restore it, then update the session's current
    // graph so subsequent RUN messages within the TX use the correct graph.
    if let Some(db) = extra.get_str("db") {
        let db = db.trim_matches(|c: char| c == '\'' || c == '"');
        if !db.is_empty() {
            session.saved_graph = Some(session.current_graph.clone());
            session.current_graph = db.to_string();
        }
    }

    // Create a copy-on-write snapshot of the target graph so that mutations
    // inside the transaction are isolated until COMMIT.
    // Returns None for backends where writes are immediately visible (e.g. Redis).
    if let Some(snapshot) = session
        .store
        .tx_snapshot(&[&session.current_graph])
        .map_err(|e| ProtocolError::Internal(e.to_string()))?
    {
        session.tx_store = Some(Arc::from(snapshot));
    }

    session.tx_had_write = false;
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
            .tx_commit(tx_store.as_ref())
            .map_err(|e| ProtocolError::Internal(e.to_string()))?;
    }
    session.tx_store = None;
    let committed_write = session.tx_had_write;
    session.finish_explicit_transaction(true);
    // Release the write lock if held.
    session.write_permit = None;

    let bookmark = next_bookmark(&session.global_bookmark);
    if committed_write {
        record_write_bookmark(&session.global_bookmark, &bookmark);
    }
    session.state = ServerState::Ready;
    session.tx_metadata = Vec::new();
    session.tx_timeout = None;
    // Restore the pre-BEGIN graph so that `db=` does not leak across TXs.
    if let Some(graph) = session.saved_graph.take() {
        session.current_graph = graph;
    }
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
    session.finish_explicit_transaction(false);
    session.pending_results.clear();
    session.write_permit = None;
    session.state = ServerState::Ready;
    session.tx_metadata = Vec::new();
    session.tx_timeout = None;
    // Preserve any existing freshness marker. Snapshot-backed backends never
    // advanced `last_write_at` for rolled-back writes, while no-snapshot
    // backends may already have made the write visible on the primary.
    // Restore the pre-BEGIN graph so that `db=` does not leak across TXs.
    if let Some(graph) = session.saved_graph.take() {
        session.current_graph = graph;
    }
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
    session.finish_explicit_transaction(false);
    session.write_permit = None;
    // Restore the pre-BEGIN graph so that `db=` does not leak across TXs.
    if let Some(graph) = session.saved_graph.take() {
        session.current_graph = graph;
    }
    session.state = ServerState::Ready;
    Ok(vec![ServerMessage::Success(bolt_map! {})])
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;
    use std::sync::atomic::AtomicU64;
    use std::time::{Duration, Instant};

    use gql_core::storage::{InMemoryGraphStore, ReadConsistency};

    fn ready_session() -> Session {
        Session {
            state: ServerState::Ready,
            store: Arc::new(InMemoryGraphStore::new()),
            conn_id: 1,
            current_graph: "neo4j".to_string(),
            pending_results: HashMap::new(),
            next_qid: 0,
            global_bookmark: Arc::new(crate::bookmark::BookmarkCounter::InMemory(AtomicU64::new(
                0,
            ))),
            tx_metadata: Vec::new(),
            tx_timeout: None,
            tx_store: None,
            write_semaphore: Arc::new(tokio::sync::Semaphore::new(128)),
            write_permit: None,
            saved_graph: None,
            tx_had_write: false,
            peer_ip: None,
            last_write_at: None,
        }
    }

    #[test]
    fn rollback_preserves_freshness_for_visible_tx_writes() {
        let mut session = ready_session();
        session.state = ServerState::TxReady;

        session.record_successful_write();
        assert!(session.last_write_at.is_some());

        handle_rollback(&mut session).unwrap();

        assert!(session.last_write_at.is_some());
        assert_eq!(
            session.read_consistency(),
            ReadConsistency::SessionFreshPrimary
        );
    }

    #[test]
    fn rollback_preserves_preexisting_freshness_window() {
        let mut session = ready_session();
        session.last_write_at = Some(Instant::now() - Duration::from_secs(1));
        session.state = ServerState::TxReady;
        session.tx_store = Some(Arc::new(InMemoryGraphStore::new()));

        handle_rollback(&mut session).unwrap();

        assert!(session.last_write_at.is_some());
        assert_eq!(
            session.read_consistency(),
            ReadConsistency::SessionFreshPrimary
        );
    }

    #[test]
    fn rollback_does_not_start_new_window_for_snapshot_transactions() {
        let mut session = ready_session();
        session.state = ServerState::TxReady;
        session.tx_store = Some(Arc::new(InMemoryGraphStore::new()));

        session.record_successful_write();
        assert!(session.last_write_at.is_none());

        handle_rollback(&mut session).unwrap();

        assert!(session.last_write_at.is_none());
        assert_eq!(session.read_consistency(), ReadConsistency::TopologyDefault);
    }

    #[test]
    fn read_only_commit_bookmark_does_not_rehydrate_freshness() {
        let global_bookmark = Arc::new(crate::bookmark::BookmarkCounter::InMemory(AtomicU64::new(
            9_100_000_000_000,
        )));
        let mut writer = ready_session();
        writer.global_bookmark = Arc::clone(&global_bookmark);
        writer.state = ServerState::TxReady;

        let commit_messages = handle_commit(&mut writer).unwrap();
        let bookmark = match &commit_messages[0] {
            ServerMessage::Success(meta) => meta
                .get_str("bookmark")
                .expect("commit should emit bookmark")
                .to_string(),
            other => panic!("expected success, got {:?}", other),
        };

        let mut reader = ready_session();
        reader.global_bookmark = global_bookmark;

        handle_begin(
            &mut reader,
            bolt_map! {
                "bookmarks" => BoltValue::List(vec![BoltValue::String(bookmark)])
            },
        )
        .unwrap();

        assert_eq!(reader.read_consistency(), ReadConsistency::TopologyDefault);
    }
}
