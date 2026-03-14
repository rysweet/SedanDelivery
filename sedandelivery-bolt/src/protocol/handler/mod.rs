//! Bolt protocol message handler.
//!
//! Dispatches incoming `ClientMessage` values to the appropriate handler
//! function, manages session state transitions, and produces
//! `ServerMessage` responses.

mod bookmarks;
mod explain;
mod hello;
mod pull;
mod run;
mod session;
mod transaction;

use crate::message::types::{ClientMessage, ServerMessage};
use crate::packstream::value::{BoltMap, BoltMapExt, BoltValue};
use crate::protocol::state::ServerState;

// Re-export public types from sub-modules.
pub(crate) use bookmarks::{next_bookmark, recent_bookmark_age, validate_bookmarks};
pub use hello::spawn_auth_rate_limiter_sweep;
#[cfg(test)]
pub(crate) use hello::with_required_password_override_for_tests;
pub(crate) use session::constant_time_eq;
pub use session::{AdminStats, PendingQuery, Session};

// -- Error type --------------------------------------------------------------

/// An error produced when the handler cannot process a message.
#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    /// The message is not valid in the current session state.
    #[error("message not accepted in current state {state:?}")]
    InvalidState { state: ServerState },

    /// An internal (non-protocol) error occurred.
    #[error("internal error: {0}")]
    Internal(String),
}

// -- Helper macro for building BoltMap literals ------------------------------

macro_rules! bolt_map {
    ($($key:expr => $val:expr),* $(,)?) => {{
        let entries: Vec<(String, BoltValue)> = vec![$(($key.to_string(), $val)),*];
        entries
    }};
}

// Re-export the macro for use in sub-modules.
pub(crate) use bolt_map;

// -- TX metadata extraction --------------------------------------------------

/// Extract transaction metadata from a message's extra map.
///
/// Drivers send metadata under either `"tx_metadata"` (Neo4j driver convention)
/// or `"metadata"` (Bolt spec). Returns the inner map, or an empty map if absent.
pub(crate) fn extract_tx_metadata(extra: &BoltMap) -> BoltMap {
    for key in &["tx_metadata", "metadata"] {
        if let Some(BoltValue::Map(m)) = extra.get_value(key) {
            return m.clone();
        }
    }
    Vec::new()
}

// -- Message handler ---------------------------------------------------------

/// Dispatch a `ClientMessage` to the appropriate handler, returning zero or
/// more `ServerMessage` responses.
///
/// This is the main entry point for protocol message processing. It enforces
/// the state machine (Failed/Interrupted handling) before delegating to the
/// per-message handler functions.
pub async fn handle_message(
    session: &mut Session,
    msg: ClientMessage,
) -> Result<Vec<ServerMessage>, ProtocolError> {
    // In Failed state, only RESET and GOODBYE are handled normally;
    // everything else gets IGNORED.
    if session.state == ServerState::Failed {
        match &msg {
            ClientMessage::Reset => {}
            ClientMessage::Goodbye => {}
            _ => return Ok(vec![ServerMessage::Ignored]),
        }
    }

    // In Interrupted state, only RESET is handled normally;
    // everything else gets IGNORED.
    if session.state == ServerState::Interrupted && !matches!(&msg, ClientMessage::Reset) {
        return Ok(vec![ServerMessage::Ignored]);
    }

    // Check state accepts the message.
    if !session.state.accepts(&msg) {
        let prev_state = session.state;
        session.state = ServerState::Failed;
        return Ok(vec![ServerMessage::Failure(bolt_map! {
            "code" => BoltValue::String("Neo.ClientError.Request.Invalid".to_string()),
            "message" => BoltValue::String(format!(
                "message not accepted in current state {:?}",
                prev_state
            ))
        })]);
    }

    match msg {
        ClientMessage::Hello(extra) => hello::handle_hello(session, extra).await,
        ClientMessage::Logon(auth) => hello::handle_logon(session, auth).await,
        ClientMessage::Run {
            query,
            params,
            extra,
        } => run::handle_run(session, query, params, extra).await,
        ClientMessage::Pull(extra) => pull::handle_pull(session, extra),
        ClientMessage::Discard(extra) => pull::handle_discard(session, extra),
        ClientMessage::Begin(extra) => transaction::handle_begin(session, extra),
        ClientMessage::Commit => transaction::handle_commit(session),
        ClientMessage::Rollback => transaction::handle_rollback(session),
        ClientMessage::Reset => transaction::handle_reset(session),
        ClientMessage::Goodbye => {
            // Handled in session loop; should not reach here.
            Ok(vec![])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gql_core::storage::{GraphStore, InMemoryGraphStore};
    use std::collections::HashMap;
    use std::sync::Arc;

    // -- Handler integration tests -------------------------------------------

    fn make_ready_session() -> Session {
        Session {
            state: ServerState::Ready,
            store: Arc::new(InMemoryGraphStore::new()),
            conn_id: 1,
            current_graph: "neo4j".to_string(),
            pending_results: HashMap::new(),
            next_qid: 0,
            global_bookmark: Arc::new(crate::bookmark::BookmarkCounter::from_env()),
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
    fn make_shared_sessions() -> (Session, Session) {
        let store: Arc<dyn GraphStore> = Arc::new(InMemoryGraphStore::new());
        let bookmark = Arc::new(crate::bookmark::BookmarkCounter::from_env());
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

    // -- Cross-session bookmark tests ----------------------------------------

    #[tokio::test]
    async fn cross_session_bookmark_from_pull_is_accepted_by_begin() {
        let (mut s1, mut s2) = make_shared_sessions();

        // Session 1: run a query and pull to get a bookmark.
        run::handle_run(&mut s1, "RETURN 1 AS n".to_string(), vec![], vec![])
            .await
            .unwrap();
        let pull_result =
            pull::handle_pull(&mut s1, bolt_map! { "n" => BoltValue::Int(-1) }).unwrap();
        let bookmark = match pull_result.last().unwrap() {
            ServerMessage::Success(meta) => meta.get_str("bookmark").unwrap().to_string(),
            other => panic!("expected Success, got {:?}", other),
        };
        assert!(bookmark.starts_with("neo4j:bookmark:v1:tx"));

        // Session 2: pass that bookmark into BEGIN -- should succeed.
        let begin_result = transaction::handle_begin(
            &mut s2,
            bolt_map! {
                "bookmarks" => BoltValue::List(vec![BoltValue::String(bookmark)])
            },
        )
        .unwrap();
        assert_eq!(begin_result.len(), 1);
        assert!(matches!(&begin_result[0], ServerMessage::Success(_)));
        assert_eq!(s2.state, ServerState::TxReady);
    }

    #[tokio::test]
    async fn cross_session_bookmark_counter_is_shared() {
        let (mut s1, mut s2) = make_shared_sessions();

        // Session 1: run + pull -> bookmark tx1.
        run::handle_run(&mut s1, "RETURN 1 AS n".to_string(), vec![], vec![])
            .await
            .unwrap();
        let r1 = pull::handle_pull(&mut s1, bolt_map! { "n" => BoltValue::Int(-1) }).unwrap();
        let bm1 = match r1.last().unwrap() {
            ServerMessage::Success(m) => m.get_str("bookmark").unwrap().to_string(),
            other => panic!("expected Success, got {:?}", other),
        };

        // Session 2: run + pull -> bookmark tx2 (not tx1 again).
        run::handle_run(&mut s2, "RETURN 2 AS n".to_string(), vec![], vec![])
            .await
            .unwrap();
        let r2 = pull::handle_pull(&mut s2, bolt_map! { "n" => BoltValue::Int(-1) }).unwrap();
        let bm2 = match r2.last().unwrap() {
            ServerMessage::Success(m) => m.get_str("bookmark").unwrap().to_string(),
            other => panic!("expected Success, got {:?}", other),
        };

        assert_eq!(bm1, "neo4j:bookmark:v1:tx1");
        assert_eq!(bm2, "neo4j:bookmark:v1:tx2");
    }

    // -- COMMIT bookmark tests -----------------------------------------------

    #[test]
    fn commit_returns_bookmark() {
        let mut session = make_ready_session();
        session.state = ServerState::TxReady;

        let result = transaction::handle_commit(&mut session).unwrap();
        assert_eq!(result.len(), 1);
        match &result[0] {
            ServerMessage::Success(meta) => {
                let bm = meta.get_str("bookmark").unwrap();
                assert!(bm.starts_with("neo4j:bookmark:v1:tx"));
            }
            other => panic!("expected Success, got {:?}", other),
        }
        assert_eq!(session.state, ServerState::Ready);
    }

    #[test]
    fn commit_clears_tx_metadata() {
        let mut session = make_ready_session();
        session.state = ServerState::TxReady;
        session.tx_metadata = vec![("key".to_string(), BoltValue::String("val".to_string()))];
        session.tx_timeout = Some(5000);

        transaction::handle_commit(&mut session).unwrap();
        assert!(session.tx_metadata.is_empty());
        assert!(session.tx_timeout.is_none());
    }

    // -- ROLLBACK tests ------------------------------------------------------

    #[test]
    fn rollback_returns_success_without_bookmark() {
        let mut session = make_ready_session();
        session.state = ServerState::TxReady;

        let result = transaction::handle_rollback(&mut session).unwrap();
        assert_eq!(result.len(), 1);
        match &result[0] {
            ServerMessage::Success(meta) => {
                // Rolled-back transaction must NOT produce a bookmark.
                assert!(
                    meta.get_str("bookmark").is_none(),
                    "rollback should not return a bookmark"
                );
            }
            other => panic!("expected Success, got {:?}", other),
        }
        assert_eq!(session.state, ServerState::Ready);
    }

    #[test]
    fn rollback_clears_tx_metadata() {
        let mut session = make_ready_session();
        session.state = ServerState::TxReady;
        session.tx_metadata = vec![("key".to_string(), BoltValue::String("val".to_string()))];
        session.tx_timeout = Some(5000);

        transaction::handle_rollback(&mut session).unwrap();
        assert!(session.tx_metadata.is_empty());
        assert!(session.tx_timeout.is_none());
    }

    // -- RESET tests ---------------------------------------------------------

    #[test]
    fn reset_clears_all_session_state() {
        let mut session = make_ready_session();
        session.state = ServerState::TxStreaming;
        session.tx_metadata = vec![("key".to_string(), BoltValue::String("val".to_string()))];
        session.tx_timeout = Some(5000);
        session.pending_results.insert(
            0,
            PendingQuery {
                records: vec![],
                query_type: "r".to_string(),
                stats: vec![],
                cursor: 0,
                start_time: std::time::Instant::now(),
                metadata: vec![],
                explain_plan: None,
                profile_plan: None,
                graph: "neo4j".to_string(),
                notifications: vec![],
                trailing_error: None,
                admin_stats: None,
            },
        );

        let result = transaction::handle_reset(&mut session).unwrap();
        assert_eq!(result.len(), 1);
        assert!(matches!(&result[0], ServerMessage::Success(_)));
        assert_eq!(session.state, ServerState::Ready);
        assert!(session.pending_results.is_empty());
        assert!(session.tx_metadata.is_empty());
        assert!(session.tx_timeout.is_none());
    }

    // -- BEGIN tests ---------------------------------------------------------

    #[test]
    fn begin_with_invalid_bookmark_returns_failure() {
        let mut session = make_ready_session();
        let extra: BoltMap = vec![(
            "bookmarks".to_string(),
            BoltValue::List(vec![BoltValue::String("invalid_bookmark".to_string())]),
        )];

        let result = transaction::handle_begin(&mut session, extra).unwrap();
        assert_eq!(result.len(), 1);
        match &result[0] {
            ServerMessage::Failure(meta) => {
                let code = meta.get_str("code").unwrap();
                assert_eq!(code, "Neo.ClientError.Transaction.InvalidBookmark");
            }
            other => panic!("expected Failure, got {:?}", other),
        }
        assert_eq!(session.state, ServerState::Failed);
    }

    #[test]
    fn begin_with_valid_sedan_bookmark_succeeds() {
        let mut session = make_ready_session();
        let extra: BoltMap = vec![(
            "bookmarks".to_string(),
            BoltValue::List(vec![BoltValue::String("sedan:tx:1".to_string())]),
        )];

        let result = transaction::handle_begin(&mut session, extra).unwrap();
        assert_eq!(result.len(), 1);
        assert!(matches!(&result[0], ServerMessage::Success(_)));
        assert_eq!(session.state, ServerState::TxReady);
    }

    #[test]
    fn begin_with_valid_neo4j_bookmark_succeeds() {
        let mut session = make_ready_session();
        let extra: BoltMap = vec![(
            "bookmarks".to_string(),
            BoltValue::List(vec![BoltValue::String(
                "neo4j:bookmark:v1:tx42".to_string(),
            )]),
        )];

        let result = transaction::handle_begin(&mut session, extra).unwrap();
        assert_eq!(result.len(), 1);
        assert!(matches!(&result[0], ServerMessage::Success(_)));
        assert_eq!(session.state, ServerState::TxReady);
    }

    #[test]
    fn begin_with_no_bookmarks_succeeds() {
        let mut session = make_ready_session();
        let extra: BoltMap = vec![];

        let result = transaction::handle_begin(&mut session, extra).unwrap();
        assert_eq!(result.len(), 1);
        assert!(matches!(&result[0], ServerMessage::Success(_)));
        assert_eq!(session.state, ServerState::TxReady);
    }

    #[test]
    fn begin_stores_tx_metadata() {
        let mut session = make_ready_session();
        let inner: BoltMap = vec![("app".to_string(), BoltValue::String("test".to_string()))];
        let extra: BoltMap = vec![("tx_metadata".to_string(), BoltValue::Map(inner.clone()))];

        transaction::handle_begin(&mut session, extra).unwrap();
        assert_eq!(session.tx_metadata, inner);
    }

    // -- RUN bookmark tests --------------------------------------------------

    #[tokio::test]
    async fn run_with_invalid_bookmark_returns_failure() {
        let mut session = make_ready_session();
        let extra: BoltMap = vec![(
            "bookmarks".to_string(),
            BoltValue::List(vec![BoltValue::String("invalid_bookmark".to_string())]),
        )];

        let result = run::handle_run(&mut session, "RETURN 1".to_string(), vec![], extra)
            .await
            .unwrap();

        assert_eq!(result.len(), 1);
        match &result[0] {
            ServerMessage::Failure(meta) => {
                let code = meta.get_str("code").unwrap();
                assert_eq!(code, "Neo.ClientError.Transaction.InvalidBookmark");
            }
            other => panic!("expected Failure, got {:?}", other),
        }
        assert_eq!(session.state, ServerState::Failed);
    }

    #[tokio::test]
    async fn run_with_valid_bookmark_succeeds() {
        let mut session = make_ready_session();
        let extra: BoltMap = vec![(
            "bookmarks".to_string(),
            BoltValue::List(vec![BoltValue::String("sedan:tx:5".to_string())]),
        )];

        let result = run::handle_run(&mut session, "RETURN 1 AS n".to_string(), vec![], extra)
            .await
            .unwrap();

        assert_eq!(result.len(), 1);
        assert!(matches!(&result[0], ServerMessage::Success(_)));
    }

    // -- Snapshot transaction isolation tests --------------------------------

    /// Helper: run a query inside a session and pull all results. Returns the
    /// PULL SUCCESS metadata.
    async fn run_and_pull(session: &mut Session, query: &str) -> Vec<ServerMessage> {
        let run_result = run::handle_run(session, query.to_string(), vec![], vec![])
            .await
            .unwrap();
        assert!(
            matches!(&run_result[0], ServerMessage::Success(_)),
            "RUN failed: {:?}",
            run_result
        );
        pull::handle_pull(session, bolt_map! { "n" => BoltValue::Int(-1) }).unwrap()
    }

    #[tokio::test]
    async fn begin_creates_tx_store() {
        let mut session = make_ready_session();
        assert!(session.tx_store.is_none());

        transaction::handle_begin(&mut session, vec![]).unwrap();
        assert!(session.tx_store.is_some());
        assert_eq!(session.state, ServerState::TxReady);
    }

    #[tokio::test]
    async fn commit_clears_tx_store() {
        let mut session = make_ready_session();
        transaction::handle_begin(&mut session, vec![]).unwrap();
        assert!(session.tx_store.is_some());

        transaction::handle_commit(&mut session).unwrap();
        assert!(session.tx_store.is_none());
    }

    #[tokio::test]
    async fn rollback_clears_tx_store() {
        let mut session = make_ready_session();
        transaction::handle_begin(&mut session, vec![]).unwrap();
        assert!(session.tx_store.is_some());

        transaction::handle_rollback(&mut session).unwrap();
        assert!(session.tx_store.is_none());
    }

    #[tokio::test]
    async fn reset_clears_tx_store() {
        let mut session = make_ready_session();
        transaction::handle_begin(&mut session, vec![]).unwrap();
        assert!(session.tx_store.is_some());

        transaction::handle_reset(&mut session).unwrap();
        assert!(session.tx_store.is_none());
    }

    #[tokio::test]
    async fn tx_read_your_writes() {
        let mut session = make_ready_session();

        // BEGIN, CREATE inside the TX, then read it back within the same TX.
        transaction::handle_begin(&mut session, vec![]).unwrap();
        run_and_pull(&mut session, "CREATE (:TxNode {val: 42})").await;

        let pull_msgs = run_and_pull(&mut session, "MATCH (n:TxNode) RETURN count(n) AS c").await;

        // Should have 1 RECORD with c=1, then SUCCESS
        let record = pull_msgs
            .iter()
            .find(|m| matches!(m, ServerMessage::Record(_)));
        assert!(record.is_some(), "expected a record");
        if let ServerMessage::Record(fields) = record.unwrap() {
            assert_eq!(fields[0], BoltValue::Int(1));
        }
    }

    #[tokio::test]
    async fn rollback_discards_mutations() {
        let (mut s1, mut s2) = make_shared_sessions();

        // Session 1: BEGIN, CREATE, ROLLBACK
        transaction::handle_begin(&mut s1, vec![]).unwrap();
        run_and_pull(&mut s1, "CREATE (:RollbackNode {val: 1})").await;
        transaction::handle_rollback(&mut s1).unwrap();

        // Session 2: the node should not be visible in the main store.
        let pull_msgs = run_and_pull(&mut s2, "MATCH (n:RollbackNode) RETURN count(n) AS c").await;
        let record = pull_msgs
            .iter()
            .find(|m| matches!(m, ServerMessage::Record(_)));
        assert!(record.is_some(), "expected a record");
        if let ServerMessage::Record(fields) = record.unwrap() {
            assert_eq!(
                fields[0],
                BoltValue::Int(0),
                "rollback should discard mutations"
            );
        }
    }

    #[tokio::test]
    async fn commit_makes_mutations_visible() {
        let (mut s1, mut s2) = make_shared_sessions();

        // Session 1: BEGIN, CREATE, COMMIT
        transaction::handle_begin(&mut s1, vec![]).unwrap();
        run_and_pull(&mut s1, "CREATE (:CommitNode {val: 2})").await;
        transaction::handle_commit(&mut s1).unwrap();

        // Session 2: the node should be visible.
        let pull_msgs = run_and_pull(&mut s2, "MATCH (n:CommitNode) RETURN count(n) AS c").await;
        let record = pull_msgs
            .iter()
            .find(|m| matches!(m, ServerMessage::Record(_)));
        assert!(record.is_some(), "expected a record");
        if let ServerMessage::Record(fields) = record.unwrap() {
            assert_eq!(
                fields[0],
                BoltValue::Int(1),
                "commit should make mutations visible"
            );
        }
    }

    #[tokio::test]
    async fn commit_refreshes_last_write_at_for_snapshot_transactions() {
        let mut session = make_ready_session();
        transaction::handle_begin(&mut session, vec![]).unwrap();
        session.last_write_at = Some(
            std::time::Instant::now()
                - crate::protocol::handler::session::READ_AFTER_WRITE_WINDOW
                - std::time::Duration::from_secs(1),
        );

        run_and_pull(&mut session, "CREATE (:CommitRefreshNode {val: 1})").await;
        assert_eq!(
            session.read_consistency(),
            gql_core::storage::ReadConsistency::TopologyDefault
        );

        transaction::handle_commit(&mut session).unwrap();
        assert_eq!(
            session.read_consistency(),
            gql_core::storage::ReadConsistency::SessionFreshPrimary
        );
    }

    #[tokio::test]
    async fn create_index_marks_session_fresh_for_read_after_write() {
        let mut session = make_ready_session();

        run::handle_run(
            &mut session,
            "CREATE INDEX FOR (n:ReadAfterWriteIndexNode) ON (n.key)".to_string(),
            vec![],
            vec![],
        )
        .await
        .unwrap();

        assert_eq!(
            session.read_consistency(),
            gql_core::storage::ReadConsistency::SessionFreshPrimary
        );
    }

    #[tokio::test]
    async fn uncommitted_mutations_not_visible_to_other_sessions() {
        let (mut s1, mut s2) = make_shared_sessions();

        // Session 1: BEGIN, CREATE (but do NOT commit yet)
        transaction::handle_begin(&mut s1, vec![]).unwrap();
        run_and_pull(&mut s1, "CREATE (:IsolatedNode {val: 3})").await;

        // Session 2: the node should NOT be visible because s1 has not committed.
        let pull_msgs = run_and_pull(&mut s2, "MATCH (n:IsolatedNode) RETURN count(n) AS c").await;
        let record = pull_msgs
            .iter()
            .find(|m| matches!(m, ServerMessage::Record(_)));
        assert!(record.is_some(), "expected a record");
        if let ServerMessage::Record(fields) = record.unwrap() {
            assert_eq!(
                fields[0],
                BoltValue::Int(0),
                "uncommitted mutations should be invisible"
            );
        }
    }

    // -- QID support tests ---------------------------------------------------

    #[tokio::test]
    async fn run_returns_qid_in_success() {
        let mut session = make_ready_session();
        let result = run::handle_run(&mut session, "RETURN 1 AS n".to_string(), vec![], vec![])
            .await
            .unwrap();
        assert_eq!(result.len(), 1);
        match &result[0] {
            ServerMessage::Success(meta) => {
                let qid = meta.get_int("qid");
                assert!(qid.is_some(), "RUN SUCCESS must include qid");
                assert_eq!(qid.unwrap(), 0);
            }
            other => panic!("expected Success, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn qid_increments_across_runs() {
        let mut session = make_ready_session();
        // First RUN
        let r1 = run::handle_run(&mut session, "RETURN 1 AS n".to_string(), vec![], vec![])
            .await
            .unwrap();
        // Pull to clear the first result
        pull::handle_pull(&mut session, bolt_map! { "n" => BoltValue::Int(-1) }).unwrap();

        // Second RUN
        let r2 = run::handle_run(&mut session, "RETURN 2 AS m".to_string(), vec![], vec![])
            .await
            .unwrap();
        let qid1 = match &r1[0] {
            ServerMessage::Success(m) => m.get_int("qid").unwrap(),
            _ => panic!("expected Success"),
        };
        let qid2 = match &r2[0] {
            ServerMessage::Success(m) => m.get_int("qid").unwrap(),
            _ => panic!("expected Success"),
        };
        assert_eq!(qid1, 0);
        assert_eq!(qid2, 1);
    }

    #[tokio::test]
    async fn pull_uses_qid_from_extra() {
        let mut session = make_ready_session();
        // Begin a TX so we can have multiple concurrent results.
        transaction::handle_begin(&mut session, vec![]).unwrap();

        // First RUN (qid=0)
        let r1 = run::handle_run(&mut session, "RETURN 1 AS n".to_string(), vec![], vec![])
            .await
            .unwrap();
        let qid0 = match &r1[0] {
            ServerMessage::Success(m) => m.get_int("qid").unwrap(),
            _ => panic!("expected Success"),
        };

        // Second RUN (qid=1)
        let r2 = run::handle_run(&mut session, "RETURN 2 AS m".to_string(), vec![], vec![])
            .await
            .unwrap();
        let qid1 = match &r2[0] {
            ServerMessage::Success(m) => m.get_int("qid").unwrap(),
            _ => panic!("expected Success"),
        };

        // Pull from qid=0 specifically
        let pull0 = pull::handle_pull(
            &mut session,
            bolt_map! { "n" => BoltValue::Int(-1), "qid" => BoltValue::Int(qid0) },
        )
        .unwrap();
        let record0 = pull0.iter().find(|m| matches!(m, ServerMessage::Record(_)));
        assert!(record0.is_some(), "expected record from qid0");
        if let ServerMessage::Record(fields) = record0.unwrap() {
            assert_eq!(fields[0], BoltValue::Int(1));
        }

        // Pull from qid=1 specifically
        let pull1 = pull::handle_pull(
            &mut session,
            bolt_map! { "n" => BoltValue::Int(-1), "qid" => BoltValue::Int(qid1) },
        )
        .unwrap();
        let record1 = pull1.iter().find(|m| matches!(m, ServerMessage::Record(_)));
        assert!(record1.is_some(), "expected record from qid1");
        if let ServerMessage::Record(fields) = record1.unwrap() {
            assert_eq!(fields[0], BoltValue::Int(2));
        }
    }

    // -- DB metadata from effective graph tests ------------------------------

    #[tokio::test]
    async fn pull_db_reflects_run_extra_db() {
        let mut session = make_ready_session();
        // RUN with db=custom_graph
        let extra: BoltMap = vec![(
            "db".to_string(),
            BoltValue::String("custom_graph".to_string()),
        )];
        run::handle_run(&mut session, "RETURN 1 AS n".to_string(), vec![], extra)
            .await
            .unwrap();
        let pull_result =
            pull::handle_pull(&mut session, bolt_map! { "n" => BoltValue::Int(-1) }).unwrap();
        let success = pull_result.last().unwrap();
        match success {
            ServerMessage::Success(meta) => {
                let db = meta.get_str("db").unwrap();
                assert_eq!(db, "custom_graph");
            }
            other => panic!("expected Success, got {:?}", other),
        }
    }

    // -- Pending results limit tests -----------------------------------------

    #[tokio::test]
    async fn run_rejects_when_pending_results_at_cap() {
        let mut session = make_ready_session();
        // Begin a TX so multiple pending results can coexist.
        transaction::handle_begin(&mut session, vec![]).unwrap();

        // Default cap is 16 (DEFAULT_MAX_PENDING_RESULTS).
        let cap: usize = 16;
        // Fill up to the cap without pulling.
        for i in 0..cap {
            let result =
                run::handle_run(&mut session, format!("RETURN {} AS n", i), vec![], vec![])
                    .await
                    .unwrap();
            assert!(
                matches!(&result[0], ServerMessage::Success(_)),
                "RUN {} should succeed, got {:?}",
                i,
                result
            );
        }

        assert_eq!(session.pending_results.len(), cap);

        // The next RUN should be rejected.
        let rejected = run::handle_run(
            &mut session,
            "RETURN 'one too many' AS x".to_string(),
            vec![],
            vec![],
        )
        .await
        .unwrap();
        assert_eq!(rejected.len(), 1);
        match &rejected[0] {
            ServerMessage::Failure(meta) => {
                let code = meta.get_str("code").unwrap();
                assert_eq!(code, "Neo.ClientError.Request.Invalid");
                let msg = meta.get_str("message").unwrap();
                assert!(msg.contains("Too many pending results"));
            }
            other => panic!("expected Failure, got {:?}", other),
        }
        assert_eq!(session.state, ServerState::Failed);
    }

    #[tokio::test]
    async fn run_succeeds_after_pulling_pending_result() {
        let mut session = make_ready_session();
        // Begin a TX so multiple pending results can coexist.
        transaction::handle_begin(&mut session, vec![]).unwrap();

        // Default cap is 16.
        let cap: usize = 16;
        // Fill up to the cap.
        for i in 0..cap {
            run::handle_run(&mut session, format!("RETURN {} AS n", i), vec![], vec![])
                .await
                .unwrap();
        }

        // Pull one result to free a slot (pull qid=0).
        pull::handle_pull(
            &mut session,
            bolt_map! { "n" => BoltValue::Int(-1), "qid" => BoltValue::Int(0) },
        )
        .unwrap();

        assert_eq!(session.pending_results.len(), cap - 1);

        // Now a new RUN should succeed.
        let result = run::handle_run(
            &mut session,
            "RETURN 'after pull' AS x".to_string(),
            vec![],
            vec![],
        )
        .await
        .unwrap();
        assert!(
            matches!(&result[0], ServerMessage::Success(_)),
            "RUN after pull should succeed, got {:?}",
            result
        );
    }

    // -- Notification detection tests ----------------------------------------

    // -- Parameterized query tests -------------------------------------------

    #[tokio::test]
    async fn run_with_params_resolves_parameter_in_return() {
        let mut session = make_ready_session();
        let params: BoltMap = vec![("x".to_string(), BoltValue::Int(42))];
        let result = run::handle_run(&mut session, "RETURN $x AS val".to_string(), params, vec![])
            .await
            .unwrap();
        assert!(matches!(&result[0], ServerMessage::Success(_)));

        let pull_result =
            pull::handle_pull(&mut session, bolt_map! { "n" => BoltValue::Int(-1) }).unwrap();
        let record = pull_result
            .iter()
            .find(|m| matches!(m, ServerMessage::Record(_)));
        assert!(
            record.is_some(),
            "expected a record from parameterized query"
        );
        if let ServerMessage::Record(fields) = record.unwrap() {
            assert_eq!(
                fields[0],
                BoltValue::Int(42),
                "parameter $x should resolve to 42"
            );
        }
    }

    #[tokio::test]
    async fn run_with_string_param_resolves() {
        let mut session = make_ready_session();
        let params: BoltMap = vec![("name".to_string(), BoltValue::String("Alice".to_string()))];
        let result = run::handle_run(
            &mut session,
            "RETURN $name AS who".to_string(),
            params,
            vec![],
        )
        .await
        .unwrap();
        assert!(matches!(&result[0], ServerMessage::Success(_)));

        let pull_result =
            pull::handle_pull(&mut session, bolt_map! { "n" => BoltValue::Int(-1) }).unwrap();
        let record = pull_result
            .iter()
            .find(|m| matches!(m, ServerMessage::Record(_)));
        assert!(record.is_some(), "expected a record");
        if let ServerMessage::Record(fields) = record.unwrap() {
            assert_eq!(
                fields[0],
                BoltValue::String("Alice".to_string()),
                "parameter $name should resolve to 'Alice'"
            );
        }
    }

    #[tokio::test]
    async fn run_with_multiple_params() {
        let mut session = make_ready_session();
        let params: BoltMap = vec![
            ("a".to_string(), BoltValue::Int(10)),
            ("b".to_string(), BoltValue::Int(20)),
        ];
        let result = run::handle_run(
            &mut session,
            "RETURN $a + $b AS sum".to_string(),
            params,
            vec![],
        )
        .await
        .unwrap();
        assert!(matches!(&result[0], ServerMessage::Success(_)));

        let pull_result =
            pull::handle_pull(&mut session, bolt_map! { "n" => BoltValue::Int(-1) }).unwrap();
        let record = pull_result
            .iter()
            .find(|m| matches!(m, ServerMessage::Record(_)));
        assert!(record.is_some(), "expected a record");
        if let ServerMessage::Record(fields) = record.unwrap() {
            assert_eq!(fields[0], BoltValue::Int(30), "$a + $b should be 30");
        }
    }

    // -- Nested BEGIN rejection tests ----------------------------------------

    #[tokio::test]
    async fn nested_begin_returns_failure() {
        let mut session = make_ready_session();

        // First BEGIN succeeds.
        let result = handle_message(&mut session, ClientMessage::Begin(vec![]))
            .await
            .unwrap();
        assert!(matches!(&result[0], ServerMessage::Success(_)));
        assert_eq!(session.state, ServerState::TxReady);

        // Second BEGIN while already in TxReady should fail.
        let result = handle_message(&mut session, ClientMessage::Begin(vec![]))
            .await
            .unwrap();
        assert_eq!(result.len(), 1);
        match &result[0] {
            ServerMessage::Failure(meta) => {
                let code = meta.get_str("code").unwrap();
                assert_eq!(code, "Neo.ClientError.Request.Invalid");
                let msg = meta.get_str("message").unwrap();
                assert!(
                    msg.contains("TxReady"),
                    "error message should mention TxReady state, got: {}",
                    msg
                );
            }
            other => panic!("expected Failure for nested BEGIN, got {:?}", other),
        }
        assert_eq!(session.state, ServerState::Failed);
    }

    #[tokio::test]
    async fn nested_begin_in_tx_streaming_returns_failure() {
        let mut session = make_ready_session();

        // BEGIN, then RUN to move to TxStreaming.
        handle_message(&mut session, ClientMessage::Begin(vec![]))
            .await
            .unwrap();
        handle_message(
            &mut session,
            ClientMessage::Run {
                query: "RETURN 1 AS n".to_string(),
                params: vec![],
                extra: vec![],
            },
        )
        .await
        .unwrap();
        assert_eq!(session.state, ServerState::TxStreaming);

        // BEGIN while in TxStreaming should fail.
        let result = handle_message(&mut session, ClientMessage::Begin(vec![]))
            .await
            .unwrap();
        assert_eq!(result.len(), 1);
        match &result[0] {
            ServerMessage::Failure(meta) => {
                let code = meta.get_str("code").unwrap();
                assert_eq!(code, "Neo.ClientError.Request.Invalid");
            }
            other => panic!("expected Failure for nested BEGIN, got {:?}", other),
        }
        assert_eq!(session.state, ServerState::Failed);
    }

    // -- RUN after ROLLBACK tests --------------------------------------------

    #[tokio::test]
    async fn run_succeeds_after_rollback() {
        let mut session = make_ready_session();

        // BEGIN -> CREATE -> ROLLBACK
        transaction::handle_begin(&mut session, vec![]).unwrap();
        run_and_pull(&mut session, "CREATE (:Temp {val: 1})").await;
        transaction::handle_rollback(&mut session).unwrap();
        assert_eq!(session.state, ServerState::Ready);

        // RUN after ROLLBACK should work normally (auto-commit mode).
        let result = run::handle_run(
            &mut session,
            "RETURN 42 AS answer".to_string(),
            vec![],
            vec![],
        )
        .await
        .unwrap();
        assert!(
            matches!(&result[0], ServerMessage::Success(_)),
            "RUN after ROLLBACK should succeed, got {:?}",
            result
        );

        let pull_msgs =
            pull::handle_pull(&mut session, bolt_map! { "n" => BoltValue::Int(-1) }).unwrap();
        let record = pull_msgs
            .iter()
            .find(|m| matches!(m, ServerMessage::Record(_)));
        assert!(record.is_some(), "expected a record after ROLLBACK + RUN");
        if let ServerMessage::Record(fields) = record.unwrap() {
            assert_eq!(fields[0], BoltValue::Int(42));
        }
    }

    #[tokio::test]
    async fn run_after_rollback_does_not_see_rolled_back_data() {
        let mut session = make_ready_session();

        // BEGIN -> CREATE -> ROLLBACK
        transaction::handle_begin(&mut session, vec![]).unwrap();
        run_and_pull(&mut session, "CREATE (:Ghost {val: 99})").await;
        transaction::handle_rollback(&mut session).unwrap();

        // Query for the rolled-back node in auto-commit mode -- should not exist.
        let pull_msgs = run_and_pull(&mut session, "MATCH (n:Ghost) RETURN count(n) AS c").await;
        let record = pull_msgs
            .iter()
            .find(|m| matches!(m, ServerMessage::Record(_)));
        assert!(record.is_some(), "expected a record");
        if let ServerMessage::Record(fields) = record.unwrap() {
            assert_eq!(
                fields[0],
                BoltValue::Int(0),
                "rolled-back data should not be visible after ROLLBACK"
            );
        }
    }

    // -- Full transaction lifecycle test -------------------------------------

    #[tokio::test]
    async fn full_transaction_lifecycle() {
        let mut session = make_ready_session();

        // 1. BEGIN
        let begin_result = handle_message(&mut session, ClientMessage::Begin(vec![]))
            .await
            .unwrap();
        assert!(matches!(&begin_result[0], ServerMessage::Success(_)));
        assert_eq!(session.state, ServerState::TxReady);

        // 2. RUN inside TX
        let run_result = handle_message(
            &mut session,
            ClientMessage::Run {
                query: "CREATE (:LifecycleNode {step: 1})".to_string(),
                params: vec![],
                extra: vec![],
            },
        )
        .await
        .unwrap();
        assert!(matches!(&run_result[0], ServerMessage::Success(_)));
        assert_eq!(session.state, ServerState::TxStreaming);

        // 3. PULL
        let pull_result = handle_message(
            &mut session,
            ClientMessage::Pull(bolt_map! { "n" => BoltValue::Int(-1) }),
        )
        .await
        .unwrap();
        let last = pull_result.last().unwrap();
        assert!(matches!(last, ServerMessage::Success(_)));
        assert_eq!(session.state, ServerState::TxReady);

        // 4. COMMIT
        let commit_result = handle_message(&mut session, ClientMessage::Commit)
            .await
            .unwrap();
        assert!(matches!(&commit_result[0], ServerMessage::Success(_)));
        assert_eq!(session.state, ServerState::Ready);

        // 5. Verify the committed data is visible
        let pull_msgs =
            run_and_pull(&mut session, "MATCH (n:LifecycleNode) RETURN count(n) AS c").await;
        let record = pull_msgs
            .iter()
            .find(|m| matches!(m, ServerMessage::Record(_)));
        if let ServerMessage::Record(fields) = record.unwrap() {
            assert_eq!(fields[0], BoltValue::Int(1));
        }
    }

    // -- Notification detection tests ----------------------------------------

    #[tokio::test]
    async fn explain_cartesian_product_generates_notification() {
        let mut session = make_ready_session();
        run::handle_run(
            &mut session,
            "EXPLAIN MATCH (n), (m) RETURN n, m".to_string(),
            vec![],
            vec![],
        )
        .await
        .unwrap();
        let pull_result =
            pull::handle_pull(&mut session, bolt_map! { "n" => BoltValue::Int(-1) }).unwrap();
        let success = pull_result.last().unwrap();
        match success {
            ServerMessage::Success(meta) => {
                let notifications = meta.get_value("notifications");
                assert!(notifications.is_some(), "must have notifications field");
                if let Some(BoltValue::List(items)) = notifications {
                    assert_eq!(items.len(), 1, "expected 1 notification");
                    if let BoltValue::Map(entries) = &items[0] {
                        let code = entries.get_str("code").unwrap();
                        assert!(code.contains("CartesianProduct"));
                    } else {
                        panic!("expected Map for notification");
                    }
                } else {
                    panic!("expected notifications to be a List");
                }
            }
            other => panic!("expected Success, got {:?}", other),
        }
    }

    // -- PROFILE tests -------------------------------------------------------

    #[tokio::test]
    async fn profile_returns_results_and_plan_with_row_counts() {
        let mut session = make_ready_session();
        // Create some data first
        run_and_pull(&mut session, "CREATE (:Pnode {v: 1}), (:Pnode {v: 2})").await;

        // Run a PROFILE query
        let run_result = run::handle_run(
            &mut session,
            "PROFILE MATCH (n:Pnode) RETURN n".to_string(),
            vec![],
            vec![],
        )
        .await
        .unwrap();
        assert!(
            matches!(&run_result[0], ServerMessage::Success(_)),
            "PROFILE RUN should succeed, got {:?}",
            run_result
        );

        let pull_result =
            pull::handle_pull(&mut session, bolt_map! { "n" => BoltValue::Int(-1) }).unwrap();

        // Should have RECORD messages (the actual query results)
        let records: Vec<_> = pull_result
            .iter()
            .filter(|m| matches!(m, ServerMessage::Record(_)))
            .collect();
        assert_eq!(records.len(), 2, "PROFILE should return 2 result rows");

        // The final SUCCESS should contain a profile plan
        let success = pull_result.last().unwrap();
        match success {
            ServerMessage::Success(meta) => {
                let profile = meta.get_value("profile");
                assert!(
                    profile.is_some(),
                    "PULL SUCCESS must include 'profile' field"
                );
                if let Some(BoltValue::Map(plan_entries)) = profile {
                    // Plan should have operatorType
                    let op_type = plan_entries.get_str("operatorType");
                    assert!(op_type.is_some(), "plan should have operatorType");

                    // Plan should have rows field with actual count
                    let rows = plan_entries.get_int("rows");
                    assert!(rows.is_some(), "plan should have rows field");
                    assert!(rows.unwrap() > 0, "root operator rows should be non-zero");

                    // Plan should have dbHits field
                    let db_hits = plan_entries.get_int("dbHits");
                    assert!(db_hits.is_some(), "plan should have dbHits field");
                } else {
                    panic!("expected profile to be a Map");
                }
            }
            other => panic!("expected Success, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn profile_simple_return_produces_plan() {
        let mut session = make_ready_session();
        let run_result = run::handle_run(
            &mut session,
            "PROFILE RETURN 1 AS x".to_string(),
            vec![],
            vec![],
        )
        .await
        .unwrap();
        assert!(matches!(&run_result[0], ServerMessage::Success(_)));

        let pull_result =
            pull::handle_pull(&mut session, bolt_map! { "n" => BoltValue::Int(-1) }).unwrap();

        // Should have 1 record
        let records: Vec<_> = pull_result
            .iter()
            .filter(|m| matches!(m, ServerMessage::Record(_)))
            .collect();
        assert_eq!(records.len(), 1);

        // Final SUCCESS should have profile
        let success = pull_result.last().unwrap();
        match success {
            ServerMessage::Success(meta) => {
                assert!(
                    meta.get_value("profile").is_some(),
                    "PROFILE RETURN 1 should produce a profile plan"
                );
            }
            other => panic!("expected Success, got {:?}", other),
        }
    }
}
