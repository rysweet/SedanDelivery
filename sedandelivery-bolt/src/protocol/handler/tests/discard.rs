//! DISCARD message handling, including admin write bookmark preservation
//! and trailing-error parity.

use super::super::*;
use super::{make_ready_session, make_shared_sessions};

#[tokio::test]
async fn discard_preserves_admin_write_bookmark_for_fresh_session_rehydration() {
    let (mut s1, mut s2) = make_shared_sessions();

    run::handle_run(
        &mut s1,
        "CREATE INDEX FOR (n:DiscardBookmarkIndexNode) ON (n.key)".to_string(),
        vec![],
        vec![],
    )
    .await
    .unwrap();

    let discard_messages = pull::handle_discard(&mut s1, vec![]).await.unwrap();
    let bookmark = match &discard_messages[0] {
        ServerMessage::Success(meta) => {
            let stats = meta
                .get_value("stats")
                .expect("discard summary should include stats");
            assert!(
                matches!(stats, BoltValue::Map(entries) if entries.iter().any(|(key, value)| {
                    key == "indexes-added" && matches!(value, BoltValue::Int(1))
                })),
                "discard summary should carry admin write stats"
            );
            meta.get_str("bookmark")
                .expect("discard summary should include bookmark")
                .to_string()
        }
        other => panic!("expected discard success, got {:?}", other),
    };

    run::handle_run(
        &mut s2,
        "RETURN 1 AS n".to_string(),
        vec![],
        vec![(
            "bookmarks".to_string(),
            BoltValue::List(vec![BoltValue::String(bookmark)]),
        )],
    )
    .await
    .unwrap();

    assert_eq!(
        s2.read_consistency(),
        cypher_core::storage::ReadConsistency::SessionFreshPrimary
    );
}

#[tokio::test]
async fn create_database_bookmark_rehydrates_primary_pin_on_fresh_session() {
    let (mut s1, mut s2) = make_shared_sessions();

    run::handle_run(
        &mut s1,
        "CREATE DATABASE FreshBookmarkGraph".to_string(),
        vec![],
        vec![],
    )
    .await
    .unwrap();

    let summary = pull::handle_discard(&mut s1, vec![]).await.unwrap();
    let bookmark = match &summary[0] {
        ServerMessage::Success(meta) => meta
            .get_str("bookmark")
            .expect("CREATE DATABASE should emit bookmark")
            .to_string(),
        other => panic!("expected success, got {:?}", other),
    };

    run::handle_run(
        &mut s2,
        "RETURN 1 AS n".to_string(),
        vec![],
        vec![(
            "bookmarks".to_string(),
            BoltValue::List(vec![BoltValue::String(bookmark)]),
        )],
    )
    .await
    .unwrap();

    assert_eq!(
        s2.read_consistency(),
        cypher_core::storage::ReadConsistency::SessionFreshPrimary
    );
}

#[tokio::test]
async fn standalone_use_bookmark_does_not_rehydrate_primary_pin() {
    let (mut s1, mut s2) = make_shared_sessions();

    run::handle_run(&mut s1, "USE othergraph".to_string(), vec![], vec![])
        .await
        .unwrap();

    let summary = pull::handle_discard(&mut s1, vec![]).await.unwrap();
    let bookmark = match &summary[0] {
        ServerMessage::Success(meta) => meta
            .get_str("bookmark")
            .expect("USE should emit bookmark")
            .to_string(),
        other => panic!("expected success, got {:?}", other),
    };

    run::handle_run(
        &mut s2,
        "RETURN 1 AS n".to_string(),
        vec![],
        vec![(
            "bookmarks".to_string(),
            BoltValue::List(vec![BoltValue::String(bookmark)]),
        )],
    )
    .await
    .unwrap();

    assert_eq!(
        s2.read_consistency(),
        cypher_core::storage::ReadConsistency::TopologyDefault
    );
}

#[tokio::test]
async fn discard_with_missing_explicit_qid_returns_client_failure() {
    let mut session = make_ready_session();

    let messages = pull::handle_discard(&mut session, bolt_map! { "qid" => BoltValue::Int(99) })
        .await
        .unwrap();

    assert_eq!(session.state, ServerState::Failed);
    assert_eq!(messages.len(), 1);
    match &messages[0] {
        ServerMessage::Failure(meta) => {
            assert_eq!(
                meta.get_str("code"),
                Some("Neo.ClientError.Statement.SyntaxError")
            );
            assert_eq!(meta.get_str("message"), Some("No result to discard"));
        }
        other => panic!("expected failure, got {:?}", other),
    }
}

#[tokio::test]
async fn discard_preserves_trailing_error_failure_parity() {
    let mut session = make_ready_session();
    session.state = ServerState::Streaming;
    session.pending_results.insert(
        7,
        PendingQuery {
            records: vec![vec![BoltValue::Int(1)]],
            query_type: "r".to_string(),
            stats: vec![],
            cursor: 0,
            start_time: std::time::Instant::now(),
            metadata: vec![],
            explain_plan: None,
            profile_plan: None,
            graph: "neo4j".to_string(),
            notifications: vec![],
            trailing_error: Some((
                "Neo.TransientError.General.DatabaseUnavailable".to_string(),
                "replica vanished".to_string(),
            )),
            write_backed_bookmark: false,
            admin_stats: None,
        },
    );

    let messages = pull::handle_discard(&mut session, bolt_map! { "qid" => BoltValue::Int(7) })
        .await
        .unwrap();

    assert_eq!(session.state, ServerState::Failed);
    assert!(session.pending_results.is_empty());
    assert_eq!(messages.len(), 1);
    match &messages[0] {
        ServerMessage::Failure(meta) => {
            assert_eq!(
                meta.get_str("code"),
                Some("Neo.TransientError.General.DatabaseUnavailable")
            );
            assert_eq!(meta.get_str("message"), Some("replica vanished"));
        }
        other => panic!("expected failure, got {:?}", other),
    }
}
