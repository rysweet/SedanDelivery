//! Snapshot transaction isolation, read-your-writes, and read-after-write
//! consistency.

use super::super::*;
use super::{make_ready_session, make_shared_sessions, run_and_pull};

#[tokio::test]
async fn begin_creates_tx_store() {
    let mut session = make_ready_session();
    assert!(session.tx_store.is_none());

    transaction::handle_begin(&mut session, vec![])
        .await
        .unwrap();
    assert!(session.tx_store.is_some());
    assert_eq!(session.state, ServerState::TxReady);
}

#[tokio::test]
async fn commit_clears_tx_store() {
    let mut session = make_ready_session();
    transaction::handle_begin(&mut session, vec![])
        .await
        .unwrap();
    assert!(session.tx_store.is_some());

    transaction::handle_commit(&mut session).await.unwrap();
    assert!(session.tx_store.is_none());
}

#[tokio::test]
async fn rollback_clears_tx_store() {
    let mut session = make_ready_session();
    transaction::handle_begin(&mut session, vec![])
        .await
        .unwrap();
    assert!(session.tx_store.is_some());

    transaction::handle_rollback(&mut session).unwrap();
    assert!(session.tx_store.is_none());
}

#[tokio::test]
async fn reset_clears_tx_store() {
    let mut session = make_ready_session();
    transaction::handle_begin(&mut session, vec![])
        .await
        .unwrap();
    assert!(session.tx_store.is_some());

    transaction::handle_reset(&mut session).unwrap();
    assert!(session.tx_store.is_none());
}

#[tokio::test]
async fn tx_read_your_writes() {
    let mut session = make_ready_session();

    // BEGIN, CREATE inside the TX, then read it back within the same TX.
    transaction::handle_begin(&mut session, vec![])
        .await
        .unwrap();
    run_and_pull(&mut session, "CREATE (:TxNode {val: 42})").await;

    let pull_msgs = run_and_pull(&mut session, "MATCH (n:TxNode) RETURN count(n) AS c").await;

    // Should have 1 RECORD with c=1, then SUCCESS
    let record = pull_msgs
        .iter()
        .find(|m| matches!(m, ServerMessage::Record(_)));
    assert!(record.is_some(), "expected a record");
    if let ServerMessage::Record(fields) = record.unwrap() {
        assert_eq!(fields[0], BoltValue::Int(1));
    }
}

#[tokio::test]
async fn rollback_discards_mutations() {
    let (mut s1, mut s2) = make_shared_sessions();

    // Session 1: BEGIN, CREATE, ROLLBACK
    transaction::handle_begin(&mut s1, vec![]).await.unwrap();
    run_and_pull(&mut s1, "CREATE (:RollbackNode {val: 1})").await;
    transaction::handle_rollback(&mut s1).unwrap();

    // Session 2: the node should not be visible in the main store.
    let pull_msgs = run_and_pull(&mut s2, "MATCH (n:RollbackNode) RETURN count(n) AS c").await;
    let record = pull_msgs
        .iter()
        .find(|m| matches!(m, ServerMessage::Record(_)));
    assert!(record.is_some(), "expected a record");
    if let ServerMessage::Record(fields) = record.unwrap() {
        assert_eq!(
            fields[0],
            BoltValue::Int(0),
            "rollback should discard mutations"
        );
    }
}

#[tokio::test]
async fn commit_makes_mutations_visible() {
    let (mut s1, mut s2) = make_shared_sessions();

    // Session 1: BEGIN, CREATE, COMMIT
    transaction::handle_begin(&mut s1, vec![]).await.unwrap();
    run_and_pull(&mut s1, "CREATE (:CommitNode {val: 2})").await;
    transaction::handle_commit(&mut s1).await.unwrap();

    // Session 2: the node should be visible.
    let pull_msgs = run_and_pull(&mut s2, "MATCH (n:CommitNode) RETURN count(n) AS c").await;
    let record = pull_msgs
        .iter()
        .find(|m| matches!(m, ServerMessage::Record(_)));
    assert!(record.is_some(), "expected a record");
    if let ServerMessage::Record(fields) = record.unwrap() {
        assert_eq!(
            fields[0],
            BoltValue::Int(1),
            "commit should make mutations visible"
        );
    }
}

#[tokio::test]
async fn commit_refreshes_last_write_at_for_snapshot_transactions() {
    let mut session = make_ready_session();
    transaction::handle_begin(&mut session, vec![])
        .await
        .unwrap();
    session.last_write_at = Some(
        std::time::Instant::now()
            - crate::protocol::handler::session::READ_AFTER_WRITE_WINDOW
            - std::time::Duration::from_secs(1),
    );

    run_and_pull(&mut session, "CREATE (:CommitRefreshNode {val: 1})").await;
    assert_eq!(
        session.read_consistency(),
        cypher_core::storage::ReadConsistency::TopologyDefault
    );

    transaction::handle_commit(&mut session).await.unwrap();
    assert_eq!(
        session.read_consistency(),
        cypher_core::storage::ReadConsistency::SessionFreshPrimary
    );
}

#[tokio::test]
async fn create_index_marks_session_fresh_for_read_after_write() {
    let mut session = make_ready_session();

    run::handle_run(
        &mut session,
        "CREATE INDEX FOR (n:ReadAfterWriteIndexNode) ON (n.key)".to_string(),
        vec![],
        vec![],
    )
    .await
    .unwrap();

    assert_eq!(
        session.read_consistency(),
        cypher_core::storage::ReadConsistency::SessionFreshPrimary
    );
}

#[tokio::test]
async fn uncommitted_mutations_not_visible_to_other_sessions() {
    let (mut s1, mut s2) = make_shared_sessions();

    // Session 1: BEGIN, CREATE (but do NOT commit yet)
    transaction::handle_begin(&mut s1, vec![]).await.unwrap();
    run_and_pull(&mut s1, "CREATE (:IsolatedNode {val: 3})").await;

    // Session 2: the node should NOT be visible because s1 has not committed.
    let pull_msgs = run_and_pull(&mut s2, "MATCH (n:IsolatedNode) RETURN count(n) AS c").await;
    let record = pull_msgs
        .iter()
        .find(|m| matches!(m, ServerMessage::Record(_)));
    assert!(record.is_some(), "expected a record");
    if let ServerMessage::Record(fields) = record.unwrap() {
        assert_eq!(
            fields[0],
            BoltValue::Int(0),
            "uncommitted mutations should be invisible"
        );
    }
}

// -- RUN after ROLLBACK tests ------------------------------------------------

#[tokio::test]
async fn run_succeeds_after_rollback() {
    let mut session = make_ready_session();

    // BEGIN -> CREATE -> ROLLBACK
    transaction::handle_begin(&mut session, vec![])
        .await
        .unwrap();
    run_and_pull(&mut session, "CREATE (:Temp {val: 1})").await;
    transaction::handle_rollback(&mut session).unwrap();
    assert_eq!(session.state, ServerState::Ready);

    // RUN after ROLLBACK should work normally (auto-commit mode).
    let result = run::handle_run(
        &mut session,
        "RETURN 42 AS answer".to_string(),
        vec![],
        vec![],
    )
    .await
    .unwrap();
    assert!(
        matches!(&result[0], ServerMessage::Success(_)),
        "RUN after ROLLBACK should succeed, got {:?}",
        result
    );

    let pull_msgs = pull::handle_pull(&mut session, bolt_map! { "n" => BoltValue::Int(-1) })
        .await
        .unwrap();
    let record = pull_msgs
        .iter()
        .find(|m| matches!(m, ServerMessage::Record(_)));
    assert!(record.is_some(), "expected a record after ROLLBACK + RUN");
    if let ServerMessage::Record(fields) = record.unwrap() {
        assert_eq!(fields[0], BoltValue::Int(42));
    }
}

#[tokio::test]
async fn run_after_rollback_does_not_see_rolled_back_data() {
    let mut session = make_ready_session();

    // BEGIN -> CREATE -> ROLLBACK
    transaction::handle_begin(&mut session, vec![])
        .await
        .unwrap();
    run_and_pull(&mut session, "CREATE (:Ghost {val: 99})").await;
    transaction::handle_rollback(&mut session).unwrap();

    // Query for the rolled-back node in auto-commit mode -- should not exist.
    let pull_msgs = run_and_pull(&mut session, "MATCH (n:Ghost) RETURN count(n) AS c").await;
    let record = pull_msgs
        .iter()
        .find(|m| matches!(m, ServerMessage::Record(_)));
    assert!(record.is_some(), "expected a record");
    if let ServerMessage::Record(fields) = record.unwrap() {
        assert_eq!(
            fields[0],
            BoltValue::Int(0),
            "rolled-back data should not be visible after ROLLBACK"
        );
    }
}
