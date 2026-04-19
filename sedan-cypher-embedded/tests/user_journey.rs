//! End-to-end **user journey** tests for `sedan-cypher-embedded`.
//!
//! Walks through a realistic workflow against a small social graph the way
//! a downstream library author would: build a graph, ask multi-clause
//! questions, parameterise queries, demonstrate plan-cache reuse, and
//! verify the public error contract.
//!
//! Hermetic — uses [`InMemoryGraphStore`] only, no I/O.

use std::collections::HashMap;

use sedan_cypher_embedded::{
    CypherEngine, GqlError, GraphStore, InMemoryGraphStore, ProcedureRegistry, Value,
};

/// 5 people connected by directed FOLLOWS edges with a `since` (year) prop.
///
/// Followers (out): edges leave the FROM person.
///   Alice -> Bob   (2020)
///   Alice -> Carol (2021)
///   Bob   -> Carol (2019)
///   Bob   -> Eve   (2018)
///   Carol -> Dave  (2022)
///   Dave  -> Eve   (2023)
///
/// Ages: Alice 30, Bob 25, Carol 40, Dave 35, Eve 28.
fn seed_social_graph(engine: &CypherEngine, store: &dyn GraphStore) {
    engine
        .execute(
            store,
            "g",
            "CREATE (a:Person {name: 'Alice', age: 30}),\
                    (b:Person {name: 'Bob',   age: 25}),\
                    (c:Person {name: 'Carol', age: 40}),\
                    (d:Person {name: 'Dave',  age: 35}),\
                    (e:Person {name: 'Eve',   age: 28}),\
                    (a)-[:FOLLOWS {since: 2020}]->(b),\
                    (a)-[:FOLLOWS {since: 2021}]->(c),\
                    (b)-[:FOLLOWS {since: 2019}]->(c),\
                    (b)-[:FOLLOWS {since: 2018}]->(e),\
                    (c)-[:FOLLOWS {since: 2022}]->(d),\
                    (d)-[:FOLLOWS {since: 2023}]->(e)",
        )
        .expect("seed graph");
}

fn fresh() -> (CypherEngine, InMemoryGraphStore) {
    let engine = CypherEngine::new();
    let store = InMemoryGraphStore::default();
    store.create_graph("g").unwrap();
    seed_social_graph(&engine, &store);
    (engine, store)
}

// ── Journey 1: filter + project + order + limit ──────────────────────────────

/// A user wants the two oldest people aged at least 28, ordered oldest first.
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

/// For every person, compute how many people they follow. Pipe through
/// `WITH`, filter to the prolific followers (>= 2), and order by count desc
/// then name asc for stable ties.
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

    // Alice follows {Bob, Carol} = 2; Bob follows {Carol, Eve} = 2.
    // Carol/Dave each follow only 1; Eve follows 0.
    assert_eq!(r.rows.len(), 2);
    assert_eq!(r.rows[0].get("name"), Some(&Value::Str("Alice".into())));
    assert_eq!(r.rows[0].get("follow_count"), Some(&Value::Int(2)));
    assert_eq!(r.rows[1].get("name"), Some(&Value::Str("Bob".into())));
    assert_eq!(r.rows[1].get("follow_count"), Some(&Value::Int(2)));
}

// ── Journey 3: OPTIONAL MATCH gives left-join semantics ──────────────────────

/// Every person is returned even if they have no outgoing FOLLOWS edge.
/// Eve follows nobody, so her followee column is `NULL`.
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
    // Eve has no outgoing FOLLOWS — OPTIONAL MATCH yields a NULL row that
    // count() correctly aggregates to 0.
    assert_eq!(degrees["Eve"], 0);
}

// ── Journey 4: parameterised query with $min_year ────────────────────────────

/// A user filters relationships by edge property, threshold supplied as a
/// parameter rather than baked into the query string.
#[test]
fn journey_parameter_filters_edges_by_year() {
    let (engine, store) = fresh();

    let mut params = HashMap::new();
    params.insert("min_year".to_string(), Value::Int(2020));
    let r = engine
        .execute_with_parameters(
            &store,
            "g",
            "MATCH (a:Person)-[r:FOLLOWS]->(b:Person) \
             WHERE r.since >= $min_year \
             RETURN a.name AS follower, b.name AS followee, r.since AS year \
             ORDER BY year ASC, follower ASC",
            &params,
            &ProcedureRegistry::default(),
        )
        .unwrap();

    // since >= 2020 picks: Alice->Bob 2020, Alice->Carol 2021,
    //                       Carol->Dave 2022, Dave->Eve 2023.
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

// ── Journey 5: error handling for malformed / nonsense queries ───────────────

#[test]
fn journey_malformed_query_surfaces_parse_error() {
    let (engine, store) = fresh();

    let err = engine
        .execute(&store, "g", "MATCH (n WHERE BUT NOT VALID")
        .expect_err("malformed query must error");

    match err {
        GqlError::Parse(_) => {}
        other => panic!("expected GqlError::Parse, got {other:?}"),
    }

    // Engine is still usable after a failure.
    let after = engine
        .execute(&store, "g", "MATCH (p:Person) RETURN count(p) AS n")
        .unwrap();
    assert_eq!(after.rows[0].get("n"), Some(&Value::Int(5)));
}

#[test]
fn journey_unknown_parameter_surfaces_typed_error() {
    let (engine, store) = fresh();

    // No `$missing` supplied — must error, not silently substitute NULL.
    let err = engine
        .execute_with_parameters(
            &store,
            "g",
            "MATCH (p:Person) WHERE p.age > $missing RETURN p",
            &HashMap::new(),
            &ProcedureRegistry::default(),
        )
        .expect_err("missing parameter must error");
    let _: GqlError = err; // type-only contract; specific variant is impl detail.
}

// ── Journey 6: plan-cache reuse for repeated queries ─────────────────────────

/// Repeating the same query string ten times must produce ten identical
/// results. The cache is internal state — the contract we expose is "same
/// answer, fast, regardless of cache hit vs miss".
#[test]
fn journey_plan_cache_reuse_returns_identical_results() {
    let (engine, store) = fresh();

    let q = "MATCH (p:Person) RETURN p.name AS name ORDER BY name";

    let baseline = engine.execute(&store, "g", q).unwrap();
    assert_eq!(baseline.rows.len(), 5);

    for _ in 0..10 {
        let r = engine.execute(&store, "g", q).unwrap();
        assert_eq!(r.columns, baseline.columns);
        assert_eq!(r.rows.len(), baseline.rows.len());
        for (got, expected) in r.rows.iter().zip(baseline.rows.iter()) {
            assert_eq!(got.get("name"), expected.get("name"));
        }
    }

    // After clear_cache(), the same query must still produce the same
    // result — the cache is purely a perf concern, not a correctness one.
    engine.clear_cache();
    let after_clear = engine.execute(&store, "g", q).unwrap();
    assert_eq!(after_clear.rows.len(), baseline.rows.len());
}

/// `with_cache_capacity(0)` disables caching entirely; results must still
/// be correct and equal to the cached engine's results.
#[test]
fn journey_uncached_engine_matches_cached_engine() {
    let (cached, store) = fresh();
    let uncached = CypherEngine::with_cache_capacity(0);

    let q = "MATCH (p:Person)-[:FOLLOWS]->(t) RETURN p.name AS f, t.name AS to ORDER BY f, to";
    let a = cached.execute(&store, "g", q).unwrap();
    let b = uncached.execute(&store, "g", q).unwrap();

    assert_eq!(a.columns, b.columns);
    assert_eq!(a.rows.len(), 6);
    assert_eq!(a.rows.len(), b.rows.len());
    for (l, r) in a.rows.iter().zip(b.rows.iter()) {
        assert_eq!(l.get("f"), r.get("f"));
        assert_eq!(l.get("to"), r.get("to"));
    }
}

// ── Journey 7: writes through the cache see fresh state ──────────────────────

/// Cached read plans must observe writes performed between executions.
#[test]
fn journey_cached_plan_sees_writes() {
    let (engine, store) = fresh();

    let q = "MATCH (p:Person) RETURN count(p) AS n";
    assert_eq!(
        engine.execute(&store, "g", q).unwrap().rows[0].get("n"),
        Some(&Value::Int(5))
    );

    // Add a 6th person via a different query string; the cached count
    // plan must reflect the new node on the next call.
    engine
        .execute(&store, "g", "CREATE (:Person {name: 'Frank', age: 50})")
        .unwrap();

    assert_eq!(
        engine.execute(&store, "g", q).unwrap().rows[0].get("n"),
        Some(&Value::Int(6)),
        "cached read plan must see writes from later queries"
    );
}
