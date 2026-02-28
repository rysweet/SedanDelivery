use std::sync::Arc;

use crate::bridge;
use crate::message::types::{ClientMessage, ServerMessage};
use crate::packstream::value::{BoltMap, BoltMapExt, BoltValue};
use crate::protocol::state::ServerState;
use gql_core::storage::InMemoryGraphStore;

// ── Error type ──────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("message not accepted in current state {state:?}")]
    InvalidState { state: ServerState },

    #[error("internal error: {0}")]
    Internal(String),
}

// ── Helper macro for building BoltMap literals ──────────────────────────────

macro_rules! bolt_map {
    ($($key:expr => $val:expr),* $(,)?) => {{
        #[allow(unused_mut)]
        let mut entries: Vec<(String, BoltValue)> = Vec::new();
        $(entries.push(($key.to_string(), $val));)*
        entries
    }};
}

// ── Session types ───────────────────────────────────────────────────────────

pub struct Session {
    pub state: ServerState,
    pub store: Arc<InMemoryGraphStore>,
    pub conn_id: u64,
    pub current_graph: String,
    pub pending_result: Option<PendingQuery>,
    pub bookmark_counter: u64,
}

pub struct PendingQuery {
    pub columns: Vec<String>,
    pub records: Vec<Vec<BoltValue>>,
    pub query_type: String,
    pub stats: BoltMap,
    pub cursor: usize,
    pub start_time: std::time::Instant,
}

// ── Message handler ─────────────────────────────────────────────────────────

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

// ── Bookmark validation ─────────────────────────────────────────────────────

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

// ── HELLO ───────────────────────────────────────────────────────────────────

async fn handle_hello(
    session: &mut Session,
    extra: BoltMap,
) -> Result<Vec<ServerMessage>, ProtocolError> {
    // Check if BOLT_PASSWORD is configured for authentication.
    let required_password = std::env::var("BOLT_PASSWORD").ok();

    if let Some(required_pw) = required_password {
        // Require scheme=basic with matching credentials.
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

    // Authentication passed (or not required).
    session.state = ServerState::Ready;
    Ok(vec![ServerMessage::Success(bolt_map! {
        "server" => BoltValue::String("SedanDelivery/0.1.0".to_string()),
        "connection_id" => BoltValue::String(format!("sedan-bolt-{}", session.conn_id))
    })])
}

// ── LOGON ───────────────────────────────────────────────────────────────────

async fn handle_logon(
    session: &mut Session,
    auth: BoltMap,
) -> Result<Vec<ServerMessage>, ProtocolError> {
    // LOGON is the Bolt 5.1+ auth message. We handle it like HELLO auth.
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

// ── RUN ─────────────────────────────────────────────────────────────────────

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

    let graph = extra
        .get_str("db")
        .unwrap_or(&session.current_graph)
        .to_string();

    let value_params = bridge::executor::bolt_map_to_value_map(&params);
    let store = Arc::clone(&session.store);
    let graph_clone = graph.clone();
    let query_clone = query.clone();

    let result = tokio::task::spawn_blocking(move || {
        bridge::executor::execute_query(&store, &graph_clone, &query_clone, value_params)
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

            session.pending_result = Some(PendingQuery {
                columns: qr.columns,
                records,
                query_type,
                stats,
                cursor: 0,
                start_time: std::time::Instant::now(),
            });

            // Transition state: Ready -> Streaming, TxReady/TxStreaming -> TxStreaming.
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

// ── PULL ────────────────────────────────────────────────────────────────────

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
    let total = pending.records.len();
    let end = if n < 0 {
        total
    } else {
        (pending.cursor + n as usize).min(total)
    };

    for i in pending.cursor..end {
        messages.push(ServerMessage::Record(pending.records[i].clone()));
    }
    pending.cursor = end;

    let has_more = pending.cursor < total;

    if has_more {
        messages.push(ServerMessage::Success(bolt_map! {
            "has_more" => BoltValue::Bool(true)
        }));
    } else {
        session.bookmark_counter += 1;
        let elapsed_ms = pending.start_time.elapsed().as_millis() as i64;
        let query_type = pending.query_type.clone();
        let stats = pending.stats.clone();
        let bookmark = format!("sedan:tx:{}", session.bookmark_counter);

        let mut meta = bolt_map! {
            "bookmark" => BoltValue::String(bookmark),
            "t_last" => BoltValue::Int(elapsed_ms),
            "type" => BoltValue::String(query_type),
            "has_more" => BoltValue::Bool(false)
        };
        if !stats.is_empty() {
            meta.push(("stats".to_string(), BoltValue::Map(stats)));
        }
        messages.push(ServerMessage::Success(meta));

        // Clear pending result and transition state.
        session.pending_result = None;
        session.state = match session.state {
            ServerState::TxStreaming => ServerState::TxReady,
            _ => ServerState::Ready,
        };
    }

    Ok(messages)
}

// ── DISCARD ─────────────────────────────────────────────────────────────────

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

// ── BEGIN ───────────────────────────────────────────────────────────────────

fn handle_begin(
    session: &mut Session,
    extra: BoltMap,
) -> Result<Vec<ServerMessage>, ProtocolError> {
    if let Some(failure) = validate_bookmarks(&extra) {
        session.state = ServerState::Failed;
        return Ok(vec![failure]);
    }
    session.state = ServerState::TxReady;
    Ok(vec![ServerMessage::Success(bolt_map! {})])
}

// ── COMMIT ──────────────────────────────────────────────────────────────────

fn handle_commit(session: &mut Session) -> Result<Vec<ServerMessage>, ProtocolError> {
    session.bookmark_counter += 1;
    session.state = ServerState::Ready;
    let bookmark = format!("sedan:tx:{}", session.bookmark_counter);
    Ok(vec![ServerMessage::Success(bolt_map! {
        "bookmark" => BoltValue::String(bookmark)
    })])
}

// ── ROLLBACK ────────────────────────────────────────────────────────────────

fn handle_rollback(session: &mut Session) -> Result<Vec<ServerMessage>, ProtocolError> {
    session.state = ServerState::Ready;
    Ok(vec![ServerMessage::Success(bolt_map! {})])
}

// ── RESET ───────────────────────────────────────────────────────────────────

fn handle_reset(session: &mut Session) -> Result<Vec<ServerMessage>, ProtocolError> {
    session.pending_result = None;
    session.state = ServerState::Ready;
    Ok(vec![ServerMessage::Success(bolt_map! {})])
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Bookmark validation unit tests ──────────────────────────────────────

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
        // If bookmarks is not a list, it is silently ignored.
        let extra: BoltMap = vec![(
            "bookmarks".to_string(),
            BoltValue::String("not_a_list".to_string()),
        )];
        assert!(validate_bookmarks(&extra).is_none());
    }

    // ── Handler integration tests (bookmark rejection) ──────────────────────

    fn make_ready_session() -> Session {
        Session {
            state: ServerState::Ready,
            store: Arc::new(InMemoryGraphStore::new()),
            conn_id: 1,
            current_graph: "default".to_string(),
            pending_result: None,
            bookmark_counter: 0,
        }
    }

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
    fn begin_with_valid_bookmark_succeeds() {
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
    fn begin_with_no_bookmarks_succeeds() {
        let mut session = make_ready_session();
        let extra: BoltMap = vec![];

        let result = handle_begin(&mut session, extra).unwrap();
        assert_eq!(result.len(), 1);
        assert!(matches!(&result[0], ServerMessage::Success(_)));
        assert_eq!(session.state, ServerState::TxReady);
    }

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
