//! RUN message handler and query plan helpers.

use std::sync::Arc;

use crate::bridge;
use crate::message::types::ServerMessage;
use crate::metrics as bolt_metrics;
use crate::packstream::value::{BoltMap, BoltMapExt, BoltValue};
use crate::protocol::state::ServerState;
use gql_core::storage::{GraphStore, InMemoryGraphStore};

use super::{
    bolt_map, extract_tx_metadata, validate_bookmarks, AdminStats, PendingQuery, ProtocolError,
    Session,
};

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

    // Try to handle as an admin/DDL query (CREATE/DROP DATABASE, SHOW DATABASES,
    // CREATE/DROP INDEX/CONSTRAINT). These commands are not supported by the
    // gql-core parser, so we intercept them here.
    let effective_store_for_admin: Arc<InMemoryGraphStore> = match &session.tx_store {
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
    if let Some(admin_result) =
        bridge::admin::try_handle_admin(&effective_store_for_admin, &graph, trimmed)
    {
        match admin_result {
            Ok(ar) => {
                let run_metadata = extract_tx_metadata(&extra);
                let combined_metadata = if !session.tx_metadata.is_empty() {
                    session.tx_metadata.clone()
                } else {
                    run_metadata
                };
                let (records, query_type, stats, _trailing) = bridge::executor::result_to_records(
                    &ar.query_result,
                    &effective_store_for_admin,
                    &graph,
                );
                let fields: Vec<BoltValue> = ar
                    .query_result
                    .columns
                    .iter()
                    .map(|c| BoltValue::String(c.clone()))
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
                        columns: ar.query_result.columns,
                        records,
                        query_type: effective_type,
                        stats,
                        cursor: 0,
                        start_time: std::time::Instant::now(),
                        metadata: combined_metadata,
                        explain_plan: None,
                        profile_plan: None,
                        graph: graph.clone(),
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
                        session.write_lock_guard = None;
                        ServerState::Streaming
                    }
                };

                return Ok(vec![ServerMessage::Success(bolt_map! {
                    "fields" => BoltValue::List(fields),
                    "t_first" => BoltValue::Int(0),
                    "qid" => BoltValue::Int(qid)
                })]);
            }
            Err(gql_err) => {
                let (code, message) = bridge::errors::gql_error_to_bolt(&gql_err);
                bolt_metrics::error_recorded(&code);
                session.write_lock_guard = None;
                session.state = ServerState::Failed;
                return Ok(vec![ServerMessage::Failure(bolt_map! {
                    "code" => BoltValue::String(code),
                    "message" => BoltValue::String(message)
                })]);
            }
        }
    }

    // For non-admin queries, check if the target database exists.
    // If the target database was explicitly specified (via the `db` parameter)
    // and doesn't exist, return DatabaseNotFound instead of auto-creating.
    // This check only applies in "multi-db mode" (when the default "neo4j"
    // database has been pre-created at server startup). In unit tests where
    // the store is empty, auto-create still works.
    if extra.get_str("db").is_some() && graph != "neo4j" && graph != "system" {
        let main_store_has_neo4j = session.store.graph_exists("neo4j").unwrap_or(false);
        if main_store_has_neo4j {
            let check_store: &InMemoryGraphStore = match &session.tx_store {
                Some(ts)
                    if matches!(
                        session.state,
                        ServerState::TxReady | ServerState::TxStreaming
                    ) =>
                {
                    ts
                }
                _ => &session.store,
            };
            let exists = check_store.graph_exists(&graph).unwrap_or(false);
            if !exists {
                use gql_core::error::{GqlError, StorageError};
                let err = GqlError::Storage(StorageError::GraphNotFound {
                    name: graph.clone(),
                });
                let (code, message) = bridge::errors::gql_error_to_bolt(&err);
                bolt_metrics::error_recorded(&code);
                session.state = ServerState::Failed;
                return Ok(vec![ServerMessage::Failure(bolt_map! {
                    "code" => BoltValue::String(code),
                    "message" => BoltValue::String(message)
                })]);
            }
        }
    }

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

    // Detect write queries for transaction timeout contention.
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
                    bolt_metrics::error_recorded(
                        "Neo.TransientError.Transaction.LockClientStopped",
                    );
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
                    admin_stats: None,
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
            bolt_metrics::error_recorded(&code);
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

// -- EXPLAIN helper ----------------------------------------------------------

/// Handle an EXPLAIN query: plan only, no execution.
async fn handle_explain(
    session: &mut Session,
    effective_store: &Arc<InMemoryGraphStore>,
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
