//! Cross-driver visibility tests (#697).
//!
//! Different Neo4j drivers send the `db` field with different conventions
//! ("neo4j", "", null, omitted). All of them must resolve to the same
//! internal graph namespace so a write from one driver is visible from
//! another.

use super::super::*;
use super::make_shared_sessions;

/// Simulates two Neo4j drivers connecting to the same Bolt endpoint:
/// - "Node.js driver" sends `db: "neo4j"` explicitly in RUN extra
/// - ".NET driver" sends `db: ""` (empty string) in RUN extra
///
/// Before the fix, these resolved to different internal graph namespaces,
/// so data written by one driver was invisible to the other.
#[tokio::test]
async fn cross_driver_explicit_neo4j_and_empty_db_see_same_data() {
    let (mut nodejs_session, mut dotnet_session) = make_shared_sessions();

    // Node.js driver: CREATE with explicit db: "neo4j"
    let run_result = run::handle_run(
        &mut nodejs_session,
        "CREATE (n:Driver {name: 'nodejs'}) RETURN n.name AS name".to_string(),
        vec![],
        bolt_map! { "db" => BoltValue::String("neo4j".to_string()) },
    )
    .await
    .unwrap();
    assert!(matches!(run_result[0], ServerMessage::Success(_)));
    pull::handle_pull(&mut nodejs_session, bolt_map! { "n" => BoltValue::Int(-1) })
        .await
        .unwrap();

    // .NET driver: MATCH with db: "" (empty string — the bug trigger)
    let run_result = run::handle_run(
        &mut dotnet_session,
        "MATCH (n:Driver) RETURN n.name AS name".to_string(),
        vec![],
        bolt_map! { "db" => BoltValue::String(String::new()) },
    )
    .await
    .unwrap();
    assert!(matches!(run_result[0], ServerMessage::Success(_)));
    let pull_result =
        pull::handle_pull(&mut dotnet_session, bolt_map! { "n" => BoltValue::Int(-1) })
            .await
            .unwrap();

    let records: Vec<_> = pull_result
        .iter()
        .filter(|m| matches!(m, ServerMessage::Record(_)))
        .collect();
    assert_eq!(
        records.len(),
        1,
        "data written with db:'neo4j' must be visible when querying with db:''"
    );
}

/// Simulates a driver omitting the `db` field entirely (BoltValue::Null or
/// absent key). Data written by a driver that sends `db: "neo4j"` must
/// still be visible.
#[tokio::test]
async fn cross_driver_explicit_neo4j_and_null_db_see_same_data() {
    let (mut nodejs_session, mut dotnet_session) = make_shared_sessions();

    // Node.js driver: CREATE with explicit db: "neo4j"
    run::handle_run(
        &mut nodejs_session,
        "CREATE (n:Driver {name: 'nodejs'}) RETURN n.name AS name".to_string(),
        vec![],
        bolt_map! { "db" => BoltValue::String("neo4j".to_string()) },
    )
    .await
    .unwrap();
    pull::handle_pull(&mut nodejs_session, bolt_map! { "n" => BoltValue::Int(-1) })
        .await
        .unwrap();

    // .NET driver: MATCH with no db field (omitted entirely)
    run::handle_run(
        &mut dotnet_session,
        "MATCH (n:Driver) RETURN n.name AS name".to_string(),
        vec![],
        vec![], // no db key at all
    )
    .await
    .unwrap();
    let pull_result =
        pull::handle_pull(&mut dotnet_session, bolt_map! { "n" => BoltValue::Int(-1) })
            .await
            .unwrap();

    let records: Vec<_> = pull_result
        .iter()
        .filter(|m| matches!(m, ServerMessage::Record(_)))
        .collect();
    assert_eq!(
        records.len(),
        1,
        "data written with db:'neo4j' must be visible when db is omitted"
    );
}

/// Simulates a driver sending `db: null` (BoltValue::Null). Data written
/// by a driver that sends `db: "neo4j"` must still be visible.
#[tokio::test]
async fn cross_driver_explicit_neo4j_and_bolt_null_db_see_same_data() {
    let (mut nodejs_session, mut dotnet_session) = make_shared_sessions();

    // Node.js driver: CREATE with explicit db: "neo4j"
    run::handle_run(
        &mut nodejs_session,
        "CREATE (n:Driver {name: 'nodejs'}) RETURN n.name AS name".to_string(),
        vec![],
        bolt_map! { "db" => BoltValue::String("neo4j".to_string()) },
    )
    .await
    .unwrap();
    pull::handle_pull(&mut nodejs_session, bolt_map! { "n" => BoltValue::Int(-1) })
        .await
        .unwrap();

    // .NET driver: MATCH with db: null (BoltValue::Null)
    run::handle_run(
        &mut dotnet_session,
        "MATCH (n:Driver) RETURN n.name AS name".to_string(),
        vec![],
        bolt_map! { "db" => BoltValue::Null },
    )
    .await
    .unwrap();
    let pull_result =
        pull::handle_pull(&mut dotnet_session, bolt_map! { "n" => BoltValue::Int(-1) })
            .await
            .unwrap();

    let records: Vec<_> = pull_result
        .iter()
        .filter(|m| matches!(m, ServerMessage::Record(_)))
        .collect();
    assert_eq!(
        records.len(),
        1,
        "data written with db:'neo4j' must be visible when db is null"
    );
}
