//! Integration tests for the Bolt protocol message handler.
//!
//! Tests are split into focused thematic submodules. Shared helpers live in
//! this file and are re-used by every submodule via `use super::*`.

use super::*;
use cypher_core::storage::{GraphStore, InMemoryGraphStore};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
pub(super) use std::sync::Arc;

mod bookmarks;
mod cross_driver;
mod discard;
mod explain_profile;
mod qid_pull;
mod snapshot_isolation;
mod transaction;

// -- Shared helpers ----------------------------------------------------------

pub(super) fn isolated_bookmark_counter() -> Arc<crate::bookmark::BookmarkCounter> {
    static NEXT_TEST_BOOKMARK: AtomicU64 = AtomicU64::new(9_200_000_000_000);
    Arc::new(crate::bookmark::BookmarkCounter::InMemory(AtomicU64::new(
        NEXT_TEST_BOOKMARK.fetch_add(10_000, Ordering::SeqCst),
    )))
}

pub(super) fn bookmark_tx_id(bookmark: &str) -> u64 {
    bookmark
        .strip_prefix("neo4j:bookmark:v1:tx")
        .expect("bookmark should use neo4j tx format")
        .parse()
        .expect("bookmark tx id should parse")
}

pub(super) fn make_ready_session() -> Session {
    Session {
        state: ServerState::Ready,
        store: Arc::new(InMemoryGraphStore::new()),
        conn_id: 1,
        current_graph: "neo4j".to_string(),
        pending_results: HashMap::new(),
        next_qid: 0,
        // Keep test bookmark ids isolated because timestamp freshness is
        // tracked in a process-global cache keyed by tx id.
        global_bookmark: isolated_bookmark_counter(),
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

/// Build two sessions that share the same store and global bookmark counter,
/// simulating two connections to the same server.
pub(super) fn make_shared_sessions() -> (Session, Session) {
    let store: Arc<dyn GraphStore> = Arc::new(InMemoryGraphStore::new());
    let bookmark = isolated_bookmark_counter();
    let s1 = Session {
        state: ServerState::Ready,
        store: Arc::clone(&store),
        conn_id: 1,
        current_graph: "neo4j".to_string(),
        pending_results: HashMap::new(),
        next_qid: 0,
        global_bookmark: Arc::clone(&bookmark),
        tx_metadata: Vec::new(),
        tx_timeout: None,
        tx_store: None,
        write_semaphore: Arc::new(tokio::sync::Semaphore::new(128)),
        write_permit: None,
        saved_graph: None,
        tx_had_write: false,
        peer_ip: None,
        last_write_at: None,
    };
    let s2 = Session {
        state: ServerState::Ready,
        store: Arc::clone(&store),
        conn_id: 2,
        current_graph: "neo4j".to_string(),
        pending_results: HashMap::new(),
        next_qid: 0,
        global_bookmark: Arc::clone(&bookmark),
        tx_metadata: Vec::new(),
        tx_timeout: None,
        tx_store: None,
        write_semaphore: Arc::new(tokio::sync::Semaphore::new(128)),
        write_permit: None,
        saved_graph: None,
        tx_had_write: false,
        peer_ip: None,
        last_write_at: None,
    };
    (s1, s2)
}

/// Run a query inside a session and pull all results. Returns the PULL
/// SUCCESS metadata plus any record messages.
pub(super) async fn run_and_pull(session: &mut Session, query: &str) -> Vec<ServerMessage> {
    let run_result = run::handle_run(session, query.to_string(), vec![], vec![])
        .await
        .unwrap();
    assert!(
        matches!(&run_result[0], ServerMessage::Success(_)),
        "RUN failed: {:?}",
        run_result
    );
    pull::handle_pull(session, bolt_map! { "n" => BoltValue::Int(-1) })
        .await
        .unwrap()
}
