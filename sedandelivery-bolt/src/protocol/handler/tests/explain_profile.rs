//! EXPLAIN notification detection and PROFILE plan-with-row-counts tests.

use super::super::*;
use super::{make_ready_session, run_and_pull};

#[tokio::test]
async fn explain_cartesian_product_generates_notification() {
    let mut session = make_ready_session();
    run::handle_run(
        &mut session,
        "EXPLAIN MATCH (n), (m) RETURN n, m".to_string(),
        vec![],
        vec![],
    )
    .await
    .unwrap();
    let pull_result = pull::handle_pull(&mut session, bolt_map! { "n" => BoltValue::Int(-1) })
        .await
        .unwrap();
    let success = pull_result.last().unwrap();
    match success {
        ServerMessage::Success(meta) => {
            let notifications = meta.get_value("notifications");
            assert!(notifications.is_some(), "must have notifications field");
            if let Some(BoltValue::List(items)) = notifications {
                assert_eq!(items.len(), 1, "expected 1 notification");
                if let BoltValue::Map(entries) = &items[0] {
                    let code = entries.get_str("code").unwrap();
                    assert!(code.contains("CartesianProduct"));
                } else {
                    panic!("expected Map for notification");
                }
            } else {
                panic!("expected notifications to be a List");
            }
        }
        other => panic!("expected Success, got {:?}", other),
    }
}

#[tokio::test]
async fn profile_returns_results_and_plan_with_row_counts() {
    let mut session = make_ready_session();
    // Create some data first
    run_and_pull(&mut session, "CREATE (:Pnode {v: 1}), (:Pnode {v: 2})").await;

    // Run a PROFILE query
    let run_result = run::handle_run(
        &mut session,
        "PROFILE MATCH (n:Pnode) RETURN n".to_string(),
        vec![],
        vec![],
    )
    .await
    .unwrap();
    assert!(
        matches!(&run_result[0], ServerMessage::Success(_)),
        "PROFILE RUN should succeed, got {:?}",
        run_result
    );

    let pull_result = pull::handle_pull(&mut session, bolt_map! { "n" => BoltValue::Int(-1) })
        .await
        .unwrap();

    // Should have RECORD messages (the actual query results)
    let records: Vec<_> = pull_result
        .iter()
        .filter(|m| matches!(m, ServerMessage::Record(_)))
        .collect();
    assert_eq!(records.len(), 2, "PROFILE should return 2 result rows");

    // The final SUCCESS should contain a profile plan
    let success = pull_result.last().unwrap();
    match success {
        ServerMessage::Success(meta) => {
            let profile = meta.get_value("profile");
            assert!(
                profile.is_some(),
                "PULL SUCCESS must include 'profile' field"
            );
            if let Some(BoltValue::Map(plan_entries)) = profile {
                // Plan should have operatorType
                let op_type = plan_entries.get_str("operatorType");
                assert!(op_type.is_some(), "plan should have operatorType");

                // Plan should have rows field with actual count
                let rows = plan_entries.get_int("rows");
                assert!(rows.is_some(), "plan should have rows field");
                assert!(rows.unwrap() > 0, "root operator rows should be non-zero");

                // Plan should have dbHits field
                let db_hits = plan_entries.get_int("dbHits");
                assert!(db_hits.is_some(), "plan should have dbHits field");
            } else {
                panic!("expected profile to be a Map");
            }
        }
        other => panic!("expected Success, got {:?}", other),
    }
}

#[tokio::test]
async fn profile_simple_return_produces_plan() {
    let mut session = make_ready_session();
    let run_result = run::handle_run(
        &mut session,
        "PROFILE RETURN 1 AS x".to_string(),
        vec![],
        vec![],
    )
    .await
    .unwrap();
    assert!(matches!(&run_result[0], ServerMessage::Success(_)));

    let pull_result = pull::handle_pull(&mut session, bolt_map! { "n" => BoltValue::Int(-1) })
        .await
        .unwrap();

    // Should have 1 record
    let records: Vec<_> = pull_result
        .iter()
        .filter(|m| matches!(m, ServerMessage::Record(_)))
        .collect();
    assert_eq!(records.len(), 1);

    // Final SUCCESS should have profile
    let success = pull_result.last().unwrap();
    match success {
        ServerMessage::Success(meta) => {
            assert!(
                meta.get_value("profile").is_some(),
                "PROFILE RETURN 1 should produce a profile plan"
            );
        }
        other => panic!("expected Success, got {:?}", other),
    }
}
