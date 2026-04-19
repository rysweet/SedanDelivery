//! EXPLAIN handler: plan-only query handling, no execution.

use std::sync::Arc;

use crate::bridge;
use crate::message::types::ServerMessage;
use crate::metrics as bolt_metrics;
use crate::packstream::value::{BoltMap, BoltValue};
use crate::protocol::state::ServerState;
use cypher_core::storage::GraphStore;

use super::super::explain::{detect_notifications, operator_to_plan_map};
use super::super::{bolt_map, PendingQuery, ProtocolError, Session};

/// Handle an EXPLAIN query: plan only, no execution.
pub(super) async fn handle_explain(
    session: &mut Session,
    effective_store: &Arc<dyn GraphStore>,
    graph: &str,
    actual_query: &str,
    value_params: std::collections::HashMap<String, cypher_core::types::Value>,
    run_metadata: BoltMap,
) -> Result<Vec<ServerMessage>, ProtocolError> {
    let store = Arc::clone(effective_store);
    let g = graph.to_string();
    let q = actual_query.to_string();
    let p = value_params;
    // Both planning and statistics collection run inside spawn_blocking
    // because collect_graph_statistics calls the store (synchronous Redis
    // I/O) and must not block Tokio worker threads.
    let plan_result = tokio::task::spawn_blocking(move || {
        let operator = bridge::executor::plan_query(&*store, &g, &q, p)?;
        let estimates = cypher_core::planner::collect_graph_statistics(&*store, &g)
            .ok()
            .map(|gs| cypher_core::planner::estimate_cardinality(&operator, &gs));
        let plan_map = operator_to_plan_map(&operator, false, 0, estimates.as_ref());
        let plan_notifications = detect_notifications(&operator);
        Ok((plan_map, plan_notifications))
    })
    .await
    .map_err(|e| ProtocolError::Internal(e.to_string()))?;

    match plan_result {
        Ok((plan_map, plan_notifications)) => {
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
                    write_backed_bookmark: false,
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
