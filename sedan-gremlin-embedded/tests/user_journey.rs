//! End-to-end **user journey** tests for `sedan-gremlin-embedded`.
//!
//! Drives the same social-graph scenario as the cypher- and gql-embedded
//! journeys but expressed entirely in Apache TinkerPop Gremlin: graph
//! mutation via `addV` / `addE`, traversals via `g.V().has()...out()`,
//! aggregation via `groupCount().by(...)`, and compile-once /
//! execute-many via [`GremlinEngine::compile`] +
//! [`GremlinEngine::execute_compiled`].
//!
//! Hermetic — uses [`InMemoryGraphStore`] only, no I/O.

use sedan_gremlin_embedded::{GraphStore, GremlinEngine, GremlinError, InMemoryGraphStore, Value};

/// 5 vertices labelled `Person` with `name` and `age`, connected by 6
/// directed `FOLLOWS` edges with a `since` (year) property:
///
///   Alice -> Bob   (2020)
///   Alice -> Carol (2021)
///   Bob   -> Carol (2019)
///   Bob   -> Eve   (2018)
///   Carol -> Dave  (2022)
///   Dave  -> Eve   (2023)
///
/// Ages: Alice 30, Bob 25, Carol 40, Dave 35, Eve 28.
fn seed_social_graph(engine: &GremlinEngine, store: &dyn GraphStore) {
    let people = [
        ("Alice", 30),
        ("Bob", 25),
        ("Carol", 40),
        ("Dave", 35),
        ("Eve", 28),
    ];
    for (name, age) in people {
        let q = format!(r#"g.addV("Person").property("name","{name}").property("age",{age})"#);
        engine.execute(store, "g", &q).expect("addV");
    }

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
            r#"g.V().has("name","{from}").addE("FOLLOWS").to(V().has("name","{to}")).property("since",{since})"#
        );
        engine.execute(store, "g", &q).expect("addE");
    }
}

fn fresh() -> (GremlinEngine, InMemoryGraphStore) {
    let engine = GremlinEngine::new();
    let store = InMemoryGraphStore::default();
    store.create_graph("g").unwrap();
    seed_social_graph(&engine, &store);
    (engine, store)
}

fn first_value(qr: &sedan_gremlin_embedded::QueryResult) -> &Value {
    let col = qr.columns.first().expect("at least one column");
    qr.rows[0].get(col).expect("first column present")
}

// ── Sanity: graph shape ──────────────────────────────────────────────────────

#[test]
fn journey_seed_creates_5_vertices_and_6_edges() {
    let (engine, store) = fresh();
    let v = engine.execute(&store, "g", "g.V().count()").unwrap();
    assert_eq!(first_value(&v), &Value::Int(5));
    let e = engine.execute(&store, "g", "g.E().count()").unwrap();
    assert_eq!(first_value(&e), &Value::Int(6));
}

// ── Journey 1: filter via has() ──────────────────────────────────────────────

#[test]
fn journey_has_filter_returns_single_match() {
    let (engine, store) = fresh();
    let r = engine
        .execute(
            &store,
            "g",
            r#"g.V().hasLabel("Person").has("name","Alice").values("age")"#,
        )
        .unwrap();
    assert_eq!(r.rows.len(), 1);
    assert_eq!(first_value(&r), &Value::Int(30));
}

// ── Journey 2: out() traversal returns followees ─────────────────────────────

/// Bob follows Carol (2019) and Eve (2018) — exactly 2 names.
#[test]
fn journey_out_traversal_returns_followees() {
    let (engine, store) = fresh();
    let r = engine
        .execute(
            &store,
            "g",
            r#"g.V().has("name","Bob").out("FOLLOWS").values("name")"#,
        )
        .unwrap();
    assert_eq!(r.rows.len(), 2);
    let mut names: Vec<String> = r
        .rows
        .iter()
        .map(|row| match row.get(r.columns.first().unwrap()).unwrap() {
            Value::Str(s) => s.to_string(),
            v => panic!("expected Str, got {v:?}"),
        })
        .collect();
    names.sort();
    assert_eq!(names, vec!["Carol".to_string(), "Eve".to_string()]);
}

/// `in("FOLLOWS")` returns followers — Carol is followed by Alice and Bob.
#[test]
fn journey_in_traversal_returns_followers() {
    let (engine, store) = fresh();
    let r = engine
        .execute(
            &store,
            "g",
            r#"g.V().has("name","Carol").in("FOLLOWS").values("name")"#,
        )
        .unwrap();
    assert_eq!(r.rows.len(), 2);
    let mut names: Vec<String> = r
        .rows
        .iter()
        .map(|row| match row.get(r.columns.first().unwrap()).unwrap() {
            Value::Str(s) => s.to_string(),
            v => panic!("expected Str, got {v:?}"),
        })
        .collect();
    names.sort();
    assert_eq!(names, vec!["Alice".to_string(), "Bob".to_string()]);
}

// ── Journey 3: group()/groupCount() aggregation ──────────────────────────────

/// `g.V().groupCount().by("age")` yields one row whose value is a `Map`
/// from each distinct age to the count of vertices with that age.
#[test]
fn journey_group_count_by_age() {
    let (engine, store) = fresh();
    let r = engine
        .execute(&store, "g", r#"g.V().groupCount().by("age")"#)
        .unwrap();
    assert_eq!(r.rows.len(), 1);
    let map = match first_value(&r) {
        Value::Map(m) => m.clone(),
        v => panic!("expected Map, got {v:?}"),
    };
    // Every person has a distinct age, so every key maps to 1.
    assert_eq!(map.len(), 5);
    for v in map.values() {
        assert_eq!(v, &Value::Int(1));
    }
}

/// `g.V().group().by("age").by(count())` is the explicit form of the
/// same aggregation — must produce an equivalent map.
#[test]
fn journey_group_by_age_by_count_matches_group_count() {
    let (engine, store) = fresh();
    let a = engine
        .execute(&store, "g", r#"g.V().groupCount().by("age")"#)
        .unwrap();
    let b = engine
        .execute(&store, "g", r#"g.V().group().by("age").by(count())"#)
        .unwrap();
    assert_eq!(first_value(&a), first_value(&b));
}

// ── Journey 4: outE / inV traversal exposes edge properties ──────────────────

/// Alice's outgoing FOLLOWS edges have `since` years 2020 and 2021.
#[test]
fn journey_out_e_then_values_returns_edge_props() {
    let (engine, store) = fresh();
    let r = engine
        .execute(
            &store,
            "g",
            r#"g.V().has("name","Alice").outE("FOLLOWS").values("since")"#,
        )
        .unwrap();
    assert_eq!(r.rows.len(), 2);
    let col = r.columns.first().unwrap();
    let mut years: Vec<i64> = r
        .rows
        .iter()
        .map(|row| match row.get(col).unwrap() {
            Value::Int(i) => *i,
            v => panic!("expected Int, got {v:?}"),
        })
        .collect();
    years.sort();
    assert_eq!(years, vec![2020, 2021]);
}

// ── Journey 5: error handling ────────────────────────────────────────────────

#[test]
fn journey_malformed_query_returns_parse_error() {
    let (engine, store) = fresh();
    let err = engine
        .execute(&store, "g", "this is not gremlin")
        .expect_err("malformed query must error");
    match err {
        GremlinError::Parse(_) | GremlinError::Compile(_) => {}
        GremlinError::Execution(e) => panic!("unexpected execution error: {e:?}"),
    }

    // Engine remains usable.
    let after = engine.execute(&store, "g", "g.V().count()").unwrap();
    assert_eq!(first_value(&after), &Value::Int(5));
}

#[test]
fn journey_unknown_step_returns_parse_or_compile_error() {
    let (engine, store) = fresh();
    let err = engine
        .execute(&store, "g", "g.V().nonexistentStep()")
        .expect_err("unknown step must error");
    match err {
        GremlinError::Parse(_) | GremlinError::Compile(_) => {}
        GremlinError::Execution(_) => {}
    }
}

// ── Journey 6: compile + execute_compiled (compiled-traversal reuse) ─────────

/// Compile a single traversal once, then execute it many times — every
/// execution must yield the same answer as the un-compiled fast path,
/// and the compiled handle must remain re-usable across mutations to
/// the underlying store.
#[test]
fn journey_compile_then_execute_compiled_reuses_traversal() {
    let (engine, store) = fresh();

    let count_q = engine.compile("g.V().count()").unwrap();
    let names_q = engine
        .compile(r#"g.V().has("name","Bob").out("FOLLOWS").values("name").count()"#)
        .unwrap();

    // 10 successive executions of `count()` must each see the same
    // current vertex count (5 — no writes yet).
    for _ in 0..10 {
        let r = engine.execute_compiled(&store, "g", &count_q).unwrap();
        assert_eq!(first_value(&r), &Value::Int(5));
    }

    // Bob has 2 followees.
    let r = engine.execute_compiled(&store, "g", &names_q).unwrap();
    assert_eq!(first_value(&r), &Value::Int(2));

    // Add a new vertex via the engine; the previously compiled count_q
    // must observe the update on the next execution.
    engine
        .execute(
            &store,
            "g",
            r#"g.addV("Person").property("name","Frank").property("age",50)"#,
        )
        .unwrap();
    let r = engine.execute_compiled(&store, "g", &count_q).unwrap();
    assert_eq!(
        first_value(&r),
        &Value::Int(6),
        "compiled traversal must see writes performed after compile()"
    );
}

#[test]
fn journey_compile_failure_for_invalid_query() {
    let engine = GremlinEngine::new();
    // Unknown step that cannot be lowered to an operator.
    let err = match engine.compile("g.V().nonexistentStep()") {
        Ok(_) => panic!("expected compile failure for unknown step"),
        Err(e) => e,
    };
    match err {
        GremlinError::Parse(_) | GremlinError::Compile(_) => {}
        GremlinError::Execution(e) => panic!("unexpected execution error: {e:?}"),
    }
}
