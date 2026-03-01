use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::bridge;
use crate::message::types::{ClientMessage, ServerMessage};
use crate::packstream::value::{BoltMap, BoltMapExt, BoltValue};
use crate::protocol::state::ServerState;
use gql_core::storage::InMemoryGraphStore;

/// Constant-time byte comparison to prevent timing side-channel attacks on
/// credential validation.  Always compares all bytes regardless of mismatch
/// position, preventing character-by-character brute-force via response timing.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        // Length mismatch leaks the length but not the content — acceptable
        // for password comparison where the expected password is fixed-length.
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

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
        let entries: Vec<(String, BoltValue)> = vec![$(($key.to_string(), $val)),*];
        entries
    }};
}

// -- Session types -----------------------------------------------------------

pub struct Session {
    pub state: ServerState,
    pub store: Arc<InMemoryGraphStore>,
    pub conn_id: u64,
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
    pub tx_store: Option<Arc<InMemoryGraphStore>>,
    /// Global write lock for transaction timeout contention.
    pub write_lock: Arc<tokio::sync::Mutex<()>>,
    /// Guard held while this session holds the write lock (during a write TX).
    pub write_lock_guard: Option<tokio::sync::OwnedMutexGuard<()>>,
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

/// Check whether a bookmark string has a recognized format.
/// Valid formats: "neo4j:bookmark:v{V}:tx{N}" or "sedan:tx:{N}" where V and N are digits.
fn is_valid_bookmark(bookmark: &str) -> bool {
    if bookmark.is_empty() {
        return true;
    }
    // neo4j:bookmark:vN:txM format — strip prefix, expect "vN:txM" with digits
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
        if !constant_time_eq(credentials.as_bytes(), required_pw.as_bytes()) {
            session.state = ServerState::Defunct;
            return Ok(vec![ServerMessage::Failure(bolt_map! {
                "code" => BoltValue::String("Neo.ClientError.Security.Unauthorized".to_string()),
                "message" => BoltValue::String("Invalid credentials".to_string())
            })]);
        }
    }

    // Check if the client requests the UTC patch (Bolt 4.3+).
    // When echoed back, the driver uses UTC-based DateTime encoding (tags 0x49/0x46)
    // instead of wall-clock encoding (tags 0x69/0x66).
    let utc_patch = match extra.get_value("patch_bolt") {
        Some(BoltValue::List(items)) => items
            .iter()
            .any(|v| matches!(v, BoltValue::String(s) if s == "utc")),
        _ => false,
    };

    session.state = ServerState::Ready;
    let mut success = bolt_map! {
        "server" => BoltValue::String("Neo4j/4.4.0".to_string()),
        "connection_id" => BoltValue::String(format!("sedandelivery-bolt-{}", session.conn_id))
    };
    if utc_patch {
        success.push((
            "patch_bolt".to_string(),
            BoltValue::List(vec![BoltValue::String("utc".to_string())]),
        ));
    }
    Ok(vec![ServerMessage::Success(success)])
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
        if !constant_time_eq(credentials.as_bytes(), required_pw.as_bytes()) {
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

    // Strip Neo4j pre-parser directives like "CYPHER runtime=interpreted ..."
    let trimmed = query.trim();
    let trimmed = if trimmed.len() > 7 && trimmed[..7].eq_ignore_ascii_case("CYPHER ") {
        let cypher_keywords = [
            "MATCH ",
            "RETURN ",
            "CREATE ",
            "MERGE ",
            "WITH ",
            "UNWIND ",
            "CALL ",
            "OPTIONAL ",
            "DELETE ",
            "DETACH ",
            "SET ",
            "REMOVE ",
            "EXPLAIN ",
            "PROFILE ",
        ];
        let upper = trimmed.to_uppercase();
        let mut start = trimmed.len(); // fallback: keep entire string
        for kw in &cypher_keywords {
            if let Some(pos) = upper[7..].find(kw) {
                let candidate = 7 + pos;
                if candidate < start {
                    start = candidate;
                }
            }
        }
        &trimmed[start..]
    } else {
        trimmed
    };

    // Detect EXPLAIN / PROFILE prefix (case-insensitive)
    tracing::debug!(query = %trimmed, "handle_run query");
    let is_explain = trimmed.len() >= 8 && trimmed[..8].eq_ignore_ascii_case("EXPLAIN ");
    let is_profile =
        !is_explain && trimmed.len() >= 8 && trimmed[..8].eq_ignore_ascii_case("PROFILE ");
    let actual_query = if is_explain || is_profile {
        trimmed[8..].to_string()
    } else {
        trimmed.to_string()
    };

    let graph = extra
        .get_str("db")
        .unwrap_or(&session.current_graph)
        .to_string();

    let run_metadata = extract_tx_metadata(&extra);
    let timeout_ms = extra
        .get_int("tx_timeout")
        .map(|v| v as u64)
        .or(session.tx_timeout);

    let mut value_params = bridge::executor::bolt_map_to_value_map(&params);

    // Inject tx_metadata as a special parameter so that the tx.getMetaData
    // procedure can access it. Prefer BEGIN-level metadata over RUN-level.
    {
        let effective_meta = if !session.tx_metadata.is_empty() {
            &session.tx_metadata
        } else {
            &run_metadata
        };
        let meta_value = bridge::executor::bolt_map_to_value_map(effective_meta);
        let meta_map: indexmap::IndexMap<Box<str>, gql_core::types::Value> = meta_value
            .into_iter()
            .map(|(k, v)| (Box::from(k.as_str()), v))
            .collect();
        value_params.insert(
            "__tx_metadata__".to_string(),
            gql_core::types::Value::Map(meta_map),
        );
    }

    // When inside an explicit transaction, execute against the snapshot store
    // so that mutations are isolated until COMMIT.
    let effective_store: Arc<InMemoryGraphStore> = match &session.tx_store {
        Some(ts)
            if matches!(
                session.state,
                ServerState::TxReady | ServerState::TxStreaming
            ) =>
        {
            Arc::clone(ts)
        }
        _ => Arc::clone(&session.store),
    };

    // EXPLAIN: plan only, no execution
    if is_explain {
        let store = Arc::clone(&effective_store);
        let g = graph.clone();
        let q = actual_query.clone();
        let p = value_params.clone();
        let plan_result =
            tokio::task::spawn_blocking(move || bridge::executor::plan_query(&store, &g, &q, p))
                .await
                .map_err(|e| ProtocolError::Internal(e.to_string()))?;

        match plan_result {
            Ok(operator) => {
                let plan_map = operator_to_plan_map(&operator, false, 0);
                let plan_notifications = detect_notifications(&operator);
                let qid = session.next_qid;
                session.next_qid += 1;
                session.pending_results.insert(
                    qid,
                    PendingQuery {
                        columns: vec![],
                        records: vec![],
                        query_type: "r".to_string(),
                        stats: vec![],
                        cursor: 0,
                        start_time: std::time::Instant::now(),
                        metadata: run_metadata,
                        explain_plan: Some(plan_map),
                        profile_plan: None,
                        graph: graph.clone(),
                        notifications: plan_notifications,
                        trailing_error: None,
                    },
                );
                session.state = match session.state {
                    ServerState::TxReady | ServerState::TxStreaming => ServerState::TxStreaming,
                    _ => ServerState::Streaming,
                };
                return Ok(vec![ServerMessage::Success(bolt_map! {
                    "fields" => BoltValue::List(vec![]),
                    "t_first" => BoltValue::Int(0),
                    "qid" => BoltValue::Int(qid)
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

    // Detect write queries for transaction timeout contention.
    // If the query contains write keywords and we're in a transaction with a
    // timeout, acquire the global write lock. If it's already held by another
    // session, wait up to tx_timeout then fail with a lock timeout error.
    let is_write_query = {
        let upper = actual_query.to_uppercase();
        upper.contains("CREATE ")
            || upper.contains("MERGE ")
            || upper.contains("DELETE ")
            || upper.contains("SET ")
            || upper.contains("REMOVE ")
    };
    if is_write_query && session.write_lock_guard.is_none() {
        let timeout_ms = session
            .tx_timeout
            .or(extra.get_int("tx_timeout").map(|v| v as u64));
        if let Some(ms) = timeout_ms {
            let lock = Arc::clone(&session.write_lock);
            match tokio::time::timeout(std::time::Duration::from_millis(ms), lock.lock_owned())
                .await
            {
                Ok(guard) => {
                    session.write_lock_guard = Some(guard);
                }
                Err(_) => {
                    // Lock acquisition timed out
                    session.state = ServerState::Failed;
                    return Ok(vec![ServerMessage::Failure(bolt_map! {
                        "code" => BoltValue::String(
                            "Neo.TransientError.Transaction.LockClientStopped".to_string()
                        ),
                        "message" => BoltValue::String(
                            format!("The transaction has been terminated. \
                                     Retry your operation in a new transaction, \
                                     and you should see a successful result. \
                                     Lock acquisition timeout after {} ms", ms)
                        )
                    })]);
                }
            }
        } else {
            // No timeout: just acquire the lock (may block indefinitely)
            let lock = Arc::clone(&session.write_lock);
            let guard = lock.lock_owned().await;
            session.write_lock_guard = Some(guard);
        }
    }

    let store = Arc::clone(&effective_store);
    let graph_clone = graph.clone();
    let query_clone = actual_query.clone();

    // For PROFILE, also get the plan
    let profile_plan = if is_profile {
        let store2 = Arc::clone(&effective_store);
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
        bridge::executor::execute_query(
            &store,
            &graph_clone,
            &query_clone,
            value_params,
            timeout_ms,
        )
    })
    .await
    .map_err(|e| ProtocolError::Internal(format!("spawn_blocking join error: {}", e)))?;

    match result {
        Ok(qr) => {
            let (records, query_type, stats, trailing_err) =
                bridge::executor::result_to_records(&qr, &effective_store, &graph);

            // Convert the trailing GqlError (if any) to a (code, message) pair
            // for deferred FAILURE emission during PULL.
            let trailing_error = trailing_err.map(|e| bridge::errors::gql_error_to_bolt(&e));

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

            let qid = session.next_qid;
            session.next_qid += 1;
            session.pending_results.insert(
                qid,
                PendingQuery {
                    columns: qr.columns,
                    records,
                    query_type,
                    stats,
                    cursor: 0,
                    start_time: std::time::Instant::now(),
                    metadata: combined_metadata,
                    explain_plan: None,
                    profile_plan: patched_profile,
                    graph: graph.clone(),
                    notifications: vec![],
                    trailing_error,
                },
            );

            session.state = match session.state {
                ServerState::TxReady | ServerState::TxStreaming => ServerState::TxStreaming,
                _ => {
                    // For autocommit queries, release the write lock immediately
                    // since the query has completed.
                    session.write_lock_guard = None;
                    ServerState::Streaming
                }
            };

            Ok(vec![ServerMessage::Success(bolt_map! {
                "fields" => BoltValue::List(fields),
                "t_first" => BoltValue::Int(0),
                "qid" => BoltValue::Int(qid)
            })])
        }
        Err(gql_err) => {
            let (code, message) = bridge::errors::gql_error_to_bolt(&gql_err);
            // Release write lock on error
            session.write_lock_guard = None;
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
    let op_type = format!("{:?}", op)
        .split('(')
        .next()
        .unwrap_or("Unknown")
        .split('{')
        .next()
        .unwrap_or("Unknown")
        .trim()
        .to_string();
    let entries: Vec<(String, BoltValue)> = vec![
        ("operatorType".to_string(), BoltValue::String(op_type)),
        ("identifiers".to_string(), BoltValue::List(vec![])),
        (
            "args".to_string(),
            BoltValue::Map(vec![
                (
                    "runtime".to_string(),
                    BoltValue::String("INTERPRETED".to_string()),
                ),
                ("EstimatedRows".to_string(), BoltValue::Float(1.0)),
            ]),
        ),
        ("children".to_string(), BoltValue::List(vec![])),
    ];
    BoltValue::Map(entries)
}

/// Detect cartesian product patterns in an operator tree.
///
/// A cartesian product occurs when an `Apply` node joins two independent scans
/// (no shared variable references). Returns a list of Bolt notification maps.
fn detect_notifications(op: &gql_core::planner::Operator) -> Vec<BoltValue> {
    let mut notifications = Vec::new();
    detect_cartesian_products(op, &mut notifications);
    notifications
}

fn detect_cartesian_products(op: &gql_core::planner::Operator, notifications: &mut Vec<BoltValue>) {
    use gql_core::planner::Operator;
    match op {
        Operator::Apply { outer, inner } => {
            // An Apply with two independent scans (both are leaf-level node scans
            // or have no shared aliases) is a cartesian product.
            let outer_aliases: std::collections::HashSet<String> =
                gql_core::planner::collect_visible_aliases(outer)
                    .into_iter()
                    .collect();
            let inner_aliases: std::collections::HashSet<String> =
                gql_core::planner::collect_visible_aliases(inner)
                    .into_iter()
                    .collect();
            if outer_aliases.is_disjoint(&inner_aliases) {
                // Determine which alias to mention in the warning.
                let inner_alias = inner_aliases
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| "?".to_string());
                notifications.push(BoltValue::Map(bolt_map! {
                    "code" => BoltValue::String(
                        "Neo.ClientNotification.Statement.CartesianProductWarning".to_string()
                    ),
                    "title" => BoltValue::String(
                        "This query builds a cartesian product between disconnected patterns.".to_string()
                    ),
                    "description" => BoltValue::String(
                        format!(
                            "If a part of a query contains multiple disconnected patterns, this will build a cartesian product between all those parts. This may produce a large amount of data and slow down query processing. While occasionally intended, it may often be possible to reformulate the query that avoids the use of this cross product, perhaps by adding a relationship between the different parts or by using OPTIONAL MATCH (identifier is: ({inner_alias}))"
                        )
                    ),
                    "severity" => BoltValue::String("WARNING".to_string()),
                    "category" => BoltValue::String("PERFORMANCE".to_string()),
                    "position" => BoltValue::Map(bolt_map! {
                        "offset" => BoltValue::Int(0),
                        "line" => BoltValue::Int(1),
                        "column" => BoltValue::Int(1)
                    })
                }));
            }
            detect_cartesian_products(outer, notifications);
            detect_cartesian_products(inner, notifications);
        }
        Operator::Filter { child, .. }
        | Operator::Sort { child, .. }
        | Operator::Limit { child, .. }
        | Operator::Skip { child, .. }
        | Operator::Distinct { child, .. }
        | Operator::Set { child, .. }
        | Operator::Remove { child, .. }
        | Operator::Delete { child, .. }
        | Operator::Unwind { child, .. }
        | Operator::Expand { child, .. }
        | Operator::VarLenExpand { child, .. }
        | Operator::AssemblePath { child, .. }
        | Operator::MergeRelationship { child, .. } => {
            detect_cartesian_products(child, notifications);
        }
        Operator::Project { child, .. } | Operator::Aggregate { child, .. } => {
            detect_cartesian_products(child, notifications);
        }
        Operator::LeftOuterApply {
            preserved,
            optional_pattern,
        } => {
            detect_cartesian_products(preserved, notifications);
            detect_cartesian_products(optional_pattern, notifications);
        }
        Operator::Union { left, right, .. } => {
            detect_cartesian_products(left, notifications);
            detect_cartesian_products(right, notifications);
        }
        Operator::Create { child, .. } | Operator::Merge { child, .. } => {
            if let Some(ch) = child {
                detect_cartesian_products(ch, notifications);
            }
        }
        Operator::Call { child, .. } => {
            if let Some(ch) = child {
                detect_cartesian_products(ch, notifications);
            }
        }
        Operator::ProduceResults { child, .. } => {
            detect_cartesian_products(child, notifications);
        }
        Operator::NodeScan { .. }
        | Operator::IndexScan { .. }
        | Operator::NodeByIdSeek { .. }
        | Operator::SingleRow
        | Operator::ShortestPath { .. } => {}
    }
}

// -- PULL --------------------------------------------------------------------

fn handle_pull(session: &mut Session, extra: BoltMap) -> Result<Vec<ServerMessage>, ProtocolError> {
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

fn handle_discard(
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

    // If the BEGIN specifies a database, update the session's current graph so
    // that subsequent RUN messages within the TX use the correct graph.
    if let Some(db) = extra.get_str("db") {
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

fn handle_commit(session: &mut Session) -> Result<Vec<ServerMessage>, ProtocolError> {
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
fn handle_rollback(session: &mut Session) -> Result<Vec<ServerMessage>, ProtocolError> {
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
fn handle_reset(session: &mut Session) -> Result<Vec<ServerMessage>, ProtocolError> {
    session.pending_results.clear();
    session.tx_metadata = Vec::new();
    session.tx_timeout = None;
    session.tx_store = None;
    session.write_lock_guard = None;
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
            write_lock: Arc::new(tokio::sync::Mutex::new(())),
            write_lock_guard: None,
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
            current_graph: "neo4j".to_string(),
            pending_results: HashMap::new(),
            next_qid: 0,
            global_bookmark: Arc::clone(&bookmark),
            tx_metadata: Vec::new(),
            tx_timeout: None,
            tx_store: None,
            write_lock: Arc::new(tokio::sync::Mutex::new(())),
            write_lock_guard: None,
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
            write_lock: Arc::new(tokio::sync::Mutex::new(())),
            write_lock_guard: None,
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
        handle_run(&mut s1, "RETURN 1 AS n".to_string(), vec![], vec![])
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
            },
        );

        let result = handle_reset(&mut session).unwrap();
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
        let extra: BoltMap = vec![("tx_metadata".to_string(), BoltValue::Map(inner.clone()))];

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

        let result = handle_run(&mut session, "RETURN 1".to_string(), vec![], extra)
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

        let result = handle_run(&mut session, "RETURN 1 AS n".to_string(), vec![], extra)
            .await
            .unwrap();

        assert_eq!(result.len(), 1);
        assert!(matches!(&result[0], ServerMessage::Success(_)));
    }

    // -- Snapshot transaction isolation tests --------------------------------

    /// Helper: run a query inside a session and pull all results. Returns the
    /// PULL SUCCESS metadata.
    async fn run_and_pull(session: &mut Session, query: &str) -> Vec<ServerMessage> {
        let run_result = handle_run(session, query.to_string(), vec![], vec![])
            .await
            .unwrap();
        assert!(
            matches!(&run_result[0], ServerMessage::Success(_)),
            "RUN failed: {:?}",
            run_result
        );
        handle_pull(session, bolt_map! { "n" => BoltValue::Int(-1) }).unwrap()
    }

    #[tokio::test]
    async fn begin_creates_tx_store() {
        let mut session = make_ready_session();
        assert!(session.tx_store.is_none());

        handle_begin(&mut session, vec![]).unwrap();
        assert!(session.tx_store.is_some());
        assert_eq!(session.state, ServerState::TxReady);
    }

    #[tokio::test]
    async fn commit_clears_tx_store() {
        let mut session = make_ready_session();
        handle_begin(&mut session, vec![]).unwrap();
        assert!(session.tx_store.is_some());

        handle_commit(&mut session).unwrap();
        assert!(session.tx_store.is_none());
    }

    #[tokio::test]
    async fn rollback_clears_tx_store() {
        let mut session = make_ready_session();
        handle_begin(&mut session, vec![]).unwrap();
        assert!(session.tx_store.is_some());

        handle_rollback(&mut session).unwrap();
        assert!(session.tx_store.is_none());
    }

    #[tokio::test]
    async fn reset_clears_tx_store() {
        let mut session = make_ready_session();
        handle_begin(&mut session, vec![]).unwrap();
        assert!(session.tx_store.is_some());

        handle_reset(&mut session).unwrap();
        assert!(session.tx_store.is_none());
    }

    #[tokio::test]
    async fn tx_read_your_writes() {
        let mut session = make_ready_session();

        // BEGIN, CREATE inside the TX, then read it back within the same TX.
        handle_begin(&mut session, vec![]).unwrap();
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
        handle_begin(&mut s1, vec![]).unwrap();
        run_and_pull(&mut s1, "CREATE (:RollbackNode {val: 1})").await;
        handle_rollback(&mut s1).unwrap();

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
        handle_begin(&mut s1, vec![]).unwrap();
        run_and_pull(&mut s1, "CREATE (:CommitNode {val: 2})").await;
        handle_commit(&mut s1).unwrap();

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
        handle_begin(&mut s1, vec![]).unwrap();
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
        let result = handle_run(&mut session, "RETURN 1 AS n".to_string(), vec![], vec![])
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
        let r1 = handle_run(&mut session, "RETURN 1 AS n".to_string(), vec![], vec![])
            .await
            .unwrap();
        // Pull to clear the first result
        handle_pull(&mut session, bolt_map! { "n" => BoltValue::Int(-1) }).unwrap();

        // Second RUN
        let r2 = handle_run(&mut session, "RETURN 2 AS m".to_string(), vec![], vec![])
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
        handle_begin(&mut session, vec![]).unwrap();

        // First RUN (qid=0)
        let r1 = handle_run(&mut session, "RETURN 1 AS n".to_string(), vec![], vec![])
            .await
            .unwrap();
        let qid0 = match &r1[0] {
            ServerMessage::Success(m) => m.get_int("qid").unwrap(),
            _ => panic!("expected Success"),
        };

        // Second RUN (qid=1)
        let r2 = handle_run(&mut session, "RETURN 2 AS m".to_string(), vec![], vec![])
            .await
            .unwrap();
        let qid1 = match &r2[0] {
            ServerMessage::Success(m) => m.get_int("qid").unwrap(),
            _ => panic!("expected Success"),
        };

        // Pull from qid=0 specifically
        let pull0 = handle_pull(
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
        let pull1 = handle_pull(
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
        handle_run(&mut session, "RETURN 1 AS n".to_string(), vec![], extra)
            .await
            .unwrap();
        let pull_result =
            handle_pull(&mut session, bolt_map! { "n" => BoltValue::Int(-1) }).unwrap();
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
        handle_run(
            &mut session,
            "EXPLAIN MATCH (n), (m) RETURN n, m".to_string(),
            vec![],
            vec![],
        )
        .await
        .unwrap();
        let pull_result =
            handle_pull(&mut session, bolt_map! { "n" => BoltValue::Int(-1) }).unwrap();
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
