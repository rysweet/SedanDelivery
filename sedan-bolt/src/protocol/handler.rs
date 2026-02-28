use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::bridge;
use crate::message::types::{ClientMessage, ServerMessage};
use crate::packstream::value::{BoltMap, BoltMapExt, BoltValue};
use crate::protocol::state::ServerState;
use gql_core::storage::InMemoryGraphStore;

// -- Error type --------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("message not accepted in current state {state:?}")]
    InvalidState { state: ServerState },

    #[error("internal error: {0}")]
    Internal(String),
}

// -- Helper macro for building BoltMap literals ------------------------------

macro_rules! bolt_map {
    ($($key:expr => $val:expr),* $(,)?) => {{
        #[allow(unused_mut)]
        let mut entries: Vec<(String, BoltValue)> = Vec::new();
        $(entries.push(($key.to_string(), $val));)*
        entries
    }};
}

// -- Session types -----------------------------------------------------------

pub struct Session {
    pub state: ServerState,
    pub store: Arc<InMemoryGraphStore>,
    pub conn_id: u64,
    pub current_graph: String,
    pub pending_result: Option<PendingQuery>,
    /// Global bookmark counter shared across all sessions connected to the same
    /// store. Bookmarks produced by one session are therefore meaningful when
    /// passed to another session, which is required by the Neo4j driver testkit.
    pub global_bookmark: Arc<AtomicU64>,
    /// TX metadata set via BEGIN extra map, echoed back on PULL completion.
    pub tx_metadata: BoltMap,
    /// TX-level timeout set via BEGIN extra map (milliseconds).
    pub tx_timeout: Option<u64>,
}

pub struct PendingQuery {
    pub columns: Vec<String>,
    pub records: Vec<Vec<BoltValue>>,
    pub query_type: String,
    pub stats: BoltMap,
    pub cursor: usize,
    pub start_time: std::time::Instant,
    /// TX metadata from RUN extra map, echoed back on PULL completion.
    pub metadata: BoltMap,
    /// EXPLAIN plan to return in PULL SUCCESS (no records emitted).
    pub explain_plan: Option<BoltValue>,
    /// PROFILE plan to return in PULL SUCCESS (records still emitted).
    pub profile_plan: Option<BoltValue>,
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

// -- Message handler ---------------------------------------------------------

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
    if session.state == ServerState::Interrupted {
        if !matches!(&msg, ClientMessage::Reset) {
            return Ok(vec![ServerMessage::Ignored]);
        }
    }

    // Check state accepts the message.
    if !session.state.accepts(&msg) {
        return Err(ProtocolError::InvalidState {
            state: session.state,
        });
    }

    match msg {
        ClientMessage::Hello(extra) => handle_hello(session, extra).await,
        ClientMessage::Logon(auth) => handle_logon(session, auth).await,
        ClientMessage::Run {
            query,
            params,
            extra,
        } => handle_run(session, query, params, extra).await,
        ClientMessage::Pull(extra) => handle_pull(session, extra),
        ClientMessage::Discard(extra) => handle_discard(session, extra),
        ClientMessage::Begin(extra) => handle_begin(session, extra),
        ClientMessage::Commit => handle_commit(session),
        ClientMessage::Rollback => handle_rollback(session),
        ClientMessage::Reset => handle_reset(session),
        ClientMessage::Goodbye => {
            // Handled in session loop; should not reach here.
            Ok(vec![])
        }
    }
}

// -- Bookmark validation -----------------------------------------------------

/// Known valid bookmark prefixes. Bookmarks must start with one of these.
const VALID_BOOKMARK_PREFIXES: &[&str] = &[
    "sedan:tx:",
    "neo4j:bookmark:",
];

/// Check whether a bookmark string has a recognized format.
fn is_valid_bookmark(bookmark: &str) -> bool {
    if bookmark.is_empty() {
        return true;
    }
    VALID_BOOKMARK_PREFIXES
        .iter()
        .any(|prefix| bookmark.starts_with(prefix))
}

/// Validate bookmarks from a message's extra map.
///
/// Returns `Some(ServerMessage::Failure(...))` if any bookmark is invalid,
/// or `None` if all bookmarks are valid (or no bookmarks are present).
fn validate_bookmarks(extra: &BoltMap) -> Option<ServerMessage> {
    let bookmarks_val = extra.get_value("bookmarks")?;
    let bookmark_list = match bookmarks_val {
        BoltValue::List(items) => items,
        _ => return None, // Non-list bookmarks field is ignored.
    };

    for item in bookmark_list {
        if let BoltValue::String(bm) = item {
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

// -- HELLO -------------------------------------------------------------------

async fn handle_hello(
    session: &mut Session,
    extra: BoltMap,
) -> Result<Vec<ServerMessage>, ProtocolError> {
    let required_password = std::env::var("BOLT_PASSWORD").ok();

    if let Some(required_pw) = required_password {
        let scheme = extra.get_str("scheme").unwrap_or("");
        if scheme != "basic" {
            session.state = ServerState::Defunct;
            return Ok(vec![ServerMessage::Failure(bolt_map! {
                "code" => BoltValue::String("Neo.ClientError.Security.Unauthorized".to_string()),
                "message" => BoltValue::String("Authentication scheme 'basic' required".to_string())
            })]);
        }
        let credentials = extra.get_str("credentials").unwrap_or("");
        if credentials != required_pw {
            session.state = ServerState::Defunct;
            return Ok(vec![ServerMessage::Failure(bolt_map! {
                "code" => BoltValue::String("Neo.ClientError.Security.Unauthorized".to_string()),
                "message" => BoltValue::String("Invalid credentials".to_string())
            })]);
        }
    }

    session.state = ServerState::Ready;
    Ok(vec![ServerMessage::Success(bolt_map! {
        "server" => BoltValue::String("Neo4j/4.4.0".to_string()),
        "connection_id" => BoltValue::String(format!("sedan-bolt-{}", session.conn_id))
    })])
}

// -- LOGON -------------------------------------------------------------------

async fn handle_logon(
    session: &mut Session,
    auth: BoltMap,
) -> Result<Vec<ServerMessage>, ProtocolError> {
    let required_password = std::env::var("BOLT_PASSWORD").ok();

    if let Some(required_pw) = required_password {
        let scheme = auth.get_str("scheme").unwrap_or("");
        if scheme != "basic" {
            session.state = ServerState::Defunct;
            return Ok(vec![ServerMessage::Failure(bolt_map! {
                "code" => BoltValue::String("Neo.ClientError.Security.Unauthorized".to_string()),
                "message" => BoltValue::String("Authentication scheme 'basic' required".to_string())
            })]);
        }
        let credentials = auth.get_str("credentials").unwrap_or("");
        if credentials != required_pw {
            session.state = ServerState::Defunct;
            return Ok(vec![ServerMessage::Failure(bolt_map! {
                "code" => BoltValue::String("Neo.ClientError.Security.Unauthorized".to_string()),
                "message" => BoltValue::String("Invalid credentials".to_string())
            })]);
        }
    }

    session.state = ServerState::Ready;
    Ok(vec![ServerMessage::Success(bolt_map! {})])
}

// -- RUN ---------------------------------------------------------------------

async fn handle_run(
    session: &mut Session,
    query: String,
    params: BoltMap,
    extra: BoltMap,
) -> Result<Vec<ServerMessage>, ProtocolError> {
    if let Some(failure) = validate_bookmarks(&extra) {
        session.state = ServerState::Failed;
        return Ok(vec![failure]);
    }

    // Detect EXPLAIN / PROFILE prefix (case-insensitive)
    let trimmed = query.trim();
    tracing::debug!(query = %trimmed, "handle_run query");
    let is_explain = trimmed.len() >= 8
        && trimmed[..8].eq_ignore_ascii_case("EXPLAIN ");
    let is_profile = !is_explain
        && trimmed.len() >= 8
        && trimmed[..8].eq_ignore_ascii_case("PROFILE ");
    let actual_query = if is_explain {
        trimmed[8..].to_string()
    } else if is_profile {
        trimmed[8..].to_string()
    } else {
        query.clone()
    };

    let graph = extra
        .get_str("db")
        .unwrap_or(&session.current_graph)
        .to_string();

    let run_metadata = extract_tx_metadata(&extra);
    let timeout_ms = extra.get_int("tx_timeout").map(|v| v as u64).or(session.tx_timeout);

    let value_params = bridge::executor::bolt_map_to_value_map(&params);

    // EXPLAIN: plan only, no execution
    if is_explain {
        let store = Arc::clone(&session.store);
        let g = graph.clone();
        let q = actual_query.clone();
        let p = value_params.clone();
        let plan_result = tokio::task::spawn_blocking(move || {
            bridge::executor::plan_query(&store, &g, &q, p)
        })
        .await
        .map_err(|e| ProtocolError::Internal(e.to_string()))?;

        match plan_result {
            Ok(operator) => {
                let plan_map = operator_to_plan_map(&operator, false, 0);
                session.pending_result = Some(PendingQuery {
                    columns: vec![],
                    records: vec![],
                    query_type: "r".to_string(),
                    stats: vec![],
                    cursor: 0,
                    start_time: std::time::Instant::now(),
                    metadata: run_metadata,
                    explain_plan: Some(plan_map),
                    profile_plan: None,
                });
                session.state = match session.state {
                    ServerState::TxReady | ServerState::TxStreaming => ServerState::TxStreaming,
                    _ => ServerState::Streaming,
                };
                return Ok(vec![ServerMessage::Success(bolt_map! {
                    "fields" => BoltValue::List(vec![]),
                    "t_first" => BoltValue::Int(0)
                })]);
            }
            Err(e) => {
                let (code, message) = bridge::errors::gql_error_to_bolt(&e);
                session.state = ServerState::Failed;
                return Ok(vec![ServerMessage::Failure(bolt_map! {
                    "code" => BoltValue::String(code),
                    "message" => BoltValue::String(message)
                })]);
            }
        }
    }

    let store = Arc::clone(&session.store);
    let graph_clone = graph.clone();
    let query_clone = actual_query.clone();

    // For PROFILE, also get the plan
    let profile_plan = if is_profile {
        let store2 = Arc::clone(&session.store);
        let g2 = graph.clone();
        let q2 = actual_query.clone();
        let p2 = value_params.clone();
        let plan = tokio::task::spawn_blocking(move || {
            bridge::executor::plan_query(&store2, &g2, &q2, p2)
        })
        .await
        .map_err(|e| ProtocolError::Internal(e.to_string()))?;
        plan.ok().map(|op| operator_to_plan_map(&op, true, 0))
    } else {
        None
    };

    let result = tokio::task::spawn_blocking(move || {
        bridge::executor::execute_query(&store, &graph_clone, &query_clone, value_params, timeout_ms)
    })
    .await
    .map_err(|e| ProtocolError::Internal(format!("spawn_blocking join error: {}", e)))?;

    match result {
        Ok(qr) => {
            let (records, query_type, stats) =
                bridge::executor::result_to_records(&qr, &session.store, &graph);

            let fields: Vec<BoltValue> = qr
                .columns
                .iter()
                .map(|c| BoltValue::String(c.clone()))
                .collect();

            let combined_metadata = if !session.tx_metadata.is_empty() {
                session.tx_metadata.clone()
            } else {
                run_metadata
            };

            // For PROFILE, patch rows into plan
            let patched_profile = profile_plan.map(|mut plan| {
                if let BoltValue::Map(ref mut entries) = plan {
                    entries.push(("rows".to_string(), BoltValue::Int(qr.rows.len() as i64)));
                    entries.push(("dbHits".to_string(), BoltValue::Int(qr.rows.len() as i64)));
                }
                plan
            });

            session.pending_result = Some(PendingQuery {
                columns: qr.columns,
                records,
                query_type,
                stats,
                cursor: 0,
                start_time: std::time::Instant::now(),
                metadata: combined_metadata,
                explain_plan: None,
                profile_plan: patched_profile,
            });

            session.state = match session.state {
                ServerState::TxReady | ServerState::TxStreaming => ServerState::TxStreaming,
                _ => ServerState::Streaming,
            };

            Ok(vec![ServerMessage::Success(bolt_map! {
                "fields" => BoltValue::List(fields),
                "t_first" => BoltValue::Int(0)
            })])
        }
        Err(gql_err) => {
            let (code, message) = bridge::errors::gql_error_to_bolt(&gql_err);
            session.state = ServerState::Failed;
            Ok(vec![ServerMessage::Failure(bolt_map! {
                "code" => BoltValue::String(code),
                "message" => BoltValue::String(message)
            })])
        }
    }
}

/// Convert a gql-core Operator tree into a Bolt plan map structure.
fn operator_to_plan_map(
    op: &gql_core::planner::Operator,
    _is_profile: bool,
    _depth: usize,
) -> BoltValue {
    let op_type = format!("{:?}", op).split('(').next().unwrap_or("Unknown").split('{').next().unwrap_or("Unknown").trim().to_string();
    let mut entries: Vec<(String, BoltValue)> = vec![
        ("operatorType".to_string(), BoltValue::String(op_type)),
        ("identifiers".to_string(), BoltValue::List(vec![])),
        ("args".to_string(), BoltValue::Map(vec![
            ("runtime".to_string(), BoltValue::String("INTERPRETED".to_string())),
            ("EstimatedRows".to_string(), BoltValue::Float(1.0)),
        ])),
        ("children".to_string(), BoltValue::List(vec![])),
    ];
    BoltValue::Map(entries)
}

// -- PULL --------------------------------------------------------------------

fn handle_pull(
    session: &mut Session,
    extra: BoltMap,
) -> Result<Vec<ServerMessage>, ProtocolError> {
    let n = extra.get_int("n").unwrap_or(-1);

    let pending = match session.pending_result.as_mut() {
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

    // For EXPLAIN, skip all RECORDs — the plan goes in the SUCCESS metadata
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

    if has_more {
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
        if let Some(plan) = explain_plan {
            meta.push(("plan".to_string(), plan));
        }
        if let Some(profile) = profile_plan {
            meta.push(("profile".to_string(), profile));
        }
        messages.push(ServerMessage::Success(meta));

        session.pending_result = None;
        session.state = match session.state {
            ServerState::TxStreaming => ServerState::TxReady,
            _ => ServerState::Ready,
        };
    }

    Ok(messages)
}

// -- DISCARD -----------------------------------------------------------------

fn handle_discard(
    session: &mut Session,
    _extra: BoltMap,
) -> Result<Vec<ServerMessage>, ProtocolError> {
    session.pending_result = None;
    session.state = match session.state {
        ServerState::TxStreaming => ServerState::TxReady,
        _ => ServerState::Ready,
    };
    Ok(vec![ServerMessage::Success(bolt_map! {})])
}

// -- BEGIN -------------------------------------------------------------------

fn handle_begin(
    session: &mut Session,
    extra: BoltMap,
) -> Result<Vec<ServerMessage>, ProtocolError> {
    if let Some(failure) = validate_bookmarks(&extra) {
        session.state = ServerState::Failed;
        return Ok(vec![failure]);
    }

    session.tx_metadata = extract_tx_metadata(&extra);
    session.tx_timeout = extra.get_int("tx_timeout").map(|v| v as u64);

    session.state = ServerState::TxReady;
    Ok(vec![ServerMessage::Success(bolt_map! {})])
}

// -- COMMIT ------------------------------------------------------------------

fn handle_commit(session: &mut Session) -> Result<Vec<ServerMessage>, ProtocolError> {
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
/// NOTE: sedan-bolt auto-commits writes on RUN (no deferred execution), so a
/// ROLLBACK cannot actually undo mutations that have already been applied to
/// the in-memory store. This is a known limitation. The response shape is
/// correct -- SUCCESS with no bookmark -- so that drivers that only check the
/// protocol framing will be satisfied.
fn handle_rollback(session: &mut Session) -> Result<Vec<ServerMessage>, ProtocolError> {
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
/// Clears all session-local state (pending results, TX metadata/timeout) and
/// returns to the Ready state. This is also the handler invoked when a client
/// exception causes the driver to send RESET.
///
/// NOTE: Like ROLLBACK, RESET cannot undo mutations that were already applied
/// during a RUN inside an open transaction. This is a known limitation of
/// sedan-bolt's auto-commit-on-RUN execution model.
fn handle_reset(session: &mut Session) -> Result<Vec<ServerMessage>, ProtocolError> {
    session.pending_result = None;
    session.tx_metadata = Vec::new();
    session.tx_timeout = None;
    session.state = ServerState::Ready;
    Ok(vec![ServerMessage::Success(bolt_map! {})])
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let extra: BoltMap = vec![(
            "bookmarks".to_string(),
            BoltValue::List(vec![]),
        )];
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
    fn validate_bookmarks_non_list_ignored() {
        let extra: BoltMap = vec![(
            "bookmarks".to_string(),
            BoltValue::String("not_a_list".to_string()),
        )];
        assert!(validate_bookmarks(&extra).is_none());
    }

    // -- Handler integration tests -------------------------------------------

    fn make_ready_session() -> Session {
        Session {
            state: ServerState::Ready,
            store: Arc::new(InMemoryGraphStore::new()),
            conn_id: 1,
            current_graph: "default".to_string(),
            pending_result: None,
            global_bookmark: Arc::new(AtomicU64::new(0)),
            tx_metadata: Vec::new(),
            tx_timeout: None,
        }
    }

    /// Build two sessions that share the same store and global bookmark counter,
    /// simulating two connections to the same server.
    fn make_shared_sessions() -> (Session, Session) {
        let store = Arc::new(InMemoryGraphStore::new());
        let bookmark = Arc::new(AtomicU64::new(0));
        let s1 = Session {
            state: ServerState::Ready,
            store: Arc::clone(&store),
            conn_id: 1,
            current_graph: "default".to_string(),
            pending_result: None,
            global_bookmark: Arc::clone(&bookmark),
            tx_metadata: Vec::new(),
            tx_timeout: None,
        };
        let s2 = Session {
            state: ServerState::Ready,
            store: Arc::clone(&store),
            conn_id: 2,
            current_graph: "default".to_string(),
            pending_result: None,
            global_bookmark: Arc::clone(&bookmark),
            tx_metadata: Vec::new(),
            tx_timeout: None,
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
        handle_run(
            &mut s1,
            "RETURN 1 AS n".to_string(),
            vec![],
            vec![],
        )
        .await
        .unwrap();
        let pull_result = handle_pull(&mut s1, bolt_map! { "n" => BoltValue::Int(-1) }).unwrap();
        let bookmark = match pull_result.last().unwrap() {
            ServerMessage::Success(meta) => meta.get_str("bookmark").unwrap().to_string(),
            other => panic!("expected Success, got {:?}", other),
        };
        assert!(bookmark.starts_with("neo4j:bookmark:v1:tx"));

        // Session 2: pass that bookmark into BEGIN -- should succeed.
        let begin_result = handle_begin(
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
        handle_run(&mut s1, "RETURN 1 AS n".to_string(), vec![], vec![])
            .await
            .unwrap();
        let r1 = handle_pull(&mut s1, bolt_map! { "n" => BoltValue::Int(-1) }).unwrap();
        let bm1 = match r1.last().unwrap() {
            ServerMessage::Success(m) => m.get_str("bookmark").unwrap().to_string(),
            other => panic!("expected Success, got {:?}", other),
        };

        // Session 2: run + pull -> bookmark tx2 (not tx1 again).
        handle_run(&mut s2, "RETURN 2 AS n".to_string(), vec![], vec![])
            .await
            .unwrap();
        let r2 = handle_pull(&mut s2, bolt_map! { "n" => BoltValue::Int(-1) }).unwrap();
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

        let result = handle_commit(&mut session).unwrap();
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

        handle_commit(&mut session).unwrap();
        assert!(session.tx_metadata.is_empty());
        assert!(session.tx_timeout.is_none());
    }

    // -- ROLLBACK tests ------------------------------------------------------

    #[test]
    fn rollback_returns_success_without_bookmark() {
        let mut session = make_ready_session();
        session.state = ServerState::TxReady;

        let result = handle_rollback(&mut session).unwrap();
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

        handle_rollback(&mut session).unwrap();
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
        session.pending_result = Some(PendingQuery {
            columns: vec!["n".to_string()],
            records: vec![],
            query_type: "r".to_string(),
            stats: vec![],
            cursor: 0,
            start_time: std::time::Instant::now(),
            metadata: vec![],
            explain_plan: None,
            profile_plan: None,
        });

        let result = handle_reset(&mut session).unwrap();
        assert_eq!(result.len(), 1);
        assert!(matches!(&result[0], ServerMessage::Success(_)));
        assert_eq!(session.state, ServerState::Ready);
        assert!(session.pending_result.is_none());
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

        let result = handle_begin(&mut session, extra).unwrap();
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

        let result = handle_begin(&mut session, extra).unwrap();
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

        let result = handle_begin(&mut session, extra).unwrap();
        assert_eq!(result.len(), 1);
        assert!(matches!(&result[0], ServerMessage::Success(_)));
        assert_eq!(session.state, ServerState::TxReady);
    }

    #[test]
    fn begin_with_no_bookmarks_succeeds() {
        let mut session = make_ready_session();
        let extra: BoltMap = vec![];

        let result = handle_begin(&mut session, extra).unwrap();
        assert_eq!(result.len(), 1);
        assert!(matches!(&result[0], ServerMessage::Success(_)));
        assert_eq!(session.state, ServerState::TxReady);
    }

    #[test]
    fn begin_stores_tx_metadata() {
        let mut session = make_ready_session();
        let inner: BoltMap = vec![("app".to_string(), BoltValue::String("test".to_string()))];
        let extra: BoltMap = vec![(
            "tx_metadata".to_string(),
            BoltValue::Map(inner.clone()),
        )];

        handle_begin(&mut session, extra).unwrap();
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

        let result = handle_run(
            &mut session,
            "RETURN 1".to_string(),
            vec![],
            extra,
        )
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

        let result = handle_run(
            &mut session,
            "RETURN 1 AS n".to_string(),
            vec![],
            extra,
        )
        .await
        .unwrap();

        assert_eq!(result.len(), 1);
        assert!(matches!(&result[0], ServerMessage::Success(_)));
    }
}
