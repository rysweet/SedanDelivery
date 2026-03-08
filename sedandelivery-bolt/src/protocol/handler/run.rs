//! RUN message handler and query plan helpers.

use std::sync::Arc;

use crate::bridge;
use crate::message::types::ServerMessage;
use crate::metrics as bolt_metrics;
use crate::packstream::value::{BoltMap, BoltMapExt, BoltValue};
use crate::protocol::state::ServerState;
use gql_core::storage::GraphStore;

use super::{
    bolt_map, extract_tx_metadata, validate_bookmarks, AdminStats, PendingQuery, ProtocolError,
    Session,
};

// -- Pending-results cap -----------------------------------------------------

const DEFAULT_MAX_PENDING_RESULTS: usize = 16;

fn max_pending_results() -> usize {
    static CAP: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *CAP.get_or_init(|| {
        std::env::var("BOLT_MAX_PENDING_RESULTS")
            .ok()
            .and_then(|v| match v.parse() {
                Ok(val) => Some(val),
                Err(_) => {
                    tracing::warn!(
                        "invalid value for BOLT_MAX_PENDING_RESULTS: {v:?}, using default {DEFAULT_MAX_PENDING_RESULTS}"
                    );
                    None
                }
            })
            .unwrap_or(DEFAULT_MAX_PENDING_RESULTS)
    })
}

// -- Query pre-processing helpers --------------------------------------------

/// Strip Neo4j pre-parser directives like `CYPHER runtime=interpreted ...`
/// from the beginning of a query string.
///
/// If the query starts with `"CYPHER "` (case-insensitive), scans forward for
/// the first recognized Cypher keyword and returns the substring starting at
/// that keyword. Otherwise returns the trimmed input unchanged.
fn strip_cypher_prefix(query: &str) -> &str {
    let trimmed = query.trim();
    if trimmed
        .get(..7)
        .map_or(false, |s| s.eq_ignore_ascii_case("CYPHER "))
    {
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
    }
}

/// Detect `EXPLAIN` or `PROFILE` prefix on a query (case-insensitive).
///
/// Returns `(is_explain, is_profile, actual_query)` where `actual_query` is
/// the query text with the prefix stripped.
fn detect_explain_profile(query: &str) -> (bool, bool, String) {
    let is_explain = query
        .get(..8)
        .map_or(false, |s| s.eq_ignore_ascii_case("EXPLAIN "));
    let is_profile = !is_explain
        && query
            .get(..8)
            .map_or(false, |s| s.eq_ignore_ascii_case("PROFILE "));
    let actual_query = if is_explain || is_profile {
        query[8..].to_string()
    } else {
        query.to_string()
    };
    (is_explain, is_profile, actual_query)
}

// -- Store resolution --------------------------------------------------------

/// Resolve the effective store for query execution.
///
/// When inside an explicit transaction (`TxReady`/`TxStreaming`), returns the
/// snapshot store so that mutations are isolated until COMMIT. Otherwise
/// returns the main session store.
fn resolve_effective_store(session: &Session) -> Arc<dyn GraphStore> {
    match &session.tx_store {
        Some(ts)
            if matches!(
                session.state,
                ServerState::TxReady | ServerState::TxStreaming
            ) =>
        {
            Arc::clone(ts)
        }
        _ => Arc::clone(&session.store),
    }
}

// -- Admin query interception ------------------------------------------------

/// Try to handle a query as an admin/DDL command (CREATE/DROP DATABASE,
/// SHOW DATABASES, CREATE/DROP INDEX/CONSTRAINT).
///
/// These commands are not supported by the gql-core parser, so we intercept
/// them before normal query execution. Returns `Some(response)` if the query
/// was handled as admin, `None` if it should proceed through normal execution.
fn intercept_admin_query(
    session: &mut Session,
    store: &Arc<dyn GraphStore>,
    graph: &str,
    trimmed: &str,
    extra: &BoltMap,
) -> Option<Result<Vec<ServerMessage>, ProtocolError>> {
    let admin_result = bridge::admin::try_handle_admin(&**store, graph, trimmed)?;

    Some(match admin_result {
        Ok(ar) => {
            let run_metadata = extract_tx_metadata(extra);
            let combined_metadata = if !session.tx_metadata.is_empty() {
                session.tx_metadata.clone()
            } else {
                run_metadata
            };
            let (records, query_type, stats, _trailing) =
                bridge::executor::result_to_records(&ar.query_result, &**store, graph);
            let fields: Vec<BoltValue> = ar
                .query_result
                .columns
                .iter()
                .map(|c: &String| BoltValue::String(c.clone()))
                .collect();

            // Determine query type: admin commands are "s" (system) when
            // they have system_updates, otherwise use the computed type.
            let effective_type = if ar.system_updates > 0 {
                "s".to_string()
            } else {
                query_type
            };

            let qid = session.next_qid;
            session.next_qid += 1;
            session.pending_results.insert(
                qid,
                PendingQuery {
                    records,
                    query_type: effective_type,
                    stats,
                    cursor: 0,
                    start_time: std::time::Instant::now(),
                    metadata: combined_metadata,
                    explain_plan: None,
                    profile_plan: None,
                    graph: graph.to_string(),
                    notifications: vec![],
                    trailing_error: None,
                    admin_stats: Some(AdminStats {
                        indexes_added: ar.indexes_added,
                        indexes_removed: ar.indexes_removed,
                        constraints_added: ar.constraints_added,
                        constraints_removed: ar.constraints_removed,
                        system_updates: ar.system_updates,
                    }),
                },
            );

            session.state = match session.state {
                ServerState::TxReady | ServerState::TxStreaming => ServerState::TxStreaming,
                _ => {
                    session.write_permit = None;
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
            bolt_metrics::error_recorded(&code);
            session.write_permit = None;
            session.state = ServerState::Failed;
            Ok(vec![ServerMessage::Failure(bolt_map! {
                "code" => BoltValue::String(code),
                "message" => BoltValue::String(message)
            })])
        }
    })
}

// -- Database existence check ------------------------------------------------

/// Check whether the target database exists when explicitly specified.
///
/// Returns `Some(failure_messages)` if the database doesn't exist, `None` if
/// the check passes or doesn't apply.
fn check_database_exists(
    session: &mut Session,
    extra: &BoltMap,
    graph: &str,
) -> Option<Vec<ServerMessage>> {
    if extra.get_str("db").is_none() || graph == "neo4j" || graph == "system" {
        return None;
    }

    let main_store_has_neo4j = session.store.graph_exists("neo4j").unwrap_or(false);
    if !main_store_has_neo4j {
        return None;
    }

    let check_store: &dyn GraphStore = match &session.tx_store {
        Some(ts)
            if matches!(
                session.state,
                ServerState::TxReady | ServerState::TxStreaming
            ) =>
        {
            &**ts
        }
        _ => &*session.store,
    };
    let exists = check_store.graph_exists(graph).unwrap_or(false);
    if exists {
        return None;
    }

    use gql_core::error::{GqlError, StorageError};
    let err = GqlError::Storage(StorageError::GraphNotFound {
        name: graph.to_string(),
    });
    let (code, message) = bridge::errors::gql_error_to_bolt(&err);
    bolt_metrics::error_recorded(&code);
    session.state = ServerState::Failed;
    Some(vec![ServerMessage::Failure(bolt_map! {
        "code" => BoltValue::String(code),
        "message" => BoltValue::String(message)
    })])
}

// -- Write semaphore acquisition ---------------------------------------------

/// Acquire the write semaphore if the query is a write and contention
/// management is needed.
///
/// Returns `Some(failure_messages)` if acquisition fails or times out, `None`
/// on success (permit stored in session) or when no acquisition is needed.
async fn acquire_write_semaphore(
    session: &mut Session,
    query: &str,
    extra: &BoltMap,
) -> Option<Vec<ServerMessage>> {
    // NOTE: This is a string-heuristic for write detection. It can produce
    // false positives (e.g. a property value containing "CREATE ") and false
    // negatives (e.g. dynamically constructed write queries). A proper solution
    // would require parsing the query AST, but this is acceptable for
    // semaphore-based contention management where the worst case is an
    // unnecessary lock acquisition or a missed lock on rare edge cases.
    let is_write_query = {
        let upper = query.to_uppercase();
        upper.contains("CREATE ")
            || upper.contains("MERGE ")
            || upper.contains("DELETE ")
            || upper.contains("SET ")
            || upper.contains("REMOVE ")
    };
    if !is_write_query || session.write_permit.is_some() {
        return None;
    }

    let in_explicit_tx = matches!(
        session.state,
        ServerState::TxReady | ServerState::TxStreaming
    );

    let timeout_ms = session.tx_timeout.or(extra
        .get_int("tx_timeout")
        .filter(|&v| v > 0)
        .map(|v| v as u64));

    if let Some(ms) = timeout_ms {
        let sem = Arc::clone(&session.write_semaphore);
        match tokio::time::timeout(std::time::Duration::from_millis(ms), sem.acquire_owned()).await
        {
            Ok(Ok(permit)) => {
                session.write_permit = Some(permit);
                None
            }
            Ok(Err(_closed)) => {
                // Semaphore closed (shutdown)
                session.state = ServerState::Failed;
                Some(vec![ServerMessage::Failure(bolt_map! {
                    "code" => BoltValue::String(
                        "Neo.TransientError.Transaction.LockClientStopped".to_string()
                    ),
                    "message" => BoltValue::String(
                        "Server is shutting down".to_string()
                    )
                })])
            }
            Err(_) => {
                // Semaphore acquisition timed out
                bolt_metrics::error_recorded("Neo.TransientError.Transaction.LockClientStopped");
                session.state = ServerState::Failed;
                Some(vec![ServerMessage::Failure(bolt_map! {
                    "code" => BoltValue::String(
                        "Neo.TransientError.Transaction.LockClientStopped".to_string()
                    ),
                    "message" => BoltValue::String(
                        format!("The transaction has been terminated. \
                                 Retry your operation in a new transaction, \
                                 and you should see a successful result. \
                                 Lock acquisition timeout after {} ms", ms)
                    )
                })])
            }
        }
    } else if in_explicit_tx {
        // Explicit transaction without timeout: acquire permit (may wait).
        let sem = Arc::clone(&session.write_semaphore);
        match sem.acquire_owned().await {
            Ok(permit) => {
                session.write_permit = Some(permit);
                None
            }
            Err(_) => {
                session.state = ServerState::Failed;
                Some(vec![ServerMessage::Failure(bolt_map! {
                    "code" => BoltValue::String(
                        "Neo.TransientError.Transaction.LockClientStopped".to_string()
                    ),
                    "message" => BoltValue::String(
                        "Server is shutting down".to_string()
                    )
                })])
            }
        }
    } else {
        // Autocommit write without tx_timeout -- skip semaphore entirely
        // for maximum concurrency. Redis handles atomicity at the command level.
        None
    }
}

// -- Success response construction -------------------------------------------

/// Build the SUCCESS response after a successful query execution.
fn build_success_response(
    session: &mut Session,
    qr: gql_core::executor::QueryResult,
    effective_store: &Arc<dyn GraphStore>,
    graph: &str,
    run_metadata: BoltMap,
    profile_plan: Option<BoltValue>,
) -> Vec<ServerMessage> {
    let (records, query_type, stats, trailing_err) =
        bridge::executor::result_to_records(&qr, &**effective_store, graph);

    // Convert the trailing GqlError (if any) to a (code, message) pair
    // for deferred FAILURE emission during PULL.
    let trailing_error = trailing_err.map(|e| bridge::errors::gql_error_to_bolt(&e));

    let fields: Vec<BoltValue> = qr
        .columns
        .iter()
        .map(|c: &String| BoltValue::String(c.clone()))
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
            records,
            query_type,
            stats,
            cursor: 0,
            start_time: std::time::Instant::now(),
            metadata: combined_metadata,
            explain_plan: None,
            profile_plan: patched_profile,
            graph: graph.to_string(),
            notifications: vec![],
            trailing_error,
            admin_stats: None,
        },
    );

    session.state = match session.state {
        ServerState::TxReady | ServerState::TxStreaming => ServerState::TxStreaming,
        _ => {
            // For autocommit queries, release the write lock immediately
            // since the query has completed.
            session.write_permit = None;
            ServerState::Streaming
        }
    };

    vec![ServerMessage::Success(bolt_map! {
        "fields" => BoltValue::List(fields),
        "t_first" => BoltValue::Int(0),
        "qid" => BoltValue::Int(qid)
    })]
}

// -- RUN ---------------------------------------------------------------------

/// Handle the RUN message (submit a Cypher query for execution).
///
/// Parses the query, resolves the target graph and parameters, executes the
/// query (or plans it for EXPLAIN/PROFILE), and stores the result as a
/// `PendingQuery` for subsequent PULL.
pub(crate) async fn handle_run(
    session: &mut Session,
    query: String,
    params: BoltMap,
    extra: BoltMap,
) -> Result<Vec<ServerMessage>, ProtocolError> {
    if let Some(failure) = validate_bookmarks(&extra) {
        session.state = ServerState::Failed;
        return Ok(vec![failure]);
    }

    // Reject if too many pending results (prevents OOM from misbehaving clients).
    let cap = max_pending_results();
    if session.pending_results.len() >= cap {
        session.state = ServerState::Failed;
        return Ok(vec![ServerMessage::Failure(bolt_map! {
            "code" => BoltValue::String(
                "Neo.ClientError.Request.Invalid".to_string()
            ),
            "message" => BoltValue::String(
                format!(
                    "Too many pending results ({}). Send PULL or DISCARD before running more queries.",
                    cap
                )
            )
        })]);
    }

    let trimmed = strip_cypher_prefix(&query);

    tracing::debug!(query = %trimmed, "handle_run query");
    let (is_explain, is_profile, actual_query) = detect_explain_profile(trimmed);

    let graph = extra
        .get_str("db")
        .unwrap_or(&session.current_graph)
        .trim_matches(|c| c == '\'' || c == '"')
        .to_string();

    // Try to handle as an admin/DDL query.
    let effective_store = resolve_effective_store(session);
    if let Some(result) = intercept_admin_query(session, &effective_store, &graph, trimmed, &extra)
    {
        return result;
    }

    // Check that the target database exists (when explicitly specified).
    if let Some(failure) = check_database_exists(session, &extra, &graph) {
        return Ok(failure);
    }

    let run_metadata = extract_tx_metadata(&extra);
    let timeout_ms = extra
        .get_int("tx_timeout")
        .filter(|&v| v > 0)
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

    // Re-resolve effective store (state has not changed since admin check,
    // but we need a fresh Arc for the execution closures below).
    let effective_store = resolve_effective_store(session);

    // EXPLAIN: plan only, no execution
    if is_explain {
        return handle_explain(
            session,
            &effective_store,
            &graph,
            &actual_query,
            value_params,
            run_metadata,
        )
        .await;
    }

    // Acquire the write semaphore if needed.
    if let Some(failure) = acquire_write_semaphore(session, &actual_query, &extra).await {
        return Ok(failure);
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
            bridge::executor::plan_query(&*store2, &g2, &q2, p2)
        })
        .await
        .map_err(|e| ProtocolError::Internal(e.to_string()))?;
        plan.map_err(|e| {
            tracing::debug!(error = %e, "PROFILE plan_query failed");
            e
        })
        .ok()
        .map(|op| operator_to_plan_map(&op, true, 0))
    } else {
        None
    };

    let result = tokio::task::spawn_blocking(move || {
        bridge::executor::execute_query(
            &*store,
            &graph_clone,
            &query_clone,
            value_params,
            timeout_ms,
        )
    })
    .await
    .map_err(|e| ProtocolError::Internal(format!("spawn_blocking join error: {}", e)))?;

    match result {
        Ok(qr) => Ok(build_success_response(
            session,
            qr,
            &effective_store,
            &graph,
            run_metadata,
            profile_plan,
        )),
        Err(gql_err) => {
            let (code, message) = bridge::errors::gql_error_to_bolt(&gql_err);
            bolt_metrics::error_recorded(&code);
            // Release write lock on error
            session.write_permit = None;
            session.state = ServerState::Failed;
            Ok(vec![ServerMessage::Failure(bolt_map! {
                "code" => BoltValue::String(code),
                "message" => BoltValue::String(message)
            })])
        }
    }
}

// -- EXPLAIN helper ----------------------------------------------------------

/// Handle an EXPLAIN query: plan only, no execution.
async fn handle_explain(
    session: &mut Session,
    effective_store: &Arc<dyn GraphStore>,
    graph: &str,
    actual_query: &str,
    value_params: std::collections::HashMap<String, gql_core::types::Value>,
    run_metadata: BoltMap,
) -> Result<Vec<ServerMessage>, ProtocolError> {
    let store = Arc::clone(effective_store);
    let g = graph.to_string();
    let q = actual_query.to_string();
    let p = value_params;
    let plan_result =
        tokio::task::spawn_blocking(move || bridge::executor::plan_query(&*store, &g, &q, p))
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
                    records: vec![],
                    query_type: "r".to_string(),
                    stats: vec![],
                    cursor: 0,
                    start_time: std::time::Instant::now(),
                    metadata: run_metadata,
                    explain_plan: Some(plan_map),
                    profile_plan: None,
                    graph: graph.to_string(),
                    notifications: plan_notifications,
                    trailing_error: None,
                    admin_stats: None,
                },
            );
            session.state = match session.state {
                ServerState::TxReady | ServerState::TxStreaming => ServerState::TxStreaming,
                _ => ServerState::Streaming,
            };
            Ok(vec![ServerMessage::Success(bolt_map! {
                "fields" => BoltValue::List(vec![]),
                "t_first" => BoltValue::Int(0),
                "qid" => BoltValue::Int(qid)
            })])
        }
        Err(e) => {
            let (code, message) = bridge::errors::gql_error_to_bolt(&e);
            bolt_metrics::error_recorded(&code);
            session.state = ServerState::Failed;
            Ok(vec![ServerMessage::Failure(bolt_map! {
                "code" => BoltValue::String(code),
                "message" => BoltValue::String(message)
            })])
        }
    }
}

// -- Plan conversion ---------------------------------------------------------

/// Convert a gql-core Operator tree into a Bolt plan map structure.
///
/// Recursively descends into child operators so EXPLAIN returns a proper tree.
#[allow(clippy::only_used_in_recursion)]
fn operator_to_plan_map(
    op: &gql_core::planner::Operator,
    is_profile: bool,
    depth: usize,
) -> BoltValue {
    use gql_core::planner::Operator;

    let op_type = format!("{:?}", op)
        .split('(')
        .next()
        .unwrap_or("Unknown")
        .split('{')
        .next()
        .unwrap_or("Unknown")
        .trim()
        .to_string();
    let children_bolt: Vec<BoltValue> = op
        .children()
        .into_iter()
        .map(|child| operator_to_plan_map(child, is_profile, depth + 1))
        .collect();
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
        ("children".to_string(), BoltValue::List(children_bolt)),
    ];
    BoltValue::Map(entries)
}

// -- Notification detection --------------------------------------------------

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packstream::value::BoltMapExt;
    use gql_core::ast::Identifier;
    use gql_core::planner::Operator;

    // ── strip_cypher_prefix ─────────────────────────────────────────────

    #[test]
    fn strip_cypher_prefix_no_prefix() {
        assert_eq!(
            strip_cypher_prefix("MATCH (n) RETURN n"),
            "MATCH (n) RETURN n"
        );
    }

    #[test]
    fn strip_cypher_prefix_with_runtime_directive() {
        let q = "CYPHER runtime=interpreted MATCH (n) RETURN n";
        assert_eq!(strip_cypher_prefix(q), "MATCH (n) RETURN n");
    }

    #[test]
    fn strip_cypher_prefix_case_insensitive() {
        let q = "cypher planner=cost RETURN 1";
        assert_eq!(strip_cypher_prefix(q), "RETURN 1");
    }

    #[test]
    fn strip_cypher_prefix_mixed_case() {
        let q = "Cypher runtime=slotted CREATE (n:Foo)";
        assert_eq!(strip_cypher_prefix(q), "CREATE (n:Foo)");
    }

    #[test]
    fn strip_cypher_prefix_with_explain() {
        let q = "CYPHER version=3.5 EXPLAIN MATCH (n) RETURN n";
        assert_eq!(strip_cypher_prefix(q), "EXPLAIN MATCH (n) RETURN n");
    }

    #[test]
    fn strip_cypher_prefix_with_profile() {
        let q = "CYPHER runtime=compiled PROFILE MATCH (n) RETURN n";
        assert_eq!(strip_cypher_prefix(q), "PROFILE MATCH (n) RETURN n");
    }

    #[test]
    fn strip_cypher_prefix_trims_whitespace() {
        assert_eq!(strip_cypher_prefix("  RETURN 1  "), "RETURN 1");
    }

    #[test]
    fn strip_cypher_prefix_short_string() {
        assert_eq!(strip_cypher_prefix("RET"), "RET");
    }

    #[test]
    fn strip_cypher_prefix_exactly_cypher_space() {
        let q = "CYPHER  ";
        let result = strip_cypher_prefix(q);
        assert!(!result.is_empty() || q.trim().is_empty());
    }

    // ── detect_explain_profile ──────────────────────────────────────────

    #[test]
    fn detect_explain_prefix() {
        let (is_explain, is_profile, actual) = detect_explain_profile("EXPLAIN MATCH (n) RETURN n");
        assert!(is_explain);
        assert!(!is_profile);
        assert_eq!(actual, "MATCH (n) RETURN n");
    }

    #[test]
    fn detect_profile_prefix() {
        let (is_explain, is_profile, actual) = detect_explain_profile("PROFILE MATCH (n) RETURN n");
        assert!(!is_explain);
        assert!(is_profile);
        assert_eq!(actual, "MATCH (n) RETURN n");
    }

    #[test]
    fn detect_explain_case_insensitive() {
        let (is_explain, _, actual) = detect_explain_profile("explain RETURN 1");
        assert!(is_explain);
        assert_eq!(actual, "RETURN 1");
    }

    #[test]
    fn detect_profile_case_insensitive() {
        let (_, is_profile, actual) = detect_explain_profile("profile RETURN 1");
        assert!(is_profile);
        assert_eq!(actual, "RETURN 1");
    }

    #[test]
    fn detect_normal_query_no_prefix() {
        let (is_explain, is_profile, actual) = detect_explain_profile("MATCH (n) RETURN n");
        assert!(!is_explain);
        assert!(!is_profile);
        assert_eq!(actual, "MATCH (n) RETURN n");
    }

    #[test]
    fn detect_explain_profile_short_query() {
        let (is_explain, is_profile, actual) = detect_explain_profile("RET");
        assert!(!is_explain);
        assert!(!is_profile);
        assert_eq!(actual, "RET");
    }

    #[test]
    fn detect_explain_profile_empty() {
        let (is_explain, is_profile, actual) = detect_explain_profile("");
        assert!(!is_explain);
        assert!(!is_profile);
        assert_eq!(actual, "");
    }

    #[test]
    fn detect_explain_not_profile() {
        let (is_explain, is_profile, _) =
            detect_explain_profile("EXPLAIN PROFILE MATCH (n) RETURN n");
        assert!(is_explain);
        assert!(!is_profile);
    }

    // ── operator_to_plan_map ────────────────────────────────────────────

    #[test]
    fn operator_to_plan_map_single_row() {
        let op = Operator::SingleRow;
        let plan = operator_to_plan_map(&op, false, 0);
        match plan {
            BoltValue::Map(entries) => {
                let op_type = entries.get_str("operatorType").unwrap();
                assert_eq!(op_type, "SingleRow");
                assert!(entries.get_value("children").is_some());
                assert!(entries.get_value("args").is_some());
                assert!(entries.get_value("identifiers").is_some());
            }
            _ => panic!("expected BoltValue::Map"),
        }
    }

    #[test]
    fn operator_to_plan_map_node_scan() {
        let op = Operator::NodeScan {
            label: None,
            alias: Identifier::new("n"),
        };
        let plan = operator_to_plan_map(&op, false, 0);
        match plan {
            BoltValue::Map(entries) => {
                let op_type = entries.get_str("operatorType").unwrap();
                assert_eq!(op_type, "NodeScan");
            }
            _ => panic!("expected BoltValue::Map"),
        }
    }

    #[test]
    fn operator_to_plan_map_filter_wrapping_scan() {
        let child = Operator::NodeScan {
            label: None,
            alias: Identifier::new("n"),
        };
        let op = Operator::Filter {
            predicate: gql_core::ast::Expr::Bool(true),
            child: Box::new(child),
        };
        let plan = operator_to_plan_map(&op, false, 0);
        match plan {
            BoltValue::Map(entries) => {
                let op_type = entries.get_str("operatorType").unwrap();
                assert_eq!(op_type, "Filter");
                if let Some(BoltValue::List(children)) = entries.get_value("children") {
                    assert_eq!(children.len(), 1, "Filter should have one child (NodeScan)");
                    // Verify child is a NodeScan
                    if let BoltValue::Map(child_entries) = &children[0] {
                        let child_type = child_entries.get_str("operatorType").unwrap();
                        assert_eq!(child_type, "NodeScan");
                    } else {
                        panic!("expected child to be a Map");
                    }
                }
            }
            _ => panic!("expected BoltValue::Map"),
        }
    }

    #[test]
    fn operator_to_plan_map_has_runtime_in_args() {
        let op = Operator::SingleRow;
        let plan = operator_to_plan_map(&op, true, 0);
        match plan {
            BoltValue::Map(entries) => {
                if let Some(BoltValue::Map(args)) = entries.get_value("args") {
                    let runtime = args.get_str("runtime").unwrap();
                    assert_eq!(runtime, "INTERPRETED");
                } else {
                    panic!("expected args to be a Map");
                }
            }
            _ => panic!("expected BoltValue::Map"),
        }
    }

    // ── detect_notifications / detect_cartesian_products ────────────────

    #[test]
    fn detect_notifications_empty_for_single_scan() {
        let op = Operator::NodeScan {
            label: None,
            alias: Identifier::new("n"),
        };
        let notes = detect_notifications(&op);
        assert!(notes.is_empty());
    }

    #[test]
    fn detect_cartesian_product_on_disjoint_apply() {
        let outer = Operator::NodeScan {
            label: None,
            alias: Identifier::new("a"),
        };
        let inner = Operator::NodeScan {
            label: None,
            alias: Identifier::new("b"),
        };
        let op = Operator::Apply {
            outer: Box::new(outer),
            inner: Box::new(inner),
        };
        let notes = detect_notifications(&op);
        assert_eq!(notes.len(), 1);
        if let BoltValue::Map(entries) = &notes[0] {
            let code = entries.get_str("code").unwrap();
            assert!(code.contains("CartesianProduct"));
            let severity = entries.get_str("severity").unwrap();
            assert_eq!(severity, "WARNING");
            let category = entries.get_str("category").unwrap();
            assert_eq!(category, "PERFORMANCE");
        } else {
            panic!("expected Map notification");
        }
    }

    #[test]
    fn detect_no_cartesian_product_on_shared_alias() {
        let outer = Operator::NodeScan {
            label: None,
            alias: Identifier::new("n"),
        };
        let inner = Operator::NodeScan {
            label: None,
            alias: Identifier::new("n"),
        };
        let op = Operator::Apply {
            outer: Box::new(outer),
            inner: Box::new(inner),
        };
        let notes = detect_notifications(&op);
        assert!(
            notes.is_empty(),
            "shared alias should not trigger cartesian warning"
        );
    }

    #[test]
    fn detect_cartesian_products_recurses_through_filter() {
        let outer = Operator::NodeScan {
            label: None,
            alias: Identifier::new("a"),
        };
        let inner = Operator::NodeScan {
            label: None,
            alias: Identifier::new("b"),
        };
        let apply = Operator::Apply {
            outer: Box::new(outer),
            inner: Box::new(inner),
        };
        let filter = Operator::Filter {
            predicate: gql_core::ast::Expr::Bool(true),
            child: Box::new(apply),
        };
        let notes = detect_notifications(&filter);
        assert_eq!(notes.len(), 1);
    }

    #[test]
    fn detect_cartesian_notification_has_position() {
        let outer = Operator::NodeScan {
            label: None,
            alias: Identifier::new("x"),
        };
        let inner = Operator::NodeScan {
            label: None,
            alias: Identifier::new("y"),
        };
        let op = Operator::Apply {
            outer: Box::new(outer),
            inner: Box::new(inner),
        };
        let notes = detect_notifications(&op);
        if let BoltValue::Map(entries) = &notes[0] {
            if let Some(BoltValue::Map(pos)) = entries.get_value("position") {
                assert_eq!(pos.get_int("offset"), Some(0));
                assert_eq!(pos.get_int("line"), Some(1));
                assert_eq!(pos.get_int("column"), Some(1));
            } else {
                panic!("expected position map");
            }
        }
    }

    #[test]
    fn detect_notifications_on_leaf_operators() {
        let single_row = Operator::SingleRow;
        assert!(detect_notifications(&single_row).is_empty());
    }

    #[test]
    fn detect_cartesian_products_union() {
        let outer = Operator::NodeScan {
            label: None,
            alias: Identifier::new("a"),
        };
        let inner = Operator::NodeScan {
            label: None,
            alias: Identifier::new("b"),
        };
        let apply = Operator::Apply {
            outer: Box::new(outer),
            inner: Box::new(inner),
        };
        let right = Operator::SingleRow;
        let union_op = Operator::Union {
            left: Box::new(apply),
            right: Box::new(right),
            all: true,
        };
        let notes = detect_notifications(&union_op);
        assert_eq!(
            notes.len(),
            1,
            "should find cartesian product in left branch of union"
        );
    }
}
