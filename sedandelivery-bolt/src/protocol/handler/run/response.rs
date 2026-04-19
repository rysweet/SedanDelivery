//! Successful-query response construction.
//!
//! Converts pre-converted Bolt records into a `PendingQuery` entry on
//! the session and emits the SUCCESS reply. All synchronous store I/O
//! must happen in the caller's `spawn_blocking` closure; this module
//! only touches session state and message construction.

use crate::bridge;
use crate::message::types::ServerMessage;
use crate::packstream::value::{BoltMap, BoltValue};
use crate::protocol::state::ServerState;

use super::super::{bolt_map, PendingQuery, ProtocolError, Session};

/// Pre-converted query result data produced inside `spawn_blocking`.
///
/// Grouping the fields into a struct avoids unwieldy tuple returns from
/// the blocking closure and makes the call sites self-documenting.
pub(super) struct ConvertedResult {
    pub(super) columns: Vec<String>,
    pub(super) records: Vec<Vec<BoltValue>>,
    pub(super) query_type: String,
    pub(super) stats: BoltMap,
    pub(super) trailing_err: Option<cypher_core::error::GqlError>,
}

/// Build the SUCCESS response from pre-converted query results.
///
/// The caller is responsible for running `result_to_records` inside
/// `spawn_blocking` (where synchronous store I/O is safe) and passing
/// the converted data here.  This function only touches session state
/// and Bolt message construction — no blocking I/O.
pub(super) fn build_success_response(
    session: &mut Session,
    cr: ConvertedResult,
    graph: &str,
    run_metadata: BoltMap,
    profile_plan: Option<BoltValue>,
) -> Result<Vec<ServerMessage>, ProtocolError> {
    let ConvertedResult {
        columns,
        records,
        query_type,
        stats,
        trailing_err,
    } = cr;
    let write_backed_bookmark = stats.iter().any(|(k, _)| {
        matches!(
            k.as_str(),
            "nodes-created"
                | "nodes-deleted"
                | "relationships-created"
                | "relationships-deleted"
                | "properties-set"
                | "labels-added"
                | "labels-removed"
                | "indexes-added"
                | "indexes-removed"
                | "constraints-added"
                | "constraints-removed"
        )
    });

    // Convert the trailing GqlError (if any) to a (code, message) pair
    // for deferred FAILURE emission during PULL.
    let trailing_error = trailing_err.map(|e| bridge::errors::gql_error_to_bolt(&e));

    let fields: Vec<BoltValue> = columns
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
            write_backed_bookmark,
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
