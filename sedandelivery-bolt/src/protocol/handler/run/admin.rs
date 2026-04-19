//! Admin/DDL query interception and database existence checks.
//!
//! Intercepts admin commands (CREATE/DROP DATABASE, SHOW DATABASES,
//! CREATE/DROP INDEX/CONSTRAINT) before normal query execution, and
//! validates that explicitly-named target databases exist.

use std::sync::Arc;

use crate::bridge;
use crate::message::types::ServerMessage;
use crate::metrics as bolt_metrics;
use crate::packstream::value::{BoltMap, BoltMapExt, BoltValue};
use crate::protocol::state::ServerState;
use cypher_core::storage::GraphStore;

use super::super::{
    bolt_map, extract_tx_metadata, AdminStats, PendingQuery, ProtocolError, Session,
};
use super::{apply_read_consistency, resolve_effective_store};

/// Pre-converted admin command result data.
struct AdminInterceptSuccess {
    columns: Vec<String>,
    records: Vec<Vec<BoltValue>>,
    query_type: String,
    stats: BoltMap,
    admin_stats: AdminStats,
    admin_wrote: bool,
}

/// Try to handle a query as an admin/DDL command (CREATE/DROP DATABASE,
/// SHOW DATABASES, CREATE/DROP INDEX/CONSTRAINT).
///
/// These commands are not supported by the cypher-core parser, so we intercept
/// them before normal query execution. Returns `Some(response)` if the query
/// was handled as admin, `None` if it should proceed through normal execution.
pub(super) async fn intercept_admin_query(
    session: &mut Session,
    store: Arc<dyn GraphStore>,
    graph: &str,
    trimmed: &str,
    extra: &BoltMap,
) -> Option<Result<Vec<ServerMessage>, ProtocolError>> {
    let graph_name = graph.to_string();
    let trimmed_query = trimmed.to_string();
    let admin_result = match tokio::task::spawn_blocking(move || {
        let admin_result = bridge::admin::try_handle_admin(&*store, &graph_name, &trimmed_query)?;
        Some(admin_result.map(|ar| {
            let (records, query_type, stats, _trailing) =
                bridge::executor::result_to_records(&ar.query_result, &*store, &graph_name);
            AdminInterceptSuccess {
                columns: ar.query_result.columns,
                records,
                query_type,
                stats,
                admin_stats: AdminStats {
                    indexes_added: ar.indexes_added,
                    indexes_removed: ar.indexes_removed,
                    constraints_added: ar.constraints_added,
                    constraints_removed: ar.constraints_removed,
                    system_updates: ar.system_updates,
                },
                admin_wrote: ar.system_updates > 0
                    || ar.indexes_added > 0
                    || ar.indexes_removed > 0
                    || ar.constraints_added > 0
                    || ar.constraints_removed > 0,
            }
        }))
    })
    .await
    {
        Ok(result) => result?,
        Err(e) => {
            return Some(Err(ProtocolError::Internal(format!(
                "blocking admin task join error: {e}"
            ))));
        }
    };

    Some(match admin_result {
        Ok(ar) => {
            let run_metadata = extract_tx_metadata(extra);
            let combined_metadata = if !session.tx_metadata.is_empty() {
                session.tx_metadata.clone()
            } else {
                run_metadata
            };
            let fields: Vec<BoltValue> = ar
                .columns
                .iter()
                .map(|c: &String| BoltValue::String(c.clone()))
                .collect();

            // Determine query type: admin commands are "s" (system) when
            // they have system_updates, otherwise use the computed type.
            let effective_type = if ar.admin_stats.system_updates > 0 {
                "s".to_string()
            } else {
                ar.query_type
            };

            let qid = match session.allocate_qid() {
                Ok(q) => q,
                Err(e) => return Some(Err(e)),
            };

            // Track writes for read-after-write session pinning.
            if effective_type.contains('w') || ar.admin_wrote {
                session.record_successful_write();
            }

            session.pending_results.insert(
                qid,
                PendingQuery {
                    records: ar.records,
                    query_type: effective_type,
                    stats: ar.stats,
                    cursor: 0,
                    start_time: std::time::Instant::now(),
                    metadata: combined_metadata,
                    explain_plan: None,
                    profile_plan: None,
                    graph: graph.to_string(),
                    notifications: vec![],
                    trailing_error: None,
                    write_backed_bookmark: ar.admin_wrote,
                    admin_stats: Some(ar.admin_stats),
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

/// Check whether the target database exists when explicitly specified.
///
/// Returns `Some(failure_messages)` if the database doesn't exist, `None` if
/// the check passes or doesn't apply.
pub(super) async fn check_database_exists(
    session: &mut Session,
    extra: &BoltMap,
    graph: &str,
) -> Result<Option<Vec<ServerMessage>>, ProtocolError> {
    if extra.get_str("db").is_none() || graph == "neo4j" || graph == "system" {
        return Ok(None);
    }

    let main_store = apply_read_consistency(session, Arc::clone(&session.store));
    let check_store = resolve_effective_store(session);
    let graph_name = graph.to_string();
    let (main_store_has_neo4j, exists) = tokio::task::spawn_blocking(move || {
        let main_store_has_neo4j = main_store.graph_exists("neo4j").unwrap_or(false);
        if !main_store_has_neo4j {
            return (false, false);
        }

        (true, check_store.graph_exists(&graph_name).unwrap_or(false))
    })
    .await
    .map_err(|e| ProtocolError::Internal(format!("blocking graph existence check failed: {e}")))?;

    if !main_store_has_neo4j {
        return Ok(None);
    }

    if exists {
        return Ok(None);
    }

    use cypher_core::error::{GqlError, StorageError};
    let err = GqlError::Storage(StorageError::GraphNotFound {
        name: graph.to_string(),
    });
    let (code, message) = bridge::errors::gql_error_to_bolt(&err);
    bolt_metrics::error_recorded(&code);
    session.state = ServerState::Failed;
    Ok(Some(vec![ServerMessage::Failure(bolt_map! {
        "code" => BoltValue::String(code),
        "message" => BoltValue::String(message)
    })]))
}
