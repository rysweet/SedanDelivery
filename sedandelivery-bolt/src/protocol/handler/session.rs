//! Per-connection session state for a single Bolt client.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::packstream::value::{BoltMap, BoltValue};
use crate::protocol::state::ServerState;
use gql_core::storage::{GraphStore, ReadConsistency};

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
    /// Tracks whether the active explicit transaction executed any successful
    /// write statements so COMMIT can refresh the read-after-write window when
    /// those writes become durable.
    pub(crate) tx_had_write: bool,
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
    /// Whether the completion bookmark should count as evidence of a durable
    /// write for fresh-session read-after-write rehydration.
    pub(crate) write_backed_bookmark: bool,
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

pub(crate) const READ_AFTER_WRITE_WINDOW: Duration = Duration::from_secs(5);

impl Session {
    /// Allocate the next query ID, returning an error if the counter overflows.
    pub(crate) fn allocate_qid(&mut self) -> Result<i64, ProtocolError> {
        let qid = self.next_qid;
        self.next_qid = self
            .next_qid
            .checked_add(1)
            .ok_or_else(|| ProtocolError::Internal("query ID counter overflow".to_string()))?;
        Ok(qid)
    }

    pub(crate) fn read_consistency(&self) -> ReadConsistency {
        match self.last_write_at {
            Some(last_write_at) if last_write_at.elapsed() < READ_AFTER_WRITE_WINDOW => {
                ReadConsistency::SessionFreshPrimary
            }
            _ => ReadConsistency::TopologyDefault,
        }
    }

    pub(crate) fn apply_recent_bookmark_age(&mut self, age: Duration) {
        if age >= READ_AFTER_WRITE_WINDOW {
            return;
        }

        let candidate = std::time::Instant::now() - age;
        match self.last_write_at {
            Some(existing) if existing >= candidate => {}
            _ => self.last_write_at = Some(candidate),
        }
    }

    pub(crate) fn record_successful_write(&mut self) {
        let in_explicit_tx = matches!(self.state, ServerState::TxReady | ServerState::TxStreaming);
        if in_explicit_tx {
            self.tx_had_write = true;

            // Backends without a transaction snapshot make writes visible as
            // each RUN executes, so same-session reads inside the transaction
            // still need the primary pin immediately.
            if self.tx_store.is_none() {
                self.last_write_at = Some(std::time::Instant::now());
            }
            return;
        }

        self.last_write_at = Some(std::time::Instant::now());
    }

    pub(crate) fn finish_explicit_transaction(&mut self, committed: bool) {
        if committed && self.tx_had_write {
            self.last_write_at = Some(std::time::Instant::now());
        }
        self.tx_had_write = false;
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

#[cfg(test)]
mod tests {
    use super::*;

    use std::net::IpAddr;
    use std::sync::atomic::AtomicU64;

    use gql_core::storage::InMemoryGraphStore;

    fn ready_session() -> Session {
        Session {
            state: ServerState::Ready,
            store: Arc::new(InMemoryGraphStore::new()),
            conn_id: 1,
            current_graph: "neo4j".to_string(),
            pending_results: HashMap::new(),
            next_qid: 0,
            global_bookmark: Arc::new(crate::bookmark::BookmarkCounter::InMemory(AtomicU64::new(
                0,
            ))),
            tx_metadata: Vec::new(),
            tx_timeout: None,
            tx_store: None,
            write_semaphore: Arc::new(tokio::sync::Semaphore::new(128)),
            write_permit: None,
            saved_graph: None,
            tx_had_write: false,
            peer_ip: Option::<IpAddr>::None,
            last_write_at: None,
        }
    }

    #[test]
    fn read_consistency_defaults_without_recent_write() {
        let session = ready_session();
        assert_eq!(session.read_consistency(), ReadConsistency::TopologyDefault);
    }

    #[test]
    fn read_consistency_pins_primary_within_window() {
        let mut session = ready_session();
        session.last_write_at = Some(std::time::Instant::now() - Duration::from_secs(1));

        assert_eq!(
            session.read_consistency(),
            ReadConsistency::SessionFreshPrimary
        );
    }

    #[test]
    fn read_consistency_returns_to_topology_default_after_window() {
        let mut session = ready_session();
        session.last_write_at =
            Some(std::time::Instant::now() - READ_AFTER_WRITE_WINDOW - Duration::from_millis(1));

        assert_eq!(session.read_consistency(), ReadConsistency::TopologyDefault);
    }

    #[test]
    fn record_successful_write_marks_snapshot_transactions_for_commit_refresh() {
        let mut session = ready_session();
        session.state = ServerState::TxReady;
        session.tx_store = Some(Arc::new(InMemoryGraphStore::new()));

        session.record_successful_write();

        assert!(session.tx_had_write);
        assert_eq!(session.last_write_at, None);
    }

    #[test]
    fn record_successful_write_pins_immediately_when_tx_writes_are_visible() {
        let mut session = ready_session();
        session.state = ServerState::TxReady;

        session.record_successful_write();

        assert!(session.tx_had_write);
        assert_eq!(
            session.read_consistency(),
            ReadConsistency::SessionFreshPrimary
        );
    }

    #[test]
    fn finish_explicit_transaction_refreshes_recent_pin_after_commit() {
        let mut session = ready_session();
        session.tx_had_write = true;
        session.last_write_at =
            Some(std::time::Instant::now() - READ_AFTER_WRITE_WINDOW - Duration::from_secs(1));

        session.finish_explicit_transaction(true);

        assert!(!session.tx_had_write);
        assert_eq!(
            session.read_consistency(),
            ReadConsistency::SessionFreshPrimary
        );
    }
}
