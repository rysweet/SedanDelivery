//! Bolt protocol message handler.
//!
//! Dispatches incoming `ClientMessage` values to the appropriate handler
//! function, manages session state transitions, and produces
//! `ServerMessage` responses.

mod hello;
mod pull;
mod run;
mod transaction;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::message::types::{ClientMessage, ServerMessage};
use crate::packstream::value::{BoltMap, BoltMapExt, BoltValue};
use crate::protocol::state::ServerState;
use gql_core::storage::GraphStore;

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

// -- Session types -----------------------------------------------------------

/// Per-connection session state for a single Bolt client.
///
/// Tracks the protocol state machine, the backing graph store, pending
/// query results, and transaction metadata.
pub struct Session {
    /// Current protocol state (Authentication, Ready, Streaming, etc.).
    pub state: ServerState,
    /// Shared graph store.
    pub store: Arc<dyn GraphStore>,
    /// Unique connection identifier for logging.
    pub conn_id: u64,
    /// The current default graph (database) for this session.
    pub current_graph: String,
    /// Multiple concurrent result streams keyed by query ID (qid).
    pub pending_results: HashMap<i64, PendingQuery>,
    /// Next query ID to assign.
    pub next_qid: i64,
    /// Global bookmark counter shared across all sessions connected to the same
    /// store. Bookmarks produced by one session are therefore meaningful when
    /// passed to another session, which is required by the Neo4j driver testkit.
    pub global_bookmark: Arc<AtomicU64>,
    /// TX metadata set via BEGIN extra map, echoed back on PULL completion.
    pub tx_metadata: BoltMap,
    /// TX-level timeout set via BEGIN extra map (milliseconds).
    pub tx_timeout: Option<u64>,
    /// Snapshot store for explicit transactions. When set, RUN executes
    /// against this store instead of the main store. COMMIT merges it back,
    /// ROLLBACK discards it.
    pub tx_store: Option<Arc<dyn GraphStore>>,
    /// Write concurrency semaphore for transaction timeout contention.
    /// Autocommit writes without tx_timeout bypass this (full concurrency).
    pub write_semaphore: Arc<tokio::sync::Semaphore>,
    /// Permit held while this session holds a write slot (during a write TX).
    pub write_permit: Option<tokio::sync::OwnedSemaphorePermit>,
}

/// A pending query result awaiting PULL or DISCARD.
pub struct PendingQuery {
    /// Column names from the query result.
    pub columns: Vec<String>,
    /// Result rows, each row being a list of `BoltValue` in column order.
    pub records: Vec<Vec<BoltValue>>,
    /// Query type string: `"r"` (read), `"w"` (write), or `"rw"` (both).
    pub query_type: String,
    /// Non-zero mutation statistics.
    pub stats: BoltMap,
    /// Current cursor position into `records`.
    pub cursor: usize,
    /// When the query started executing (for `t_last` computation).
    pub start_time: std::time::Instant,
    /// TX metadata from RUN extra map, echoed back on PULL completion.
    pub metadata: BoltMap,
    /// EXPLAIN plan to return in PULL SUCCESS (no records emitted).
    pub explain_plan: Option<BoltValue>,
    /// PROFILE plan to return in PULL SUCCESS (records still emitted).
    pub profile_plan: Option<BoltValue>,
    /// The effective graph (database) for this query, used in the `db`
    /// metadata field of PULL SUCCESS.
    pub graph: String,
    /// Notifications (e.g. cartesian product warnings) to include in PULL
    /// SUCCESS metadata.
    pub notifications: Vec<BoltValue>,
    /// A per-row error that terminated streaming evaluation. When set,
    /// handle_pull will emit all buffered records and then send FAILURE
    /// with this error rather than SUCCESS.
    pub trailing_error: Option<(String, String)>,
    /// Extended stats for admin/DDL operations (indexes, constraints, system updates).
    /// These are merged into the stats map when emitting PULL SUCCESS.
    pub admin_stats: Option<AdminStats>,
}

/// Extended statistics for admin/DDL operations that go beyond standard
/// MutationStats.
#[derive(Debug, Clone, Default)]
pub struct AdminStats {
    pub indexes_added: u64,
    pub indexes_removed: u64,
    pub constraints_added: u64,
    pub constraints_removed: u64,
    pub system_updates: u64,
}

// -- Bookmark helpers --------------------------------------------------------

/// Format a bookmark value using the Neo4j convention.
///
/// The format `neo4j:bookmark:v1:tx{N}` is recognized by all Neo4j drivers
/// and the testkit. Using a shared atomic counter ensures that bookmarks
/// produced by session A can be meaningfully consumed by session B.
fn format_bookmark(n: u64) -> String {
    format!("neo4j:bookmark:v1:tx{}", n)
}

/// Increment the global bookmark counter and return a formatted bookmark string.
fn next_bookmark(counter: &AtomicU64) -> String {
    let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
    format_bookmark(n)
}

// -- Constant-time comparison ------------------------------------------------

/// Constant-time byte comparison to prevent timing side-channel attacks on
/// credential validation.  Always compares all bytes regardless of mismatch
/// position, preventing character-by-character brute-force via response timing.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        // Length mismatch leaks the length but not the content -- acceptable
        // for password comparison where the expected password is fixed-length.
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

// -- Bookmark validation -----------------------------------------------------

/// Check whether a bookmark string has a recognized format.
/// Valid formats: "neo4j:bookmark:v{V}:tx{N}" or "sedan:tx:{N}" where V and N are digits.
fn is_valid_bookmark(bookmark: &str) -> bool {
    if bookmark.is_empty() {
        return true;
    }
    // neo4j:bookmark:vN:txM format -- strip prefix, expect "vN:txM" with digits
    if let Some(rest) = bookmark.strip_prefix("neo4j:bookmark:") {
        // rest should be "vN:txM" where N and M are digit sequences
        if let Some(after_v) = rest.strip_prefix('v') {
            if let Some(colon_pos) = after_v.find(':') {
                let version_part = &after_v[..colon_pos];
                let tx_part = &after_v[colon_pos + 1..];
                if let Some(tx_num) = tx_part.strip_prefix("tx") {
                    return !version_part.is_empty()
                        && version_part.chars().all(|c| c.is_ascii_digit())
                        && !tx_num.is_empty()
                        && tx_num.chars().all(|c| c.is_ascii_digit());
                }
            }
        }
        return false;
    }
    // sedan:tx:N format
    if let Some(suffix) = bookmark.strip_prefix("sedan:tx:") {
        return !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit());
    }
    false
}

/// Validate bookmarks from a message's extra map.
///
/// Returns `Some(ServerMessage::Failure(...))` if any bookmark is invalid,
/// or `None` if all bookmarks are valid (or no bookmarks are present).
fn validate_bookmarks(extra: &BoltMap) -> Option<ServerMessage> {
    let bookmarks_val = extra.get_value("bookmarks")?;

    // Collect bookmark strings: either from a List or a single String.
    let bookmark_strings: Vec<&str> = match bookmarks_val {
        BoltValue::List(items) => items
            .iter()
            .filter_map(|v| {
                if let BoltValue::String(s) = v {
                    Some(s.as_str())
                } else {
                    None
                }
            })
            .collect(),
        BoltValue::String(s) => vec![s.as_str()],
        _ => return None, // Non-list/non-string bookmarks field is ignored.
    };

    for bm in bookmark_strings {
        if !is_valid_bookmark(bm) {
            return Some(ServerMessage::Failure(bolt_map! {
                "code" => BoltValue::String(
                    "Neo.ClientError.Transaction.InvalidBookmark".to_string(),
                ),
                "message" => BoltValue::String(
                    format!("Supplied bookmark '{}' does not conform to pattern {{database_id}}:{{tx_id}}", bm),
                )
            }));
        }
    }
    None
}

// -- TX metadata extraction --------------------------------------------------

/// Extract transaction metadata from a message's extra map.
///
/// Drivers send metadata under either `"tx_metadata"` (Neo4j driver convention)
/// or `"metadata"` (Bolt spec). Returns the inner map, or an empty map if absent.
fn extract_tx_metadata(extra: &BoltMap) -> BoltMap {
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
        return Err(ProtocolError::InvalidState {
            state: session.state,
        });
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
    use gql_core::storage::InMemoryGraphStore;

    // -- Bookmark validation unit tests --------------------------------------

    #[test]
    fn valid_bookmark_sedan_prefix() {
        assert!(is_valid_bookmark("sedan:tx:1"));
        assert!(is_valid_bookmark("sedan:tx:42"));
        assert!(is_valid_bookmark("sedan:tx:999999"));
    }

    #[test]
    fn valid_bookmark_neo4j_prefix() {
        assert!(is_valid_bookmark("neo4j:bookmark:v1:tx100"));
        assert!(is_valid_bookmark("neo4j:bookmark:v2:tx1"));
    }

    #[test]
    fn valid_bookmark_empty() {
        assert!(is_valid_bookmark(""));
    }

    #[test]
    fn invalid_bookmark_random_string() {
        assert!(!is_valid_bookmark("invalid_bookmark"));
        assert!(!is_valid_bookmark("random"));
        assert!(!is_valid_bookmark("tx:1"));
        assert!(!is_valid_bookmark("bookmark:1"));
    }

    #[test]
    fn validate_bookmarks_none_present() {
        let extra: BoltMap = vec![];
        assert!(validate_bookmarks(&extra).is_none());
    }

    #[test]
    fn validate_bookmarks_empty_list() {
        let extra: BoltMap = vec![("bookmarks".to_string(), BoltValue::List(vec![]))];
        assert!(validate_bookmarks(&extra).is_none());
    }

    #[test]
    fn validate_bookmarks_valid_sedan() {
        let extra: BoltMap = vec![(
            "bookmarks".to_string(),
            BoltValue::List(vec![BoltValue::String("sedan:tx:1".to_string())]),
        )];
        assert!(validate_bookmarks(&extra).is_none());
    }

    #[test]
    fn validate_bookmarks_valid_neo4j() {
        let extra: BoltMap = vec![(
            "bookmarks".to_string(),
            BoltValue::List(vec![BoltValue::String(
                "neo4j:bookmark:v1:tx100".to_string(),
            )]),
        )];
        assert!(validate_bookmarks(&extra).is_none());
    }

    #[test]
    fn validate_bookmarks_invalid_returns_failure() {
        let extra: BoltMap = vec![(
            "bookmarks".to_string(),
            BoltValue::List(vec![BoltValue::String("invalid_bookmark".to_string())]),
        )];
        let failure = validate_bookmarks(&extra);
        assert!(failure.is_some());
        match failure.unwrap() {
            ServerMessage::Failure(meta) => {
                let code = meta.get_str("code").unwrap();
                assert_eq!(code, "Neo.ClientError.Transaction.InvalidBookmark");
                let msg = meta.get_str("message").unwrap();
                assert!(msg.contains("invalid_bookmark"));
            }
            other => panic!("expected Failure, got {:?}", other),
        }
    }

    #[test]
    fn validate_bookmarks_mixed_valid_and_invalid() {
        let extra: BoltMap = vec![(
            "bookmarks".to_string(),
            BoltValue::List(vec![
                BoltValue::String("sedan:tx:1".to_string()),
                BoltValue::String("bad_bookmark".to_string()),
            ]),
        )];
        let failure = validate_bookmarks(&extra);
        assert!(failure.is_some());
        match failure.unwrap() {
            ServerMessage::Failure(meta) => {
                let code = meta.get_str("code").unwrap();
                assert_eq!(code, "Neo.ClientError.Transaction.InvalidBookmark");
            }
            other => panic!("expected Failure, got {:?}", other),
        }
    }

    #[test]
    fn validate_bookmarks_single_string_valid() {
        let extra: BoltMap = vec![(
            "bookmarks".to_string(),
            BoltValue::String("neo4j:bookmark:v1:tx1".to_string()),
        )];
        assert!(validate_bookmarks(&extra).is_none());
    }

    #[test]
    fn validate_bookmarks_single_string_invalid() {
        let extra: BoltMap = vec![(
            "bookmarks".to_string(),
            BoltValue::String("not_a_valid_bookmark".to_string()),
        )];
        let failure = validate_bookmarks(&extra);
        assert!(failure.is_some());
        match failure.unwrap() {
            ServerMessage::Failure(meta) => {
                let code = meta.get_str("code").unwrap();
                assert_eq!(code, "Neo.ClientError.Transaction.InvalidBookmark");
            }
            other => panic!("expected Failure, got {:?}", other),
        }
    }

    #[test]
    fn validate_bookmarks_non_list_non_string_ignored() {
        let extra: BoltMap = vec![("bookmarks".to_string(), BoltValue::Int(42))];
        assert!(validate_bookmarks(&extra).is_none());
    }

    // -- Handler integration tests -------------------------------------------

    fn make_ready_session() -> Session {
        Session {
            state: ServerState::Ready,
            store: Arc::new(InMemoryGraphStore::new()),
            conn_id: 1,
            current_graph: "neo4j".to_string(),
            pending_results: HashMap::new(),
            next_qid: 0,
            global_bookmark: Arc::new(AtomicU64::new(0)),
            tx_metadata: Vec::new(),
            tx_timeout: None,
            tx_store: None,
            write_semaphore: Arc::new(tokio::sync::Semaphore::new(128)),
            write_permit: None,
        }
    }

    /// Build two sessions that share the same store and global bookmark counter,
    /// simulating two connections to the same server.
    fn make_shared_sessions() -> (Session, Session) {
        let store: Arc<dyn GraphStore> = Arc::new(InMemoryGraphStore::new());
        let bookmark = Arc::new(AtomicU64::new(0));
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
        };
        (s1, s2)
    }

    // -- Bookmark format tests -----------------------------------------------

    #[test]
    fn bookmark_format_matches_neo4j_convention() {
        assert_eq!(format_bookmark(1), "neo4j:bookmark:v1:tx1");
        assert_eq!(format_bookmark(42), "neo4j:bookmark:v1:tx42");
    }

    #[test]
    fn next_bookmark_increments_counter() {
        let counter = AtomicU64::new(0);
        assert_eq!(next_bookmark(&counter), "neo4j:bookmark:v1:tx1");
        assert_eq!(next_bookmark(&counter), "neo4j:bookmark:v1:tx2");
        assert_eq!(next_bookmark(&counter), "neo4j:bookmark:v1:tx3");
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
                columns: vec!["n".to_string()],
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
}
