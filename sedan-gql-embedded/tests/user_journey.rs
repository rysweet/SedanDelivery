//! End-to-end **user journey** tests for `sedan-gql-embedded`.
//!
//! Mirrors the cypher-embedded journeys but drives the engine with GQL
//! (ISO/IEC 39075) syntax — `INSERT` for writes, parameter binding via
//! `execute_with_context`, and the same multi-clause read patterns the
//! GQL spec defines on top of MATCH/WHERE/WITH/RETURN.
//!
//! Hermetic — uses [`InMemoryGraphStore`] only, no I/O.

use std::collections::HashMap;

use sedan_gql_embedded::{
    GqlError, GraphStore, InMemoryGraphStore, ProcedureRegistry, SedanGql, Value,
};

/// 5 people connected by directed FOLLOWS edges with a `since` (year) prop.
///
///   Alice -> Bob   (2020)
///   Alice -> Carol (2021)
///   Bob   -> Carol (2019)
///   Bob   -> Eve   (2018)
///   Carol -> Dave  (2022)
///   Dave  -> Eve   (2023)
///
/// Ages: Alice 30, Bob 25, Carol 40, Dave 35, Eve 28.
fn seed_social_graph(engine: &SedanGql, store: &dyn GraphStore) {
    // Step 1 — INSERT the 5 nodes (GQL spelling). Done in one statement so
    // the engine sees a single transaction-equivalent batch.
    engine
        .execute(
            store,
            "g",
            "INSERT (:Person {name: 'Alice', age: 30}),\
                    (:Person {name: 'Bob',   age: 25}),\
                    (:Person {name: 'Carol', age: 40}),\
                    (:Person {name: 'Dave',  age: 35}),\
                    (:Person {name: 'Eve',   age: 28})",
        )
        .expect("seed nodes");

    // Step 2 — INSERT each FOLLOWS edge by re-matching its endpoints.
    let edges = [
        ("Alice", "Bob", 2020),
        ("Alice", "Carol", 2021),
        ("Bob", "Carol", 2019),
        ("Bob", "Eve", 2018),
        ("Carol", "Dave", 2022),
        ("Dave", "Eve", 2023),
    ];
    for (from, to, since) in edges {
        let q = format!(
            "MATCH (a:Person {{name: '{from}'}}), (b:Person {{name: '{to}'}}) \
             INSERT (a)-[:FOLLOWS {{since: {since}}}]->(b)"
        );
        engine.execute(store, "g", &q).expect("seed edge");
    }
}

fn fresh() -> (SedanGql, InMemoryGraphStore) {
    let engine = SedanGql::new();
    let store = InMemoryGraphStore::default();
    store.create_graph("g").unwrap();
    seed_social_graph(&engine, &store);
    (engine, store)
}

// ── Sanity: graph shape ──────────────────────────────────────────────────────

#[test]
fn journey_seed_creates_5_nodes_and_6_edges() {
    let (engine, store) = fresh();

    let nodes = engine
        .execute(&store, "g", "MATCH (p:Person) RETURN count(p) AS n")
        .unwrap();
    assert_eq!(nodes.rows[0].get("n"), Some(&Value::Int(5)));

    let edges = engine
        .execute(
            &store,
            "g",
            "MATCH (:Person)-[r:FOLLOWS]->(:Person) RETURN count(r) AS m",
        )
        .unwrap();
    assert_eq!(edges.rows[0].get("m"), Some(&Value::Int(6)));
}

// ── Journey 1: filter + project + order + limit ──────────────────────────────

#[test]
fn journey_filter_order_limit_picks_top_two_oldest() {
    let (engine, store) = fresh();
    let r = engine
        .execute(
            &store,
            "g",
            "MATCH (p:Person) WHERE p.age >= 28 \
             RETURN p.name AS name, p.age AS age \
             ORDER BY p.age DESC LIMIT 2",
        )
        .unwrap();

    assert_eq!(r.rows.len(), 2);
    assert_eq!(r.rows[0].get("name"), Some(&Value::Str("Carol".into())));
    assert_eq!(r.rows[0].get("age"), Some(&Value::Int(40)));
    assert_eq!(r.rows[1].get("name"), Some(&Value::Str("Dave".into())));
    assert_eq!(r.rows[1].get("age"), Some(&Value::Int(35)));
}

// ── Journey 2: WITH + aggregation pipeline ───────────────────────────────────

#[test]
fn journey_with_aggregation_finds_prolific_followers() {
    let (engine, store) = fresh();

    let r = engine
        .execute(
            &store,
            "g",
            "MATCH (p:Person)-[:FOLLOWS]->(t:Person) \
             WITH p, count(t) AS follow_count \
             WHERE follow_count >= 2 \
             RETURN p.name AS name, follow_count \
             ORDER BY follow_count DESC, name ASC",
        )
        .unwrap();

    assert_eq!(r.rows.len(), 2);
    assert_eq!(r.rows[0].get("name"), Some(&Value::Str("Alice".into())));
    assert_eq!(r.rows[0].get("follow_count"), Some(&Value::Int(2)));
    assert_eq!(r.rows[1].get("name"), Some(&Value::Str("Bob".into())));
    assert_eq!(r.rows[1].get("follow_count"), Some(&Value::Int(2)));
}

// ── Journey 3: OPTIONAL MATCH gives left-join semantics ──────────────────────

#[test]
fn journey_optional_match_keeps_unmatched_people() {
    let (engine, store) = fresh();
    let r = engine
        .execute(
            &store,
            "g",
            "MATCH (p:Person) \
             OPTIONAL MATCH (p)-[:FOLLOWS]->(t:Person) \
             RETURN p.name AS name, count(t) AS out_degree \
             ORDER BY name ASC",
        )
        .unwrap();

    assert_eq!(r.rows.len(), 5, "every person appears exactly once");
    let degrees: HashMap<String, i64> = r
        .rows
        .iter()
        .map(|row| {
            let n = match row.get("name").unwrap() {
                Value::Str(s) => s.to_string(),
                v => panic!("expected Str, got {v:?}"),
            };
            let d = match row.get("out_degree").unwrap() {
                Value::Int(i) => *i,
                v => panic!("expected Int, got {v:?}"),
            };
            (n, d)
        })
        .collect();
    assert_eq!(degrees["Alice"], 2);
    assert_eq!(degrees["Bob"], 2);
    assert_eq!(degrees["Carol"], 1);
    assert_eq!(degrees["Dave"], 1);
    assert_eq!(degrees["Eve"], 0);
}

// ── Journey 4: parameterised query with $min_year ────────────────────────────

#[test]
fn journey_parameter_filters_edges_by_year() {
    let (engine, store) = fresh();
    let mut params = HashMap::new();
    params.insert("min_year".to_string(), Value::Int(2020));
    let r = engine
        .execute_with_context(
            &store,
            "g",
            "MATCH (a:Person)-[r:FOLLOWS]->(b:Person) \
             WHERE r.since >= $min_year \
             RETURN a.name AS follower, b.name AS followee, r.since AS year \
             ORDER BY year ASC, follower ASC",
            &ProcedureRegistry::default(),
            params,
        )
        .unwrap();

    assert_eq!(r.rows.len(), 4);
    let years: Vec<_> = r
        .rows
        .iter()
        .map(|row| row.get("year").cloned().unwrap())
        .collect();
    assert_eq!(
        years,
        vec![
            Value::Int(2020),
            Value::Int(2021),
            Value::Int(2022),
            Value::Int(2023),
        ]
    );
    assert_eq!(r.rows[0].get("follower"), Some(&Value::Str("Alice".into())));
    assert_eq!(r.rows[3].get("followee"), Some(&Value::Str("Eve".into())));
}

/// String-typed parameter binding (the GQL spec allows any literal type).
#[test]
fn journey_string_parameter_filters_by_name() {
    let (engine, store) = fresh();
    let mut params = HashMap::new();
    params.insert("who".to_string(), Value::Str("Alice".into()));
    let r = engine
        .execute_with_context(
            &store,
            "g",
            "MATCH (a:Person {name: $who})-[:FOLLOWS]->(t:Person) \
             RETURN t.name AS followee ORDER BY followee",
            &ProcedureRegistry::default(),
            params,
        )
        .unwrap();
    assert_eq!(r.rows.len(), 2);
    assert_eq!(r.rows[0].get("followee"), Some(&Value::Str("Bob".into())));
    assert_eq!(r.rows[1].get("followee"), Some(&Value::Str("Carol".into())));
}

// ── Journey 5: error handling ────────────────────────────────────────────────

#[test]
fn journey_malformed_query_surfaces_typed_error() {
    let (engine, store) = fresh();
    let err = engine
        .execute(&store, "g", "INSERT GARBAGE WITHOUT PARENS")
        .expect_err("malformed query must error");
    let _: GqlError = err;

    // Engine remains usable.
    let after = engine
        .execute(&store, "g", "MATCH (p:Person) RETURN count(p) AS n")
        .unwrap();
    assert_eq!(after.rows[0].get("n"), Some(&Value::Int(5)));
}

#[test]
fn journey_missing_parameter_surfaces_error() {
    let (engine, store) = fresh();
    let err = engine
        .execute_with_context(
            &store,
            "g",
            "MATCH (p:Person) WHERE p.age > $missing RETURN p",
            &ProcedureRegistry::default(),
            HashMap::new(),
        )
        .expect_err("missing parameter must error");
    let _: GqlError = err;
}

// ── Journey 6: repeated execution returns identical results ──────────────────

#[test]
fn journey_repeated_execution_is_deterministic() {
    let (engine, store) = fresh();

    let q = "MATCH (p:Person) RETURN p.name AS name ORDER BY name";
    let baseline = engine.execute(&store, "g", q).unwrap();
    assert_eq!(baseline.rows.len(), 5);
    let expected_names = ["Alice", "Bob", "Carol", "Dave", "Eve"];
    for (row, expected) in baseline.rows.iter().zip(expected_names.iter()) {
        assert_eq!(row.get("name"), Some(&Value::Str((*expected).into())));
    }

    for _ in 0..10 {
        let r = engine.execute(&store, "g", q).unwrap();
        assert_eq!(r.columns, baseline.columns);
        assert_eq!(r.rows.len(), baseline.rows.len());
        for (got, expected) in r.rows.iter().zip(baseline.rows.iter()) {
            assert_eq!(got.get("name"), expected.get("name"));
        }
    }
}

// ── Journey 7: average age aggregation across followees ──────────────────────

/// Bob's followees are Carol (40) and Eve (28); average 34.
#[test]
fn journey_average_age_of_followees_per_person() {
    let (engine, store) = fresh();
    let r = engine
        .execute(
            &store,
            "g",
            "MATCH (p:Person {name: 'Bob'})-[:FOLLOWS]->(t:Person) \
             RETURN avg(t.age) AS avg_age, count(t) AS n",
        )
        .unwrap();
    assert_eq!(r.rows[0].get("n"), Some(&Value::Int(2)));
    match r.rows[0].get("avg_age") {
        Some(Value::Float(f)) => assert!((f - 34.0).abs() < 1e-9, "got {f}"),
        Some(Value::Int(i)) => assert_eq!(*i, 34),
        other => panic!("unexpected avg type: {other:?}"),
    }
}
