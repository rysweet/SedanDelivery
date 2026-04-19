use super::*;

use std::net::IpAddr;
use std::sync::atomic::AtomicU64;

use cypher_core::storage::InMemoryGraphStore;

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

// -- normalize_graph_name tests (#697) ------------------------------------

#[test]
fn normalize_graph_name_returns_explicit_name_unchanged() {
    assert_eq!(normalize_graph_name("mydb"), "mydb");
}

#[test]
fn normalize_graph_name_returns_neo4j_unchanged() {
    assert_eq!(normalize_graph_name("neo4j"), "neo4j");
}

#[test]
fn normalize_graph_name_maps_empty_string_to_default() {
    assert_eq!(normalize_graph_name(""), DEFAULT_GRAPH);
}

#[test]
fn normalize_graph_name_strips_single_quotes() {
    assert_eq!(normalize_graph_name("'mydb'"), "mydb");
}

#[test]
fn normalize_graph_name_strips_double_quotes() {
    assert_eq!(normalize_graph_name("\"mydb\""), "mydb");
}

#[test]
fn normalize_graph_name_quoted_empty_maps_to_default() {
    // A driver could send db: "''" — after stripping quotes the name is empty.
    assert_eq!(normalize_graph_name("''"), DEFAULT_GRAPH);
    assert_eq!(normalize_graph_name("\"\""), DEFAULT_GRAPH);
}

#[test]
fn normalize_graph_name_preserves_custom_databases() {
    assert_eq!(normalize_graph_name("analytics"), "analytics");
    assert_eq!(normalize_graph_name("test-db"), "test-db");
}

// -- force_primary_read convenience predicate ----------------------------

#[test]
fn force_primary_read_false_without_prior_write() {
    let session = ready_session();
    assert!(!session.force_primary_read());
}

#[test]
fn force_primary_read_true_within_window() {
    let mut session = ready_session();
    session.last_write_at = Some(std::time::Instant::now() - Duration::from_secs(1));
    assert!(session.force_primary_read());
}

#[test]
fn force_primary_read_false_after_window_expires() {
    let mut session = ready_session();
    session.last_write_at =
        Some(std::time::Instant::now() - READ_AFTER_WRITE_WINDOW - Duration::from_millis(1));
    assert!(!session.force_primary_read());
}
