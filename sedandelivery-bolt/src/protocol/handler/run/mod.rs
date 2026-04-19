//! RUN message handler and query pre-processing.
//!
//! Decomposed into focused submodules:
//!
//! - [`preprocess`]    — query string parsing helpers (CYPHER prefix,
//!   EXPLAIN/PROFILE detection, USE clause parsing)
//! - [`admin`]         — admin/DDL interception and database existence checks
//! - [`write_permit`]  — write-keyword detection and write-semaphore acquisition
//! - [`response`]      — SUCCESS response construction from converted results
//! - [`plan`]          — EXPLAIN handler (plan only, no execution)

use std::sync::Arc;

use crate::bridge;
use crate::message::types::ServerMessage;
use crate::metrics as bolt_metrics;
use crate::packstream::value::{BoltMap, BoltMapExt, BoltValue};
use crate::protocol::state::ServerState;
use cypher_core::storage::GraphStore;

use super::explain::operator_to_profile_plan_map;
use super::{
    bolt_map, extract_tx_metadata, recent_bookmark_age, validate_bookmarks, AdminStats,
    PendingQuery, ProtocolError, Session,
};

mod admin;
mod plan;
mod preprocess;
mod response;
mod write_permit;

use admin::{check_database_exists, intercept_admin_query};
use plan::handle_explain;
use preprocess::{
    detect_explain_profile, parse_use_database, parse_use_prefix, strip_cypher_prefix,
};
use response::{build_success_response, ConvertedResult};
use write_permit::acquire_write_semaphore;

#[cfg(test)]
mod tests;

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

// -- Store resolution --------------------------------------------------------

/// Resolve the effective store for query execution.
///
/// When inside an explicit transaction (`TxReady`/`TxStreaming`), returns the
/// snapshot store so that mutations are isolated until COMMIT. Otherwise
/// returns the main session store.
pub(super) fn apply_read_consistency(
    session: &Session,
    store: Arc<dyn GraphStore>,
) -> Arc<dyn GraphStore> {
    store
        .with_read_consistency(session.read_consistency())
        .unwrap_or(store)
}

pub(super) fn resolve_effective_store(session: &Session) -> Arc<dyn GraphStore> {
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
    if let Some(age) = recent_bookmark_age(&extra, &session.global_bookmark).await {
        tracing::debug!(
            age_ms = age.as_millis(),
            "rehydrated session freshness from recent bookmark"
        );
        session.apply_recent_bookmark_age(age);
    } else if extra.get_value("bookmarks").is_some() {
        tracing::debug!("received bookmarks but none mapped to recent freshness");
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
                write_backed_bookmark: false,
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
        .map(super::normalize_graph_name)
        .unwrap_or(&session.current_graph)
        .to_string();

    // Try to handle as an admin/DDL query.
    let effective_store = resolve_effective_store(session);
    if let Some(result) = intercept_admin_query(
        session,
        Arc::clone(&effective_store),
        &graph,
        trimmed,
        &extra,
    )
    .await
    {
        return result;
    }

    // Check that the target database exists (when explicitly specified).
    if let Some(failure) = check_database_exists(session, &extra, &graph).await? {
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
        let meta_map: indexmap::IndexMap<Box<str>, cypher_core::types::Value> = meta_value
            .into_iter()
            .map(|(k, v)| (Box::from(k.as_str()), v))
            .collect();
        value_params.insert(
            "__tx_metadata__".to_string(),
            cypher_core::types::Value::Map(meta_map),
        );
    }

    // Re-resolve effective store (state has not changed since admin check,
    // but we need a fresh Arc for the execution closures below).
    let effective_store = resolve_effective_store(session);

    if session.force_primary_read() {
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
            let (qr, profile_plan) = bridge::executor::execute_query_with_profiling(
                store_ref,
                graph_ref,
                &query_for_profile,
                params_for_profile,
                timeout_ms,
                |plan, profile_stats| {
                    let est = cypher_core::planner::collect_graph_statistics(store_ref, graph_ref)
                        .ok()
                        .map(|gs| cypher_core::planner::estimate_cardinality(plan, &gs));
                    operator_to_profile_plan_map(plan, 0, est.as_ref(), profile_stats)
                },
            )?;
            // Convert results to Bolt records inside the blocking context.
            // result_to_records calls store.get_node()/get_edge() which perform
            // synchronous Redis I/O — running on the async runtime would block
            // Tokio worker threads and starve connection acceptance under load.
            let (records, query_type, stats, trailing_err) =
                bridge::executor::result_to_records(&qr, store_ref, graph_ref);
            Ok((
                ConvertedResult {
                    columns: qr.columns,
                    records,
                    query_type,
                    stats,
                    trailing_err,
                },
                profile_plan,
            ))
        })
        .await
        .map_err(|e| ProtocolError::Internal(format!("blocking task join error: {}", e)))?;

        match result {
            Ok((cr, profile_plan)) => {
                build_success_response(session, cr, &graph, run_metadata, Some(profile_plan))
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
    } else {
        // Normal (non-PROFILE) execution.
        // Both query execution AND result-to-Bolt conversion run inside
        // spawn_blocking so that synchronous Redis I/O in value_to_bolt
        // (store.get_node / store.get_edge) never blocks Tokio worker
        // threads.  This is the fix for issue #953: under concurrent MERGE
        // load the previous code blocked all workers during result
        // conversion, starving new-connection handshakes ("early eof").
        let result = tokio::task::spawn_blocking(move || {
            let qr = bridge::executor::execute_query(
                &*store,
                &graph_clone,
                &query_clone,
                value_params,
                timeout_ms,
            )?;
            let (records, query_type, stats, trailing_err) =
                bridge::executor::result_to_records(&qr, &*store, &graph_clone);
            Ok(ConvertedResult {
                columns: qr.columns,
                records,
                query_type,
                stats,
                trailing_err,
            })
        })
        .await
        .map_err(|e| ProtocolError::Internal(format!("blocking task join error: {}", e)))?;

        match result {
            Ok(cr) => build_success_response(session, cr, &graph, run_metadata, None),
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
