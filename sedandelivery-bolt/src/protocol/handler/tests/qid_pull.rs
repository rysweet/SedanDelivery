//! QID assignment, PULL routing by qid, db metadata propagation, pending
//! results cap, and parameterised query tests.

use super::super::*;
use super::make_ready_session;

// -- QID support tests -------------------------------------------------------

#[tokio::test]
async fn run_returns_qid_in_success() {
    let mut session = make_ready_session();
    let result = run::handle_run(&mut session, "RETURN 1 AS n".to_string(), vec![], vec![])
        .await
        .unwrap();
    assert_eq!(result.len(), 1);
    match &result[0] {
        ServerMessage::Success(meta) => {
            let qid = meta.get_int("qid");
            assert!(qid.is_some(), "RUN SUCCESS must include qid");
            assert_eq!(qid.unwrap(), 0);
        }
        other => panic!("expected Success, got {:?}", other),
    }
}

#[tokio::test]
async fn qid_increments_across_runs() {
    let mut session = make_ready_session();
    // First RUN
    let r1 = run::handle_run(&mut session, "RETURN 1 AS n".to_string(), vec![], vec![])
        .await
        .unwrap();
    // Pull to clear the first result
    pull::handle_pull(&mut session, bolt_map! { "n" => BoltValue::Int(-1) })
        .await
        .unwrap();

    // Second RUN
    let r2 = run::handle_run(&mut session, "RETURN 2 AS m".to_string(), vec![], vec![])
        .await
        .unwrap();
    let qid1 = match &r1[0] {
        ServerMessage::Success(m) => m.get_int("qid").unwrap(),
        _ => panic!("expected Success"),
    };
    let qid2 = match &r2[0] {
        ServerMessage::Success(m) => m.get_int("qid").unwrap(),
        _ => panic!("expected Success"),
    };
    assert_eq!(qid1, 0);
    assert_eq!(qid2, 1);
}

#[tokio::test]
async fn pull_uses_qid_from_extra() {
    let mut session = make_ready_session();
    // Begin a TX so we can have multiple concurrent results.
    transaction::handle_begin(&mut session, vec![])
        .await
        .unwrap();

    // First RUN (qid=0)
    let r1 = run::handle_run(&mut session, "RETURN 1 AS n".to_string(), vec![], vec![])
        .await
        .unwrap();
    let qid0 = match &r1[0] {
        ServerMessage::Success(m) => m.get_int("qid").unwrap(),
        _ => panic!("expected Success"),
    };

    // Second RUN (qid=1)
    let r2 = run::handle_run(&mut session, "RETURN 2 AS m".to_string(), vec![], vec![])
        .await
        .unwrap();
    let qid1 = match &r2[0] {
        ServerMessage::Success(m) => m.get_int("qid").unwrap(),
        _ => panic!("expected Success"),
    };

    // Pull from qid=0 specifically
    let pull0 = pull::handle_pull(
        &mut session,
        bolt_map! { "n" => BoltValue::Int(-1), "qid" => BoltValue::Int(qid0) },
    )
    .await
    .unwrap();
    let record0 = pull0.iter().find(|m| matches!(m, ServerMessage::Record(_)));
    assert!(record0.is_some(), "expected record from qid0");
    if let ServerMessage::Record(fields) = record0.unwrap() {
        assert_eq!(fields[0], BoltValue::Int(1));
    }

    // Pull from qid=1 specifically
    let pull1 = pull::handle_pull(
        &mut session,
        bolt_map! { "n" => BoltValue::Int(-1), "qid" => BoltValue::Int(qid1) },
    )
    .await
    .unwrap();
    let record1 = pull1.iter().find(|m| matches!(m, ServerMessage::Record(_)));
    assert!(record1.is_some(), "expected record from qid1");
    if let ServerMessage::Record(fields) = record1.unwrap() {
        assert_eq!(fields[0], BoltValue::Int(2));
    }
}

// -- DB metadata from effective graph tests ----------------------------------

#[tokio::test]
async fn pull_db_reflects_run_extra_db() {
    let mut session = make_ready_session();
    // RUN with db=custom_graph
    let extra: BoltMap = vec![(
        "db".to_string(),
        BoltValue::String("custom_graph".to_string()),
    )];
    run::handle_run(&mut session, "RETURN 1 AS n".to_string(), vec![], extra)
        .await
        .unwrap();
    let pull_result = pull::handle_pull(&mut session, bolt_map! { "n" => BoltValue::Int(-1) })
        .await
        .unwrap();
    let success = pull_result.last().unwrap();
    match success {
        ServerMessage::Success(meta) => {
            let db = meta.get_str("db").unwrap();
            assert_eq!(db, "custom_graph");
        }
        other => panic!("expected Success, got {:?}", other),
    }
}

// -- Pending results limit tests ---------------------------------------------

#[tokio::test]
async fn run_rejects_when_pending_results_at_cap() {
    let mut session = make_ready_session();
    // Begin a TX so multiple pending results can coexist.
    transaction::handle_begin(&mut session, vec![])
        .await
        .unwrap();

    // Default cap is 16 (DEFAULT_MAX_PENDING_RESULTS).
    let cap: usize = 16;
    // Fill up to the cap without pulling.
    for i in 0..cap {
        let result = run::handle_run(&mut session, format!("RETURN {} AS n", i), vec![], vec![])
            .await
            .unwrap();
        assert!(
            matches!(&result[0], ServerMessage::Success(_)),
            "RUN {} should succeed, got {:?}",
            i,
            result
        );
    }

    assert_eq!(session.pending_results.len(), cap);

    // The next RUN should be rejected.
    let rejected = run::handle_run(
        &mut session,
        "RETURN 'one too many' AS x".to_string(),
        vec![],
        vec![],
    )
    .await
    .unwrap();
    assert_eq!(rejected.len(), 1);
    match &rejected[0] {
        ServerMessage::Failure(meta) => {
            let code = meta.get_str("code").unwrap();
            assert_eq!(code, "Neo.ClientError.Request.Invalid");
            let msg = meta.get_str("message").unwrap();
            assert!(msg.contains("Too many pending results"));
        }
        other => panic!("expected Failure, got {:?}", other),
    }
    assert_eq!(session.state, ServerState::Failed);
}

#[tokio::test]
async fn run_succeeds_after_pulling_pending_result() {
    let mut session = make_ready_session();
    // Begin a TX so multiple pending results can coexist.
    transaction::handle_begin(&mut session, vec![])
        .await
        .unwrap();

    // Default cap is 16.
    let cap: usize = 16;
    // Fill up to the cap.
    for i in 0..cap {
        run::handle_run(&mut session, format!("RETURN {} AS n", i), vec![], vec![])
            .await
            .unwrap();
    }

    // Pull one result to free a slot (pull qid=0).
    pull::handle_pull(
        &mut session,
        bolt_map! { "n" => BoltValue::Int(-1), "qid" => BoltValue::Int(0) },
    )
    .await
    .unwrap();

    assert_eq!(session.pending_results.len(), cap - 1);

    // Now a new RUN should succeed.
    let result = run::handle_run(
        &mut session,
        "RETURN 'after pull' AS x".to_string(),
        vec![],
        vec![],
    )
    .await
    .unwrap();
    assert!(
        matches!(&result[0], ServerMessage::Success(_)),
        "RUN after pull should succeed, got {:?}",
        result
    );
}

// -- Parameterized query tests -----------------------------------------------

#[tokio::test]
async fn run_with_params_resolves_parameter_in_return() {
    let mut session = make_ready_session();
    let params: BoltMap = vec![("x".to_string(), BoltValue::Int(42))];
    let result = run::handle_run(&mut session, "RETURN $x AS val".to_string(), params, vec![])
        .await
        .unwrap();
    assert!(matches!(&result[0], ServerMessage::Success(_)));

    let pull_result = pull::handle_pull(&mut session, bolt_map! { "n" => BoltValue::Int(-1) })
        .await
        .unwrap();
    let record = pull_result
        .iter()
        .find(|m| matches!(m, ServerMessage::Record(_)));
    assert!(
        record.is_some(),
        "expected a record from parameterized query"
    );
    if let ServerMessage::Record(fields) = record.unwrap() {
        assert_eq!(
            fields[0],
            BoltValue::Int(42),
            "parameter $x should resolve to 42"
        );
    }
}

#[tokio::test]
async fn run_with_string_param_resolves() {
    let mut session = make_ready_session();
    let params: BoltMap = vec![("name".to_string(), BoltValue::String("Alice".to_string()))];
    let result = run::handle_run(
        &mut session,
        "RETURN $name AS who".to_string(),
        params,
        vec![],
    )
    .await
    .unwrap();
    assert!(matches!(&result[0], ServerMessage::Success(_)));

    let pull_result = pull::handle_pull(&mut session, bolt_map! { "n" => BoltValue::Int(-1) })
        .await
        .unwrap();
    let record = pull_result
        .iter()
        .find(|m| matches!(m, ServerMessage::Record(_)));
    assert!(record.is_some(), "expected a record");
    if let ServerMessage::Record(fields) = record.unwrap() {
        assert_eq!(
            fields[0],
            BoltValue::String("Alice".to_string()),
            "parameter $name should resolve to 'Alice'"
        );
    }
}

#[tokio::test]
async fn run_with_multiple_params() {
    let mut session = make_ready_session();
    let params: BoltMap = vec![
        ("a".to_string(), BoltValue::Int(10)),
        ("b".to_string(), BoltValue::Int(20)),
    ];
    let result = run::handle_run(
        &mut session,
        "RETURN $a + $b AS sum".to_string(),
        params,
        vec![],
    )
    .await
    .unwrap();
    assert!(matches!(&result[0], ServerMessage::Success(_)));

    let pull_result = pull::handle_pull(&mut session, bolt_map! { "n" => BoltValue::Int(-1) })
        .await
        .unwrap();
    let record = pull_result
        .iter()
        .find(|m| matches!(m, ServerMessage::Record(_)));
    assert!(record.is_some(), "expected a record");
    if let ServerMessage::Record(fields) = record.unwrap() {
        assert_eq!(fields[0], BoltValue::Int(30), "$a + $b should be 30");
    }
}
