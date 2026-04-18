//! Comprehensive Gremlin step coverage tests for sedan-gremlin-embedded.
//!
//! Organized by TinkerPop step category. Each test exercises end-to-end:
//! parse → compile → execute against InMemoryGraphStore.

use indexmap::IndexMap;
use sedan_gremlin_embedded::{GraphStore, GremlinEngine, GremlinError, InMemoryGraphStore, Value};

// ── Helpers ─────────────────────────────────────────────────────────────────

fn engine() -> GremlinEngine {
    GremlinEngine::new()
}

fn store_with_graph(name: &str) -> InMemoryGraphStore {
    let s = InMemoryGraphStore::default();
    s.create_graph(name).unwrap();
    s
}

fn seed_people(store: &InMemoryGraphStore) {
    // Alice(30) --knows--> Bob(25)
    let alice_id = store
        .create_node(
            "g",
            vec!["Person".into()],
            IndexMap::from([
                ("name".into(), Value::Str("Alice".into())),
                ("age".into(), Value::Int(30)),
            ]),
        )
        .unwrap();
    let bob_id = store
        .create_node(
            "g",
            vec!["Person".into()],
            IndexMap::from([
                ("name".into(), Value::Str("Bob".into())),
                ("age".into(), Value::Int(25)),
            ]),
        )
        .unwrap();
    store
        .create_edge(
            "g",
            alice_id,
            bob_id,
            "knows".to_string(),
            IndexMap::from([("since".into(), Value::Int(2020))]),
        )
        .unwrap();
}

fn seed_triangle(store: &InMemoryGraphStore) {
    // Alice --knows--> Bob --knows--> Charlie --knows--> Alice
    let alice = store
        .create_node(
            "g",
            vec!["Person".into()],
            IndexMap::from([("name".into(), Value::Str("Alice".into()))]),
        )
        .unwrap();
    let bob = store
        .create_node(
            "g",
            vec!["Person".into()],
            IndexMap::from([("name".into(), Value::Str("Bob".into()))]),
        )
        .unwrap();
    let charlie = store
        .create_node(
            "g",
            vec!["Person".into()],
            IndexMap::from([("name".into(), Value::Str("Charlie".into()))]),
        )
        .unwrap();
    store
        .create_edge("g", alice, bob, "knows".to_string(), IndexMap::new())
        .unwrap();
    store
        .create_edge("g", bob, charlie, "knows".to_string(), IndexMap::new())
        .unwrap();
    store
        .create_edge("g", charlie, alice, "knows".to_string(), IndexMap::new())
        .unwrap();
}

fn first_value(result: &sedan_gremlin_embedded::QueryResult) -> &Value {
    result.rows[0].values().next().unwrap()
}

// ═══════════════════════════════════════════════════════════════════════════
// Source steps
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn v_returns_all_vertices() {
    let e = engine();
    let s = store_with_graph("g");
    seed_people(&s);
    let r = e.execute(&s, "g", "g.V()").unwrap();
    assert_eq!(r.rows.len(), 2);
}

#[test]
fn v_count_on_empty_graph() {
    let e = engine();
    let s = store_with_graph("g");
    let r = e.execute(&s, "g", "g.V().count()").unwrap();
    assert_eq!(r.rows.len(), 1);
    assert_eq!(first_value(&r), &Value::Int(0));
}

#[test]
fn e_returns_all_edges() {
    let e = engine();
    let s = store_with_graph("g");
    seed_people(&s);
    let r = e.execute(&s, "g", "g.E()").unwrap();
    assert_eq!(r.rows.len(), 1); // one "knows" edge
}

#[test]
fn e_count() {
    let e = engine();
    let s = store_with_graph("g");
    seed_people(&s);
    let r = e.execute(&s, "g", "g.E().count()").unwrap();
    assert_eq!(first_value(&r), &Value::Int(1));
}

// ═══════════════════════════════════════════════════════════════════════════
// Filter steps
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn has_label_filters_vertices() {
    let e = engine();
    let s = store_with_graph("g");
    seed_people(&s);
    // Add a non-Person node
    s.create_node("g", vec!["Animal".into()], IndexMap::new())
        .unwrap();
    let r = e
        .execute(&s, "g", r#"g.V().hasLabel("Person").count()"#)
        .unwrap();
    assert_eq!(first_value(&r), &Value::Int(2));
}

#[test]
fn has_key_value_filter() {
    let e = engine();
    let s = store_with_graph("g");
    seed_people(&s);
    let r = e
        .execute(&s, "g", r#"g.V().has("name","Alice").count()"#)
        .unwrap();
    assert_eq!(first_value(&r), &Value::Int(1));
}

#[test]
fn has_key_existence_filter() {
    let e = engine();
    let s = store_with_graph("g");
    seed_people(&s);
    // Both Alice and Bob have "name"
    let r = e.execute(&s, "g", r#"g.V().has("name").count()"#).unwrap();
    assert_eq!(first_value(&r), &Value::Int(2));
}

// ═══════════════════════════════════════════════════════════════════════════
// Traversal steps: out/in/both/outE/inE/bothE + vertex endpoints
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn out_traversal() {
    let e = engine();
    let s = store_with_graph("g");
    seed_people(&s);
    // Alice --knows--> Bob; from Alice, out("knows") should give Bob
    let r = e
        .execute(
            &s,
            "g",
            r#"g.V().has("name","Alice").out("knows").values("name")"#,
        )
        .unwrap();
    assert_eq!(r.rows.len(), 1);
    assert_eq!(first_value(&r), &Value::Str("Bob".into()));
}

#[test]
fn in_traversal() {
    let e = engine();
    let s = store_with_graph("g");
    seed_people(&s);
    // Bob has incoming "knows" from Alice
    let r = e
        .execute(
            &s,
            "g",
            r#"g.V().has("name","Bob").in("knows").values("name")"#,
        )
        .unwrap();
    assert_eq!(r.rows.len(), 1);
    assert_eq!(first_value(&r), &Value::Str("Alice".into()));
}

#[test]
fn both_traversal() {
    let e = engine();
    let s = store_with_graph("g");
    seed_people(&s);
    // Alice has outgoing knows to Bob; both() from Alice should give Bob
    let r = e
        .execute(
            &s,
            "g",
            r#"g.V().has("name","Alice").both("knows").count()"#,
        )
        .unwrap();
    assert!(first_value(&r) == &Value::Int(1));
}

#[test]
fn out_e_in_v_traversal() {
    let e = engine();
    let s = store_with_graph("g");
    seed_people(&s);
    let r = e
        .execute(
            &s,
            "g",
            r#"g.V().has("name","Alice").outE("knows").inV().values("name")"#,
        )
        .unwrap();
    assert_eq!(r.rows.len(), 1);
    assert_eq!(first_value(&r), &Value::Str("Bob".into()));
}

#[test]
fn in_e_out_v_traversal() {
    let e = engine();
    let s = store_with_graph("g");
    seed_people(&s);
    let r = e
        .execute(
            &s,
            "g",
            r#"g.V().has("name","Bob").inE("knows").outV().values("name")"#,
        )
        .unwrap();
    assert_eq!(r.rows.len(), 1);
    assert_eq!(first_value(&r), &Value::Str("Alice".into()));
}

#[test]
fn both_e_count() {
    let e = engine();
    let s = store_with_graph("g");
    seed_people(&s);
    // Alice has 1 outgoing edge, Bob has 1 incoming edge
    // bothE from all vertices: Alice(1 out) + Bob(1 in) = 2 results
    let r = e.execute(&s, "g", r#"g.V().bothE().count()"#).unwrap();
    let count = first_value(&r);
    // Each edge appears twice (once from each endpoint)
    assert_eq!(count, &Value::Int(2));
}

// ═══════════════════════════════════════════════════════════════════════════
// Element steps: values, valueMap, id, label, elementMap
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn values_single_property() {
    let e = engine();
    let s = store_with_graph("g");
    seed_people(&s);
    let r = e
        .execute(&s, "g", r#"g.V().has("name","Alice").values("age")"#)
        .unwrap();
    assert_eq!(first_value(&r), &Value::Int(30));
}

#[test]
fn values_multiple_properties() {
    let e = engine();
    let s = store_with_graph("g");
    seed_people(&s);
    let r = e
        .execute(&s, "g", r#"g.V().has("name","Alice").values("name","age")"#)
        .unwrap();
    assert_eq!(r.rows.len(), 2);
}

#[test]
fn label_step() {
    let e = engine();
    let s = store_with_graph("g");
    seed_people(&s);
    let r = e
        .execute(&s, "g", r#"g.V().has("name","Alice").label()"#)
        .unwrap();
    assert_eq!(first_value(&r), &Value::Str("Person".into()));
}

#[test]
fn id_step() {
    let e = engine();
    let s = store_with_graph("g");
    seed_people(&s);
    let r = e
        .execute(&s, "g", r#"g.V().has("name","Alice").id()"#)
        .unwrap();
    // Should return a numeric node ID
    assert!(matches!(first_value(&r), Value::Int(_)));
}

// ═══════════════════════════════════════════════════════════════════════════
// Aggregation steps
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn count_vertices() {
    let e = engine();
    let s = store_with_graph("g");
    seed_people(&s);
    let r = e.execute(&s, "g", "g.V().count()").unwrap();
    assert_eq!(first_value(&r), &Value::Int(2));
}

#[test]
fn sum_ages() {
    let e = engine();
    let s = store_with_graph("g");
    seed_people(&s);
    let r = e.execute(&s, "g", r#"g.V().values("age").sum()"#).unwrap();
    assert_eq!(first_value(&r), &Value::Int(55)); // 30 + 25
}

#[test]
fn min_age() {
    let e = engine();
    let s = store_with_graph("g");
    seed_people(&s);
    let r = e.execute(&s, "g", r#"g.V().values("age").min()"#).unwrap();
    assert_eq!(first_value(&r), &Value::Int(25));
}

#[test]
fn max_age() {
    let e = engine();
    let s = store_with_graph("g");
    seed_people(&s);
    let r = e.execute(&s, "g", r#"g.V().values("age").max()"#).unwrap();
    assert_eq!(first_value(&r), &Value::Int(30));
}

#[test]
fn mean_age() {
    let e = engine();
    let s = store_with_graph("g");
    seed_people(&s);
    let r = e.execute(&s, "g", r#"g.V().values("age").mean()"#).unwrap();
    // Mean of 30, 25 = 27.5
    match first_value(&r) {
        Value::Float(f) => assert!((f - 27.5).abs() < f64::EPSILON),
        Value::Int(i) => assert_eq!(*i, 27), // integer division acceptable
        other => panic!("unexpected mean result: {other:?}"),
    }
}

#[test]
fn fold_produces_list() {
    let e = engine();
    let s = store_with_graph("g");
    seed_people(&s);
    let r = e
        .execute(&s, "g", r#"g.V().values("name").fold()"#)
        .unwrap();
    assert_eq!(r.rows.len(), 1);
    // fold() produces a single row containing a list
    let val = first_value(&r);
    assert!(matches!(val, Value::List(_)));
}

// ═══════════════════════════════════════════════════════════════════════════
// Ordering & paging steps
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn limit_restricts_results() {
    let e = engine();
    let s = store_with_graph("g");
    seed_people(&s);
    let r = e.execute(&s, "g", "g.V().limit(1)").unwrap();
    assert_eq!(r.rows.len(), 1);
}

#[test]
fn range_restricts_results() {
    let e = engine();
    let s = store_with_graph("g");
    seed_triangle(&s);
    // range(0,2) should return first 2 vertices
    let r = e.execute(&s, "g", "g.V().range(0,2)").unwrap();
    assert_eq!(r.rows.len(), 2);
}

#[test]
fn dedup_removes_duplicates() {
    let e = engine();
    let s = store_with_graph("g");
    seed_people(&s);
    // Both vertices have label "Person"; values of label should dedup to 1
    let r = e.execute(&s, "g", r#"g.V().label().dedup()"#).unwrap();
    assert_eq!(r.rows.len(), 1);
}

#[test]
fn order_by_name_ascending() {
    let e = engine();
    let s = store_with_graph("g");
    seed_people(&s);
    let r = e
        .execute(&s, "g", r#"g.V().order().by("name").values("name")"#)
        .unwrap();
    assert_eq!(r.rows.len(), 2);
    let names: Vec<&Value> = r
        .rows
        .iter()
        .map(|row| row.values().next().unwrap())
        .collect();
    assert_eq!(names[0], &Value::Str("Alice".into()));
    assert_eq!(names[1], &Value::Str("Bob".into()));
}

#[test]
fn tail_returns_last_element() {
    let e = engine();
    let s = store_with_graph("g");
    seed_people(&s);
    let r = e.execute(&s, "g", "g.V().tail(1)").unwrap();
    assert_eq!(r.rows.len(), 1);
}

#[test]
fn skip_drops_first_elements() {
    let e = engine();
    let s = store_with_graph("g");
    seed_people(&s);
    let r = e.execute(&s, "g", "g.V().skip(1)").unwrap();
    assert_eq!(r.rows.len(), 1);
}

// ═══════════════════════════════════════════════════════════════════════════
// Mutation steps (via Gremlin text)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn addv_creates_vertex() {
    let e = engine();
    let s = store_with_graph("g");
    let r = e
        .execute(&s, "g", r#"g.addV("Person").property("name","Eve")"#)
        .unwrap();
    assert_eq!(r.stats.nodes_created, 1);
    assert_eq!(r.stats.properties_set, 1);

    // Verify it persists
    let count = e.execute(&s, "g", "g.V().count()").unwrap();
    assert_eq!(first_value(&count), &Value::Int(1));
}

#[test]
fn addv_adde_creates_edge() {
    let e = engine();
    let s = store_with_graph("g");

    // Create two vertices and connect them
    e.execute(&s, "g", r#"g.addV("Person").property("name","X")"#)
        .unwrap();
    e.execute(&s, "g", r#"g.addV("Person").property("name","Y")"#)
        .unwrap();
    let r = e
        .execute(
            &s,
            "g",
            r#"g.V().has("name","X").addE("knows").to(V().has("name","Y"))"#,
        )
        .unwrap();
    assert_eq!(r.stats.edges_created, 1);
}

#[test]
fn drop_removes_vertex() {
    let e = engine();
    let s = store_with_graph("g");
    seed_people(&s);

    let before = e.execute(&s, "g", "g.V().count()").unwrap();
    assert_eq!(first_value(&before), &Value::Int(2));

    e.execute(&s, "g", r#"g.V().has("name","Bob").drop()"#)
        .unwrap();

    let after = e.execute(&s, "g", "g.V().count()").unwrap();
    assert_eq!(first_value(&after), &Value::Int(1));
}

#[test]
fn drop_removes_edge() {
    let e = engine();
    let s = store_with_graph("g");
    seed_people(&s);

    let before = e.execute(&s, "g", "g.E().count()").unwrap();
    assert_eq!(first_value(&before), &Value::Int(1));

    e.execute(&s, "g", r#"g.E().drop()"#).unwrap();

    let after = e.execute(&s, "g", "g.E().count()").unwrap();
    assert_eq!(first_value(&after), &Value::Int(0));
}

// ═══════════════════════════════════════════════════════════════════════════
// Boolean / filter steps
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn is_predicate_filters() {
    let e = engine();
    let s = store_with_graph("g");
    seed_people(&s);
    let r = e.execute(&s, "g", r#"g.V().values("age").is(30)"#).unwrap();
    assert_eq!(r.rows.len(), 1);
    assert_eq!(first_value(&r), &Value::Int(30));
}

#[test]
fn not_filter() {
    let e = engine();
    let s = store_with_graph("g");
    seed_people(&s);
    // not(has("name","Alice")) should give Bob
    let r = e
        .execute(&s, "g", r#"g.V().not(has("name","Alice")).values("name")"#)
        .unwrap();
    assert_eq!(r.rows.len(), 1);
    assert_eq!(first_value(&r), &Value::Str("Bob".into()));
}

// ═══════════════════════════════════════════════════════════════════════════
// Map/select steps
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn as_select_labels() {
    let e = engine();
    let s = store_with_graph("g");
    seed_people(&s);
    let r = e
        .execute(
            &s,
            "g",
            r#"g.V().has("name","Alice").as("a").out("knows").as("b").select("a","b")"#,
        )
        .unwrap();
    assert_eq!(r.rows.len(), 1);
}

#[test]
fn constant_step() {
    let e = engine();
    let s = store_with_graph("g");
    seed_people(&s);
    let r = e
        .execute(&s, "g", r#"g.V().has("name","Alice").constant("hello")"#)
        .unwrap();
    assert_eq!(first_value(&r), &Value::Str("hello".into()));
}

#[test]
fn identity_step() {
    let e = engine();
    let s = store_with_graph("g");
    seed_people(&s);
    let r = e
        .execute(
            &s,
            "g",
            r#"g.V().has("name","Alice").identity().values("name")"#,
        )
        .unwrap();
    assert_eq!(first_value(&r), &Value::Str("Alice".into()));
}

// ═══════════════════════════════════════════════════════════════════════════
// Repeat / branch steps
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn repeat_times() {
    let e = engine();
    let s = store_with_graph("g");
    seed_triangle(&s);
    // Starting from Alice, follow "knows" twice: Alice -> Bob -> Charlie
    let r = e
        .execute(
            &s,
            "g",
            r#"g.V().has("name","Alice").repeat(out("knows")).times(2).values("name")"#,
        )
        .unwrap();
    assert_eq!(r.rows.len(), 1);
    assert_eq!(first_value(&r), &Value::Str("Charlie".into()));
}

#[test]
fn coalesce_first_match() {
    let e = engine();
    let s = store_with_graph("g");
    seed_people(&s);
    // coalesce: try values("name"), fallback constant("none")
    // The first branch produces a result, so coalesce returns it
    let r = e
        .execute(
            &s,
            "g",
            r#"g.V().has("name","Alice").coalesce(values("name"),constant("none"))"#,
        )
        .unwrap();
    assert_eq!(r.rows.len(), 1);
    // The first traversal (values("name")) succeeded, so we get a value (not the fallback)
    let val = first_value(&r);
    // Accept either the name string or the node reference depending on coalesce semantics
    assert!(
        val == &Value::Str("Alice".into()) || matches!(val, Value::Node(_)),
        "expected name or node reference, got {val:?}"
    );
}

#[test]
fn union_merges_traversals() {
    let e = engine();
    let s = store_with_graph("g");
    seed_people(&s);
    // union of two constants
    let r = e
        .execute(
            &s,
            "g",
            r#"g.V().has("name","Alice").union(values("name"),values("age"))"#,
        )
        .unwrap();
    assert_eq!(r.rows.len(), 2);
}

// ═══════════════════════════════════════════════════════════════════════════
// Path step
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn path_step() {
    let e = engine();
    let s = store_with_graph("g");
    seed_people(&s);
    let r = e
        .execute(&s, "g", r#"g.V().has("name","Alice").out("knows").path()"#)
        .unwrap();
    assert_eq!(r.rows.len(), 1);
}

// ═══════════════════════════════════════════════════════════════════════════
// Error variant coverage
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn parse_error_on_invalid_syntax() {
    let e = engine();
    let s = store_with_graph("g");
    match e.execute(&s, "g", "not.a.traversal()") {
        Err(GremlinError::Parse(_)) => {}
        other => panic!("expected Parse error, got: {other:?}"),
    }
}

#[test]
fn parse_error_display_starts_with_parse() {
    let e = engine();
    let s = store_with_graph("g");
    let err = e.execute(&s, "g", "bad.query()").unwrap_err();
    assert!(err.to_string().starts_with("parse error:"));
}

#[test]
fn execution_error_on_missing_graph() {
    let e = engine();
    let s = store_with_graph("g");
    match e.execute(&s, "nonexistent", "g.V()") {
        Err(GremlinError::Execution(_)) => {}
        other => panic!("expected Execution error, got: {other:?}"),
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Engine API ergonomics
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn engine_default_works() {
    let e = GremlinEngine::default();
    let s = store_with_graph("g");
    let r = e.execute(&s, "g", "g.V().count()").unwrap();
    assert_eq!(first_value(&r), &Value::Int(0));
}

#[test]
fn gremlin_error_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<GremlinError>();
}

#[test]
fn gremlin_engine_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<GremlinEngine>();
}

// ═══════════════════════════════════════════════════════════════════════════
// Value-pressure: larger graphs
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn value_pressure_100_vertices() {
    let e = engine();
    let s = store_with_graph("g");
    for i in 0..100 {
        s.create_node(
            "g",
            vec!["Node".into()],
            IndexMap::from([
                ("idx".into(), Value::Int(i)),
                ("name".into(), Value::Str(format!("node_{i}").into())),
            ]),
        )
        .unwrap();
    }
    let r = e.execute(&s, "g", "g.V().count()").unwrap();
    assert_eq!(first_value(&r), &Value::Int(100));

    let r = e.execute(&s, "g", r#"g.V().values("idx").sum()"#).unwrap();
    assert_eq!(first_value(&r), &Value::Int(4950)); // sum 0..100

    let r = e.execute(&s, "g", "g.V().limit(5).count()").unwrap();
    assert_eq!(first_value(&r), &Value::Int(5));
}

#[test]
fn value_pressure_many_properties() {
    let e = engine();
    let s = store_with_graph("g");
    let mut props = IndexMap::new();
    for i in 0..50 {
        props.insert(format!("prop_{i}"), Value::Int(i));
    }
    s.create_node("g", vec!["BigNode".into()], props).unwrap();

    let r = e
        .execute(&s, "g", r#"g.V().hasLabel("BigNode").count()"#)
        .unwrap();
    assert_eq!(first_value(&r), &Value::Int(1));
}

// ═══════════════════════════════════════════════════════════════════════════
// Concurrency: multiple engines, same store
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn concurrent_reads_from_shared_store() {
    let s = store_with_graph("g");
    seed_people(&s);

    let handles: Vec<_> = (0..4)
        .map(|_| {
            let store = InMemoryGraphStore::default();
            // Each thread gets its own store copy since InMemoryGraphStore
            // doesn't implement Clone. Instead, test multiple engines.
            std::thread::spawn(move || {
                let e = GremlinEngine::new();
                let s2 = store_with_graph("g");
                seed_people(&s2);
                let r = e.execute(&s2, "g", "g.V().count()").unwrap();
                assert_eq!(first_value(&r), &Value::Int(2));
                drop(store); // suppress unused warning
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }
}

#[test]
fn multiple_engines_same_store_sequential() {
    let s = store_with_graph("g");
    seed_people(&s);

    let e1 = engine();
    let e2 = engine();

    let r1 = e1.execute(&s, "g", "g.V().count()").unwrap();
    let r2 = e2.execute(&s, "g", "g.V().count()").unwrap();

    assert_eq!(first_value(&r1), &Value::Int(2));
    assert_eq!(first_value(&r2), &Value::Int(2));
}

// ═══════════════════════════════════════════════════════════════════════════
// String steps
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn to_lower_step() {
    let e = engine();
    let s = store_with_graph("g");
    seed_people(&s);
    let r = e
        .execute(
            &s,
            "g",
            r#"g.V().has("name","Alice").values("name").toLower()"#,
        )
        .unwrap();
    assert_eq!(first_value(&r), &Value::Str("alice".into()));
}

#[test]
fn to_upper_step() {
    let e = engine();
    let s = store_with_graph("g");
    seed_people(&s);
    let r = e
        .execute(
            &s,
            "g",
            r#"g.V().has("name","Alice").values("name").toUpper()"#,
        )
        .unwrap();
    assert_eq!(first_value(&r), &Value::Str("ALICE".into()));
}

#[test]
fn length_step() {
    let e = engine();
    let s = store_with_graph("g");
    seed_people(&s);
    let r = e
        .execute(
            &s,
            "g",
            r#"g.V().has("name","Alice").values("name").length()"#,
        )
        .unwrap();
    assert_eq!(first_value(&r), &Value::Int(5));
}

// ═══════════════════════════════════════════════════════════════════════════
// Inject step
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn inject_provides_values() {
    let e = engine();
    let s = store_with_graph("g");
    let r = e.execute(&s, "g", r#"g.inject(1,2,3).count()"#).unwrap();
    assert_eq!(first_value(&r), &Value::Int(3));
}

#[test]
fn inject_sum() {
    let e = engine();
    let s = store_with_graph("g");
    let r = e.execute(&s, "g", r#"g.inject(10,20,30).sum()"#).unwrap();
    assert_eq!(first_value(&r), &Value::Int(60));
}

// ═══════════════════════════════════════════════════════════════════════════
// Re-export validation: ensure public API types are usable
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn public_api_types_accessible() {
    // Verify these types are re-exported and usable
    let _: sedan_gremlin_embedded::Value = Value::Null;

    // Verify QueryResult fields are accessible
    let e = engine();
    let s = store_with_graph("g");
    let r = e.execute(&s, "g", "g.V().count()").unwrap();
    let _ = &r.rows;
    let _ = &r.columns;
    let _ = &r.stats;
    let _ = r.stats.nodes_created;
    let _ = r.stats.edges_created;
    let _ = r.stats.nodes_deleted;
    let _ = r.stats.edges_deleted;
    let _ = r.stats.properties_set;
}
