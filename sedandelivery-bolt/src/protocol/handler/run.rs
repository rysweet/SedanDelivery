//! RUN message handler and query pre-processing.

use std::sync::Arc;

use crate::bridge;
use crate::message::types::ServerMessage;
use crate::metrics as bolt_metrics;
use crate::packstream::value::{BoltMap, BoltMapExt, BoltValue};
use crate::protocol::state::ServerState;
use gql_core::storage::{GraphStore, ReadConsistency};

use super::explain::{detect_notifications, operator_to_plan_map, operator_to_profile_plan_map};
use super::{
    bolt_map, extract_tx_metadata, recent_bookmark_age, validate_bookmarks, AdminStats,
    PendingQuery, ProtocolError, Session,
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
        .is_some_and(|s| s.eq_ignore_ascii_case("CYPHER "))
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
        .is_some_and(|s| s.eq_ignore_ascii_case("EXPLAIN "));
    let is_profile = !is_explain
        && query
            .get(..8)
            .is_some_and(|s| s.eq_ignore_ascii_case("PROFILE "));
    let actual_query = if is_explain || is_profile {
        query[8..].to_string()
    } else {
        query.to_string()
    };
    (is_explain, is_profile, actual_query)
}

// -- USE database parsing ----------------------------------------------------

/// Parse a `USE <database>` prefix from a query string.
///
/// Returns `(db_name, rest)` where `rest` is the remaining query after the
/// USE clause (trimmed), or an empty string if the USE is the entire query.
/// Returns `None` if the query does not start with `USE `.
fn parse_use_prefix(query: &str) -> Option<(String, &str)> {
    let trimmed = query.trim();
    let upper = trimmed.to_uppercase();
    if !upper.starts_with("USE ") {
        return None;
    }
    let rest = trimmed["USE ".len()..].trim();
    if rest.is_empty() {
        return None;
    }
    // Extract the database name. Handle backtick-quoted and simple identifiers.
    let (name, name_end): (String, usize) = if rest.starts_with('`') {
        // Backtick-quoted: scan for closing backtick.
        let bytes = rest.as_bytes();
        let mut i = 1;
        let mut result = String::new();
        while i < bytes.len() {
            if bytes[i] == b'`' {
                if i + 1 < bytes.len() && bytes[i + 1] == b'`' {
                    result.push('`');
                    i += 2;
                } else {
                    return Some((result, rest[i + 1..].trim()));
                }
            } else {
                result.push(bytes[i] as char);
                i += 1;
            }
        }
        (result, rest.len())
    } else {
        // Simple identifier: alphanumeric + underscore + hyphen + dot
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '-' || *c == '.')
            .collect();
        let end = name.len();
        (name, end)
    };
    if name.is_empty() {
        None
    } else {
        let after_name = rest[name_end..].trim();
        Some((name, after_name))
    }
}

/// Parse `USE <database>` standalone command. Returns the database name if
/// the query is purely a USE switch (no trailing content), `None` otherwise.
fn parse_use_database(query: &str) -> Option<String> {
    parse_use_prefix(query).and_then(
        |(name, rest)| {
            if rest.is_empty() {
                Some(name)
            } else {
                None
            }
        },
    )
}

// -- Store resolution --------------------------------------------------------

/// Resolve the effective store for query execution.
///
/// When inside an explicit transaction (`TxReady`/`TxStreaming`), returns the
/// snapshot store so that mutations are isolated until COMMIT. Otherwise
/// returns the main session store.
fn apply_read_consistency(session: &Session, store: Arc<dyn GraphStore>) -> Arc<dyn GraphStore> {
    store
        .with_read_consistency(session.read_consistency())
        .unwrap_or(store)
}

fn resolve_effective_store(session: &Session) -> Arc<dyn GraphStore> {
    let base_store = match &session.tx_store {
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
    apply_read_consistency(session, base_store)
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
            let admin_wrote = ar.system_updates > 0
                || ar.indexes_added > 0
                || ar.indexes_removed > 0
                || ar.constraints_added > 0
                || ar.constraints_removed > 0;

            let qid = match session.allocate_qid() {
                Ok(q) => q,
                Err(e) => return Some(Err(e)),
            };

            // Track writes for read-after-write session pinning.
            if effective_type.contains('w') || admin_wrote {
                session.record_successful_write();
            }

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

    let main_store_has_neo4j = apply_read_consistency(session, Arc::clone(&session.store))
        .graph_exists("neo4j")
        .unwrap_or(false);
    if !main_store_has_neo4j {
        return None;
    }

    let check_store = resolve_effective_store(session);
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
) -> Result<Vec<ServerMessage>, ProtocolError> {
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

    // PROFILE plan is already annotated with per-operator rows and dbHits
    // by operator_to_profile_plan_map, so no patching is needed.
    let patched_profile = profile_plan;

    let qid = session.allocate_qid()?;

    // Track writes for read-after-write session pinning.
    if query_type.contains('w') {
        session.record_successful_write();
    }

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

    Ok(vec![ServerMessage::Success(bolt_map! {
        "fields" => BoltValue::List(fields),
        "t_first" => BoltValue::Int(0),
        "qid" => BoltValue::Int(qid)
    })])
}

// -- RUN ---------------------------------------------------------------------

/// Handle the RUN message (submit a Cypher query for execution).
///
/// Parses the query, resolves the target graph and parameters, executes the
/// query (or plans it for EXPLAIN/PROFILE), and stores the result as a
/// `PendingQuery` for subsequent PULL.
#[tracing::instrument(name = "bolt.run", skip_all, fields(query = %query))]
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
    if let Some(age) = recent_bookmark_age(&extra, &session.global_bookmark) {
        session.apply_recent_bookmark_age(age);
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

    // Handle compound `USE <database> <rest>` — switch graph then run rest.
    // e.g. "USE mygraph MATCH (n) RETURN n" — set mygraph, run the MATCH.
    if let Some((db_name, rest_query)) = parse_use_prefix(trimmed) {
        if !rest_query.is_empty() {
            // Switch the session graph for this request.
            session.current_graph = db_name;
            // Rewrite the query to be just the rest (without the USE prefix) and
            // fall through to the normal execution path below.
            return Box::pin(handle_run(session, rest_query.to_string(), params, extra)).await;
        }
    }

    // Handle `USE <database>` — standalone switch of the session's active graph.
    if let Some(db_name) = parse_use_database(trimmed) {
        session.current_graph = db_name.clone();
        let qid = session.allocate_qid()?;
        session.pending_results.insert(
            qid,
            PendingQuery {
                records: vec![],
                query_type: "s".to_string(),
                stats: vec![],
                cursor: 0,
                start_time: std::time::Instant::now(),
                metadata: vec![],
                explain_plan: None,
                profile_plan: None,
                graph: db_name,
                notifications: vec![],
                trailing_error: None,
                admin_stats: Some(AdminStats {
                    indexes_added: 0,
                    indexes_removed: 0,
                    constraints_added: 0,
                    constraints_removed: 0,
                    system_updates: 1,
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
        return Ok(vec![ServerMessage::Success(bolt_map! {
            "fields" => BoltValue::List(vec![]),
            "t_first" => BoltValue::Int(0),
            "qid" => BoltValue::Int(qid)
        })]);
    }

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

    if matches!(
        session.read_consistency(),
        ReadConsistency::SessionFreshPrimary
    ) {
        tracing::debug!(
            graph = %graph,
            "routing reads to primary (session freshness window active)"
        );
    }

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

    if is_profile {
        // PROFILE: execute with profiling enabled, then build annotated plan.
        // The plan annotation closure runs inside execute_query_with_profiling
        // while the plan's pointer addresses still match the profile stats keys.
        let store_for_profile = Arc::clone(&effective_store);
        let graph_for_profile = graph.clone();
        let query_for_profile = actual_query.clone();
        let params_for_profile = value_params;

        let result = tokio::task::spawn_blocking(move || {
            let store_ref = &*store_for_profile;
            let graph_ref = &graph_for_profile;
            bridge::executor::execute_query_with_profiling(
                store_ref,
                graph_ref,
                &query_for_profile,
                params_for_profile,
                timeout_ms,
                |plan, profile_stats| {
                    let est = gql_core::planner::collect_graph_statistics(store_ref, graph_ref)
                        .ok()
                        .map(|gs| gql_core::planner::estimate_cardinality(plan, &gs));
                    operator_to_profile_plan_map(plan, 0, est.as_ref(), profile_stats)
                },
            )
        })
        .await
        .map_err(|e| ProtocolError::Internal(format!("blocking task join error: {}", e)))?;

        match result {
            Ok((qr, profile_plan)) => build_success_response(
                session,
                qr,
                &effective_store,
                &graph,
                run_metadata,
                Some(profile_plan),
            ),
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
        }
    } else {
        // Normal (non-PROFILE) execution
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
        .map_err(|e| ProtocolError::Internal(format!("blocking task join error: {}", e)))?;

        match result {
            Ok(qr) => {
                build_success_response(session, qr, &effective_store, &graph, run_metadata, None)
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
            let estimates =
                gql_core::planner::collect_graph_statistics(effective_store.as_ref(), graph)
                    .ok()
                    .map(|gs| gql_core::planner::estimate_cardinality(&operator, &gs));
            let plan_map = operator_to_plan_map(&operator, false, 0, estimates.as_ref());
            let plan_notifications = detect_notifications(&operator);
            let qid = session.allocate_qid()?;
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

#[cfg(test)]
mod tests {
    use super::*;

    use std::any::Any;
    use std::collections::HashMap;
    use std::sync::atomic::AtomicU64;

    use gql_core::storage::{
        AdjEntry, EdgeData, EdgeId, GraphStats, GraphStore, InMemoryGraphStore, IndexDescriptor,
        NodeData, NodeId, RangeQuery, ReadConsistency,
    };
    use gql_core::types::Value;

    struct TrackingStore {
        inner: Arc<InMemoryGraphStore>,
        applied_consistency: Option<ReadConsistency>,
    }

    impl TrackingStore {
        fn new() -> Self {
            Self {
                inner: Arc::new(InMemoryGraphStore::new()),
                applied_consistency: None,
            }
        }
    }

    macro_rules! delegate {
        ($name:ident(&self) -> $ret:ty) => {
            fn $name(&self) -> $ret {
                self.inner.$name()
            }
        };
        ($name:ident(&self, $($arg:ident : $ty:ty),+) -> $ret:ty) => {
            fn $name(&self, $($arg: $ty),+) -> $ret {
                self.inner.$name($($arg),+)
            }
        };
    }

    impl GraphStore for TrackingStore {
        delegate!(get_node(&self, graph: &str, id: NodeId) -> Result<Option<NodeData>, gql_core::error::StorageError>);
        delegate!(get_edge(&self, graph: &str, id: EdgeId) -> Result<Option<EdgeData>, gql_core::error::StorageError>);

        fn get_nodes_batch(
            &self,
            graph: &str,
            ids: &[NodeId],
        ) -> Result<Vec<Option<NodeData>>, gql_core::error::StorageError> {
            self.inner.get_nodes_batch(graph, ids)
        }

        fn get_edges_batch(
            &self,
            graph: &str,
            ids: &[EdgeId],
        ) -> Result<Vec<Option<EdgeData>>, gql_core::error::StorageError> {
            self.inner.get_edges_batch(graph, ids)
        }

        delegate!(find_nodes_by_label(&self, graph: &str, label: &str) -> Result<Vec<NodeId>, gql_core::error::StorageError>);

        fn find_nodes_by_label_and_property(
            &self,
            graph: &str,
            label: &str,
            property: &str,
            range: RangeQuery<'_>,
        ) -> Result<Vec<NodeId>, gql_core::error::StorageError> {
            self.inner
                .find_nodes_by_label_and_property(graph, label, property, range)
        }

        delegate!(find_edges_by_type(&self, graph: &str, rel_type: &str) -> Result<Vec<EdgeId>, gql_core::error::StorageError>);
        delegate!(out_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, gql_core::error::StorageError>);
        delegate!(in_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, gql_core::error::StorageError>);

        fn out_edges_by_type(
            &self,
            graph: &str,
            node_id: NodeId,
            rel_types: &[&str],
        ) -> Result<Vec<AdjEntry>, gql_core::error::StorageError> {
            self.inner.out_edges_by_type(graph, node_id, rel_types)
        }

        fn in_edges_by_type(
            &self,
            graph: &str,
            node_id: NodeId,
            rel_types: &[&str],
        ) -> Result<Vec<AdjEntry>, gql_core::error::StorageError> {
            self.inner.in_edges_by_type(graph, node_id, rel_types)
        }

        delegate!(out_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, gql_core::error::StorageError>);
        delegate!(in_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, gql_core::error::StorageError>);
        delegate!(all_node_ids(&self, graph: &str) -> Result<Vec<NodeId>, gql_core::error::StorageError>);
        delegate!(all_edge_ids(&self, graph: &str) -> Result<Vec<EdgeId>, gql_core::error::StorageError>);

        fn create_node(
            &self,
            graph: &str,
            labels: Vec<String>,
            properties: HashMap<String, Value>,
        ) -> Result<NodeId, gql_core::error::StorageError> {
            self.inner.create_node(graph, labels, properties)
        }

        fn create_edge(
            &self,
            graph: &str,
            src: NodeId,
            dst: NodeId,
            rel_type: String,
            properties: HashMap<String, Value>,
        ) -> Result<EdgeId, gql_core::error::StorageError> {
            self.inner
                .create_edge(graph, src, dst, rel_type, properties)
        }

        fn set_node_properties(
            &self,
            graph: &str,
            id: NodeId,
            properties: HashMap<String, Value>,
        ) -> Result<(), gql_core::error::StorageError> {
            self.inner.set_node_properties(graph, id, properties)
        }

        fn replace_node_properties(
            &self,
            graph: &str,
            id: NodeId,
            properties: HashMap<String, Value>,
        ) -> Result<(), gql_core::error::StorageError> {
            self.inner.replace_node_properties(graph, id, properties)
        }

        fn set_edge_properties(
            &self,
            graph: &str,
            id: EdgeId,
            properties: HashMap<String, Value>,
        ) -> Result<(), gql_core::error::StorageError> {
            self.inner.set_edge_properties(graph, id, properties)
        }

        fn replace_edge_properties(
            &self,
            graph: &str,
            id: EdgeId,
            properties: HashMap<String, Value>,
        ) -> Result<(), gql_core::error::StorageError> {
            self.inner.replace_edge_properties(graph, id, properties)
        }

        delegate!(remove_node_property(&self, graph: &str, id: NodeId, key: &str) -> Result<(), gql_core::error::StorageError>);
        delegate!(remove_edge_property(&self, graph: &str, id: EdgeId, key: &str) -> Result<(), gql_core::error::StorageError>);

        fn add_node_labels(
            &self,
            graph: &str,
            id: NodeId,
            labels: &[String],
        ) -> Result<(), gql_core::error::StorageError> {
            self.inner.add_node_labels(graph, id, labels)
        }

        fn remove_node_labels(
            &self,
            graph: &str,
            id: NodeId,
            labels: &[String],
        ) -> Result<(), gql_core::error::StorageError> {
            self.inner.remove_node_labels(graph, id, labels)
        }

        delegate!(delete_node(&self, graph: &str, id: NodeId, detach: bool) -> Result<(), gql_core::error::StorageError>);
        delegate!(delete_edge(&self, graph: &str, id: EdgeId) -> Result<(), gql_core::error::StorageError>);
        delegate!(list_indexes(&self, graph: &str) -> Result<Vec<IndexDescriptor>, gql_core::error::StorageError>);

        fn create_index(
            &self,
            graph: &str,
            label: &str,
            property: &str,
        ) -> Result<(), gql_core::error::StorageError> {
            self.inner.create_index(graph, label, property)
        }

        fn drop_index(
            &self,
            graph: &str,
            label: &str,
            property: &str,
        ) -> Result<(), gql_core::error::StorageError> {
            self.inner.drop_index(graph, label, property)
        }

        delegate!(create_graph(&self, name: &str) -> Result<(), gql_core::error::StorageError>);
        delegate!(delete_graph(&self, name: &str) -> Result<(), gql_core::error::StorageError>);
        delegate!(graph_exists(&self, name: &str) -> Result<bool, gql_core::error::StorageError>);
        delegate!(list_graphs(&self) -> Result<Vec<String>, gql_core::error::StorageError>);
        delegate!(graph_stats(&self, graph: &str) -> Result<GraphStats, gql_core::error::StorageError>);
        delegate!(all_label_names(&self, graph: &str) -> Result<Vec<String>, gql_core::error::StorageError>);

        fn with_read_consistency(
            &self,
            consistency: ReadConsistency,
        ) -> Option<Arc<dyn GraphStore>> {
            Some(Arc::new(Self {
                inner: Arc::clone(&self.inner),
                applied_consistency: Some(consistency),
            }))
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    fn ready_session_with_store(store: Arc<dyn GraphStore>) -> Session {
        ready_session_with_store_and_bookmark(
            store,
            Arc::new(crate::bookmark::BookmarkCounter::InMemory(AtomicU64::new(
                0,
            ))),
        )
    }

    fn ready_session_with_store_and_bookmark(
        store: Arc<dyn GraphStore>,
        global_bookmark: Arc<crate::bookmark::BookmarkCounter>,
    ) -> Session {
        Session {
            state: ServerState::Ready,
            store,
            conn_id: 1,
            current_graph: "neo4j".to_string(),
            pending_results: HashMap::new(),
            next_qid: 0,
            global_bookmark,
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
    fn resolve_effective_store_uses_topology_default_view_without_recent_write() {
        let store: Arc<dyn GraphStore> = Arc::new(TrackingStore::new());
        let session = ready_session_with_store(store);

        let effective_store = resolve_effective_store(&session);
        let tracking_store = effective_store
            .as_any()
            .downcast_ref::<TrackingStore>()
            .expect("tracking store view should be returned");
        assert_eq!(
            tracking_store.applied_consistency,
            Some(ReadConsistency::TopologyDefault)
        );
    }

    #[test]
    fn resolve_effective_store_uses_session_fresh_primary_view_with_recent_write() {
        let store: Arc<dyn GraphStore> = Arc::new(TrackingStore::new());
        let mut session = ready_session_with_store(store);
        session.last_write_at = Some(std::time::Instant::now() - std::time::Duration::from_secs(1));

        let effective_store = resolve_effective_store(&session);
        let tracking_store = effective_store
            .as_any()
            .downcast_ref::<TrackingStore>()
            .expect("tracking store view should be returned");
        assert_eq!(
            tracking_store.applied_consistency,
            Some(ReadConsistency::SessionFreshPrimary)
        );
    }

    #[tokio::test]
    async fn handle_run_rehydrates_primary_pin_from_recent_bookmark() {
        let store: Arc<dyn GraphStore> = Arc::new(TrackingStore::new());
        let global_bookmark = Arc::new(crate::bookmark::BookmarkCounter::InMemory(AtomicU64::new(
            0,
        )));
        let bookmark = crate::protocol::handler::next_bookmark(global_bookmark.as_ref());
        let mut session = ready_session_with_store_and_bookmark(store, global_bookmark);

        let _ = handle_run(
            &mut session,
            "RETURN 1 AS n".to_string(),
            vec![],
            bolt_map! {
                "bookmarks" => BoltValue::List(vec![BoltValue::String(bookmark)])
            },
        )
        .await
        .expect("RUN should succeed");

        assert_eq!(
            session.read_consistency(),
            ReadConsistency::SessionFreshPrimary
        );
    }

    #[test]
    fn resolve_effective_store_falls_back_for_stores_without_read_views() {
        let store: Arc<dyn GraphStore> = Arc::new(InMemoryGraphStore::new());
        let session = ready_session_with_store(Arc::clone(&store));

        let effective_store = resolve_effective_store(&session);
        assert!(
            Arc::ptr_eq(&store, &effective_store),
            "stores without read-view support should be reused directly"
        );
    }

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

    // ── parse_use_database ──────────────────────────────────────────────

    #[test]
    fn parse_use_simple_database() {
        assert_eq!(parse_use_database("USE mydb"), Some("mydb".to_string()));
    }

    #[test]
    fn parse_use_database_case_insensitive() {
        assert_eq!(parse_use_database("use neo4j"), Some("neo4j".to_string()));
    }

    #[test]
    fn parse_use_database_backtick_quoted() {
        assert_eq!(
            parse_use_database("USE `my-database`"),
            Some("my-database".to_string())
        );
    }

    #[test]
    fn parse_use_database_with_trailing_whitespace() {
        assert_eq!(
            parse_use_database("USE testdb  "),
            Some("testdb".to_string())
        );
    }

    #[test]
    fn parse_use_database_returns_none_for_non_use() {
        assert_eq!(parse_use_database("MATCH (n) RETURN n"), None);
    }

    #[test]
    fn parse_use_database_returns_none_for_empty_name() {
        assert_eq!(parse_use_database("USE "), None);
    }

    #[test]
    fn parse_use_database_with_hyphen() {
        assert_eq!(
            parse_use_database("USE test-db"),
            Some("test-db".to_string())
        );
    }
}
