//! COMMIT, ROLLBACK, RESET, nested-BEGIN, and full transaction lifecycle.

use super::super::*;
use super::{make_ready_session, run_and_pull};

// -- COMMIT bookmark tests ---------------------------------------------------

#[tokio::test]
async fn commit_returns_bookmark() {
    let mut session = make_ready_session();
    session.state = ServerState::TxReady;

    let result = transaction::handle_commit(&mut session).await.unwrap();
    assert_eq!(result.len(), 1);
    match &result[0] {
        ServerMessage::Success(meta) => {
            let bm = meta.get_str("bookmark").unwrap();
            assert!(bm.starts_with("neo4j:bookmark:v1:tx"));
        }
        other => panic!("expected Success, got {:?}", other),
    }
    assert_eq!(session.state, ServerState::Ready);
}

#[tokio::test]
async fn commit_clears_tx_metadata() {
    let mut session = make_ready_session();
    session.state = ServerState::TxReady;
    session.tx_metadata = vec![("key".to_string(), BoltValue::String("val".to_string()))];
    session.tx_timeout = Some(5000);

    transaction::handle_commit(&mut session).await.unwrap();
    assert!(session.tx_metadata.is_empty());
    assert!(session.tx_timeout.is_none());
}

// -- ROLLBACK tests ----------------------------------------------------------

#[test]
fn rollback_returns_success_without_bookmark() {
    let mut session = make_ready_session();
    session.state = ServerState::TxReady;

    let result = transaction::handle_rollback(&mut session).unwrap();
    assert_eq!(result.len(), 1);
    match &result[0] {
        ServerMessage::Success(meta) => {
            // Rolled-back transaction must NOT produce a bookmark.
            assert!(
                meta.get_str("bookmark").is_none(),
                "rollback should not return a bookmark"
            );
        }
        other => panic!("expected Success, got {:?}", other),
    }
    assert_eq!(session.state, ServerState::Ready);
}

#[test]
fn rollback_clears_tx_metadata() {
    let mut session = make_ready_session();
    session.state = ServerState::TxReady;
    session.tx_metadata = vec![("key".to_string(), BoltValue::String("val".to_string()))];
    session.tx_timeout = Some(5000);

    transaction::handle_rollback(&mut session).unwrap();
    assert!(session.tx_metadata.is_empty());
    assert!(session.tx_timeout.is_none());
}

// -- RESET tests -------------------------------------------------------------

#[test]
fn reset_clears_all_session_state() {
    let mut session = make_ready_session();
    session.state = ServerState::TxStreaming;
    session.tx_metadata = vec![("key".to_string(), BoltValue::String("val".to_string()))];
    session.tx_timeout = Some(5000);
    session.pending_results.insert(
        0,
        PendingQuery {
            records: vec![],
            query_type: "r".to_string(),
            stats: vec![],
            cursor: 0,
            start_time: std::time::Instant::now(),
            metadata: vec![],
            explain_plan: None,
            profile_plan: None,
            graph: "neo4j".to_string(),
            notifications: vec![],
            trailing_error: None,
            write_backed_bookmark: false,
            admin_stats: None,
        },
    );

    let result = transaction::handle_reset(&mut session).unwrap();
    assert_eq!(result.len(), 1);
    assert!(matches!(&result[0], ServerMessage::Success(_)));
    assert_eq!(session.state, ServerState::Ready);
    assert!(session.pending_results.is_empty());
    assert!(session.tx_metadata.is_empty());
    assert!(session.tx_timeout.is_none());
}

// -- Nested BEGIN rejection tests --------------------------------------------

#[tokio::test]
async fn nested_begin_returns_failure() {
    let mut session = make_ready_session();

    // First BEGIN succeeds.
    let result = handle_message(&mut session, ClientMessage::Begin(vec![]))
        .await
        .unwrap();
    assert!(matches!(&result[0], ServerMessage::Success(_)));
    assert_eq!(session.state, ServerState::TxReady);

    // Second BEGIN while already in TxReady should fail.
    let result = handle_message(&mut session, ClientMessage::Begin(vec![]))
        .await
        .unwrap();
    assert_eq!(result.len(), 1);
    match &result[0] {
        ServerMessage::Failure(meta) => {
            let code = meta.get_str("code").unwrap();
            assert_eq!(code, "Neo.ClientError.Request.Invalid");
            let msg = meta.get_str("message").unwrap();
            assert!(
                msg.contains("TxReady"),
                "error message should mention TxReady state, got: {}",
                msg
            );
        }
        other => panic!("expected Failure for nested BEGIN, got {:?}", other),
    }
    assert_eq!(session.state, ServerState::Failed);
}

#[tokio::test]
async fn nested_begin_in_tx_streaming_returns_failure() {
    let mut session = make_ready_session();

    // BEGIN, then RUN to move to TxStreaming.
    handle_message(&mut session, ClientMessage::Begin(vec![]))
        .await
        .unwrap();
    handle_message(
        &mut session,
        ClientMessage::Run {
            query: "RETURN 1 AS n".to_string(),
            params: vec![],
            extra: vec![],
        },
    )
    .await
    .unwrap();
    assert_eq!(session.state, ServerState::TxStreaming);

    // BEGIN while in TxStreaming should fail.
    let result = handle_message(&mut session, ClientMessage::Begin(vec![]))
        .await
        .unwrap();
    assert_eq!(result.len(), 1);
    match &result[0] {
        ServerMessage::Failure(meta) => {
            let code = meta.get_str("code").unwrap();
            assert_eq!(code, "Neo.ClientError.Request.Invalid");
        }
        other => panic!("expected Failure for nested BEGIN, got {:?}", other),
    }
    assert_eq!(session.state, ServerState::Failed);
}

// -- Full transaction lifecycle test -----------------------------------------

#[tokio::test]
async fn full_transaction_lifecycle() {
    let mut session = make_ready_session();

    // 1. BEGIN
    let begin_result = handle_message(&mut session, ClientMessage::Begin(vec![]))
        .await
        .unwrap();
    assert!(matches!(&begin_result[0], ServerMessage::Success(_)));
    assert_eq!(session.state, ServerState::TxReady);

    // 2. RUN inside TX
    let run_result = handle_message(
        &mut session,
        ClientMessage::Run {
            query: "CREATE (:LifecycleNode {step: 1})".to_string(),
            params: vec![],
            extra: vec![],
        },
    )
    .await
    .unwrap();
    assert!(matches!(&run_result[0], ServerMessage::Success(_)));
    assert_eq!(session.state, ServerState::TxStreaming);

    // 3. PULL
    let pull_result = handle_message(
        &mut session,
        ClientMessage::Pull(bolt_map! { "n" => BoltValue::Int(-1) }),
    )
    .await
    .unwrap();
    let last = pull_result.last().unwrap();
    assert!(matches!(last, ServerMessage::Success(_)));
    assert_eq!(session.state, ServerState::TxReady);

    // 4. COMMIT
    let commit_result = handle_message(&mut session, ClientMessage::Commit)
        .await
        .unwrap();
    assert!(matches!(&commit_result[0], ServerMessage::Success(_)));
    assert_eq!(session.state, ServerState::Ready);

    // 5. Verify the committed data is visible
    let pull_msgs =
        run_and_pull(&mut session, "MATCH (n:LifecycleNode) RETURN count(n) AS c").await;
    let record = pull_msgs
        .iter()
        .find(|m| matches!(m, ServerMessage::Record(_)));
    if let ServerMessage::Record(fields) = record.unwrap() {
        assert_eq!(fields[0], BoltValue::Int(1));
    }
}
