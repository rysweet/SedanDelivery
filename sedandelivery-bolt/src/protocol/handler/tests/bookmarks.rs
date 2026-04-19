//! Cross-session bookmark propagation and BEGIN/RUN bookmark validation.

use super::super::*;
use super::{bookmark_tx_id, make_ready_session, make_shared_sessions, Arc};

// -- Cross-session bookmark tests --------------------------------------------

#[tokio::test]
async fn cross_session_read_only_bookmark_is_accepted_by_begin_without_primary_pin() {
    let (mut s1, mut s2) = make_shared_sessions();
    let isolated_bookmark = Arc::new(crate::bookmark::BookmarkCounter::InMemory(
        std::sync::atomic::AtomicU64::new(9_000_000_000_000),
    ));
    s1.global_bookmark = Arc::clone(&isolated_bookmark);
    s2.global_bookmark = isolated_bookmark;

    // Session 1: run a read-only query and pull to get a bookmark.
    run::handle_run(&mut s1, "RETURN 1 AS n".to_string(), vec![], vec![])
        .await
        .unwrap();
    let pull_result = pull::handle_pull(&mut s1, bolt_map! { "n" => BoltValue::Int(-1) })
        .await
        .unwrap();
    let bookmark = match pull_result.last().unwrap() {
        ServerMessage::Success(meta) => meta.get_str("bookmark").unwrap().to_string(),
        other => panic!("expected Success, got {:?}", other),
    };
    assert!(bookmark.starts_with("neo4j:bookmark:v1:tx"));

    // Session 2: pass that bookmark into BEGIN -- should succeed.
    let begin_result = transaction::handle_begin(
        &mut s2,
        bolt_map! {
            "bookmarks" => BoltValue::List(vec![BoltValue::String(bookmark)])
        },
    )
    .await
    .unwrap();
    assert_eq!(begin_result.len(), 1);
    assert!(matches!(&begin_result[0], ServerMessage::Success(_)));
    assert_eq!(s2.state, ServerState::TxReady);
    assert_eq!(
        s2.read_consistency(),
        cypher_core::storage::ReadConsistency::TopologyDefault
    );
}

#[tokio::test]
async fn cross_session_bookmark_counter_is_shared() {
    let (mut s1, mut s2) = make_shared_sessions();

    // Session 1: run + pull -> bookmark tx1.
    run::handle_run(&mut s1, "RETURN 1 AS n".to_string(), vec![], vec![])
        .await
        .unwrap();
    let r1 = pull::handle_pull(&mut s1, bolt_map! { "n" => BoltValue::Int(-1) })
        .await
        .unwrap();
    let bm1 = match r1.last().unwrap() {
        ServerMessage::Success(m) => m.get_str("bookmark").unwrap().to_string(),
        other => panic!("expected Success, got {:?}", other),
    };

    // Session 2: run + pull -> bookmark tx2 (not tx1 again).
    run::handle_run(&mut s2, "RETURN 2 AS n".to_string(), vec![], vec![])
        .await
        .unwrap();
    let r2 = pull::handle_pull(&mut s2, bolt_map! { "n" => BoltValue::Int(-1) })
        .await
        .unwrap();
    let bm2 = match r2.last().unwrap() {
        ServerMessage::Success(m) => m.get_str("bookmark").unwrap().to_string(),
        other => panic!("expected Success, got {:?}", other),
    };

    assert!(bm1.starts_with("neo4j:bookmark:v1:tx"));
    assert!(bm2.starts_with("neo4j:bookmark:v1:tx"));
    assert_eq!(bookmark_tx_id(&bm2), bookmark_tx_id(&bm1) + 1);
}

// -- BEGIN bookmark tests ----------------------------------------------------

#[tokio::test]
async fn begin_with_invalid_bookmark_returns_failure() {
    let mut session = make_ready_session();
    let extra: BoltMap = vec![(
        "bookmarks".to_string(),
        BoltValue::List(vec![BoltValue::String("invalid_bookmark".to_string())]),
    )];

    let result = transaction::handle_begin(&mut session, extra)
        .await
        .unwrap();
    assert_eq!(result.len(), 1);
    match &result[0] {
        ServerMessage::Failure(meta) => {
            let code = meta.get_str("code").unwrap();
            assert_eq!(code, "Neo.ClientError.Transaction.InvalidBookmark");
        }
        other => panic!("expected Failure, got {:?}", other),
    }
    assert_eq!(session.state, ServerState::Failed);
}

#[tokio::test]
async fn begin_with_valid_sedan_bookmark_succeeds() {
    let mut session = make_ready_session();
    let extra: BoltMap = vec![(
        "bookmarks".to_string(),
        BoltValue::List(vec![BoltValue::String("sedan:tx:1".to_string())]),
    )];

    let result = transaction::handle_begin(&mut session, extra)
        .await
        .unwrap();
    assert_eq!(result.len(), 1);
    assert!(matches!(&result[0], ServerMessage::Success(_)));
    assert_eq!(session.state, ServerState::TxReady);
}

#[tokio::test]
async fn begin_with_valid_neo4j_bookmark_succeeds() {
    let mut session = make_ready_session();
    let extra: BoltMap = vec![(
        "bookmarks".to_string(),
        BoltValue::List(vec![BoltValue::String(
            "neo4j:bookmark:v1:tx42".to_string(),
        )]),
    )];

    let result = transaction::handle_begin(&mut session, extra)
        .await
        .unwrap();
    assert_eq!(result.len(), 1);
    assert!(matches!(&result[0], ServerMessage::Success(_)));
    assert_eq!(session.state, ServerState::TxReady);
}

#[tokio::test]
async fn begin_with_no_bookmarks_succeeds() {
    let mut session = make_ready_session();
    let extra: BoltMap = vec![];

    let result = transaction::handle_begin(&mut session, extra)
        .await
        .unwrap();
    assert_eq!(result.len(), 1);
    assert!(matches!(&result[0], ServerMessage::Success(_)));
    assert_eq!(session.state, ServerState::TxReady);
}

#[tokio::test]
async fn begin_stores_tx_metadata() {
    let mut session = make_ready_session();
    let inner: BoltMap = vec![("app".to_string(), BoltValue::String("test".to_string()))];
    let extra: BoltMap = vec![("tx_metadata".to_string(), BoltValue::Map(inner.clone()))];

    transaction::handle_begin(&mut session, extra)
        .await
        .unwrap();
    assert_eq!(session.tx_metadata, inner);
}

// -- RUN bookmark tests ------------------------------------------------------

#[tokio::test]
async fn run_with_invalid_bookmark_returns_failure() {
    let mut session = make_ready_session();
    let extra: BoltMap = vec![(
        "bookmarks".to_string(),
        BoltValue::List(vec![BoltValue::String("invalid_bookmark".to_string())]),
    )];

    let result = run::handle_run(&mut session, "RETURN 1".to_string(), vec![], extra)
        .await
        .unwrap();

    assert_eq!(result.len(), 1);
    match &result[0] {
        ServerMessage::Failure(meta) => {
            let code = meta.get_str("code").unwrap();
            assert_eq!(code, "Neo.ClientError.Transaction.InvalidBookmark");
        }
        other => panic!("expected Failure, got {:?}", other),
    }
    assert_eq!(session.state, ServerState::Failed);
}

#[tokio::test]
async fn run_with_valid_bookmark_succeeds() {
    let mut session = make_ready_session();
    let extra: BoltMap = vec![(
        "bookmarks".to_string(),
        BoltValue::List(vec![BoltValue::String("sedan:tx:5".to_string())]),
    )];

    let result = run::handle_run(&mut session, "RETURN 1 AS n".to_string(), vec![], extra)
        .await
        .unwrap();

    assert_eq!(result.len(), 1);
    assert!(matches!(&result[0], ServerMessage::Success(_)));
}
