//! Per-connection session state for a single Bolt client.

use std::collections::HashMap;
use std::sync::Arc;

use crate::packstream::value::{BoltMap, BoltValue};
use crate::protocol::state::ServerState;
use gql_core::storage::GraphStore;

use super::ProtocolError;

// -- Session types -----------------------------------------------------------

/// Per-connection session state for a single Bolt client.
///
/// Tracks the protocol state machine, the backing graph store, pending
/// query results, and transaction metadata.
pub struct Session {
    /// Current protocol state (Authentication, Ready, Streaming, etc.).
    pub(crate) state: ServerState,
    /// Shared graph store.
    pub(crate) store: Arc<dyn GraphStore>,
    /// Unique connection identifier for logging.
    pub(crate) conn_id: u64,
    /// The current default graph (database) for this session.
    pub(crate) current_graph: String,
    /// Multiple concurrent result streams keyed by query ID (qid).
    pub(crate) pending_results: HashMap<i64, PendingQuery>,
    /// Next query ID to assign.
    pub(crate) next_qid: i64,
    /// Global bookmark counter shared across all sessions connected to the same
    /// store. Bookmarks produced by one session are therefore meaningful when
    /// passed to another session, which is required by the Neo4j driver testkit.
    pub(crate) global_bookmark: Arc<crate::bookmark::BookmarkCounter>,
    /// TX metadata set via BEGIN extra map, echoed back on PULL completion.
    pub(crate) tx_metadata: BoltMap,
    /// TX-level timeout set via BEGIN extra map (milliseconds).
    pub(crate) tx_timeout: Option<u64>,
    /// Snapshot store for explicit transactions. When set, RUN executes
    /// against this store instead of the main store. COMMIT merges it back,
    /// ROLLBACK discards it.
    pub(crate) tx_store: Option<Arc<dyn GraphStore>>,
    /// Write concurrency semaphore for transaction timeout contention.
    /// Autocommit writes without tx_timeout bypass this (full concurrency).
    pub(crate) write_semaphore: Arc<tokio::sync::Semaphore>,
    /// Permit held while this session holds a write slot (during a write TX).
    pub(crate) write_permit: Option<tokio::sync::OwnedSemaphorePermit>,
    /// Saved `current_graph` from before BEGIN overrode it, so that ROLLBACK
    /// and RESET can restore the original default graph.
    pub(crate) saved_graph: Option<String>,
    /// Peer IP address used for auth rate limiting.
    pub(crate) peer_ip: Option<std::net::IpAddr>,
    /// Timestamp of the last write query in this session. Used for
    /// read-after-write consistency: reads within 5 seconds of a write
    /// are routed to the primary instead of replicas.
    pub(crate) last_write_at: Option<std::time::Instant>,
}

/// A pending query result awaiting PULL or DISCARD.
pub struct PendingQuery {
    /// Result rows, each row being a list of `BoltValue` in column order.
    pub(crate) records: Vec<Vec<BoltValue>>,
    /// Query type string: `"r"` (read), `"w"` (write), or `"rw"` (both).
    pub(crate) query_type: String,
    /// Non-zero mutation statistics.
    pub(crate) stats: BoltMap,
    /// Current cursor position into `records`.
    pub(crate) cursor: usize,
    /// When the query started executing (for `t_last` computation).
    pub(crate) start_time: std::time::Instant,
    /// TX metadata from RUN extra map, echoed back on PULL completion.
    pub(crate) metadata: BoltMap,
    /// EXPLAIN plan to return in PULL SUCCESS (no records emitted).
    pub(crate) explain_plan: Option<BoltValue>,
    /// PROFILE plan to return in PULL SUCCESS (records still emitted).
    pub(crate) profile_plan: Option<BoltValue>,
    /// The effective graph (database) for this query, used in the `db`
    /// metadata field of PULL SUCCESS.
    pub(crate) graph: String,
    /// Notifications (e.g. cartesian product warnings) to include in PULL
    /// SUCCESS metadata.
    pub(crate) notifications: Vec<BoltValue>,
    /// A per-row error that terminated streaming evaluation. When set,
    /// handle_pull will emit all buffered records and then send FAILURE
    /// with this error rather than SUCCESS.
    pub(crate) trailing_error: Option<(String, String)>,
    /// Extended stats for admin/DDL operations (indexes, constraints, system updates).
    /// These are merged into the stats map when emitting PULL SUCCESS.
    pub(crate) admin_stats: Option<AdminStats>,
}

/// Extended statistics for admin/DDL operations that go beyond standard
/// MutationStats.
#[derive(Debug, Clone, Default)]
pub struct AdminStats {
    pub(crate) indexes_added: u64,
    pub(crate) indexes_removed: u64,
    pub(crate) constraints_added: u64,
    pub(crate) constraints_removed: u64,
    pub(crate) system_updates: u64,
}

/// Duration after a write during which reads should be routed to the primary
/// for read-after-write consistency.
const READ_AFTER_WRITE_WINDOW: std::time::Duration = std::time::Duration::from_secs(5);

impl Session {
    /// Returns `true` if reads should be forced to the primary (not replicas)
    /// because a write occurred recently on this session.
    pub(crate) fn force_primary_read(&self) -> bool {
        match self.last_write_at {
            Some(t) => t.elapsed() < READ_AFTER_WRITE_WINDOW,
            None => false,
        }
    }

    /// Allocate the next query ID, returning an error if the counter overflows.
    pub(crate) fn allocate_qid(&mut self) -> Result<i64, ProtocolError> {
        let qid = self.next_qid;
        self.next_qid = self
            .next_qid
            .checked_add(1)
            .ok_or_else(|| ProtocolError::Internal("query ID counter overflow".to_string()))?;
        Ok(qid)
    }
}

// -- Constant-time comparison ------------------------------------------------

/// Constant-time byte comparison to prevent timing side-channel attacks on
/// credential validation.  Always compares all bytes regardless of mismatch
/// position, preventing character-by-character brute-force via response timing.
///
/// When lengths differ, the shorter slice is padded with zeros so that
/// comparison time is always proportional to the longer input. This avoids
/// leaking password length through timing.
pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let max_len = a.len().max(b.len());
    let mut diff = if a.len() != b.len() { 1u8 } else { 0u8 };
    for i in 0..max_len {
        let x = if i < a.len() { a[i] } else { 0 };
        let y = if i < b.len() { b[i] } else { 0 };
        diff |= x ^ y;
    }
    diff == 0
}
