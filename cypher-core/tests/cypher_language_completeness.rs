//! OpenCypher language completeness tests.
//!
//! Systematically tests all major openCypher clauses, operators, and functions
//! to identify gaps in language support. Each test exercises one specific language
//! feature through the full pipeline: parse → plan → execute.
//!
//! Organized by openCypher specification sections:
//!  - Reading clauses: MATCH, OPTIONAL MATCH, WHERE
//!  - Projecting clauses: RETURN, WITH, UNWIND
//!  - Writing clauses: CREATE, MERGE, SET, DELETE, REMOVE, FOREACH
//!  - Sub-clauses: ORDER BY, SKIP, LIMIT, DISTINCT
//!  - Operators: comparison, arithmetic, string, list, boolean
//!  - Functions: aggregation, scalar, string, list, math, type predicates

use std::collections::HashMap;
use std::time::Duration;

use cypher_core::prelude::*;

// ── Helpers ──────────────────────────────────────────────────────────────────

fn make_store() -> InMemoryGraphStore {
    let store = InMemoryGraphStore::default();
    store.create_graph("test").unwrap();
    store
}

fn run(store: &InMemoryGraphStore, query: &str) -> Vec<HashMap<String, Value>> {
    let ast = parse_query(query).unwrap_or_else(|e| panic!("parse failed: {query}: {e}"));
    let planner = QueryPlanner::new(store, "test");
    let plan = planner
        .plan(&ast)
        .unwrap_or_else(|e| panic!("plan failed: {query}: {e}"));
    let mut budget = ExecutionBudget::new(Duration::from_secs(60), usize::MAX);
    QueryExecutor::new(store)
        .execute("test", &plan, &mut budget)
        .unwrap_or_else(|e| panic!("exec failed: {query}: {e}"))
        .rows
}

fn seed_social_graph(store: &InMemoryGraphStore) {
    run(store, "CREATE (a:Person {name: 'Alice', age: 30})-[:KNOWS {since: 2020}]->(b:Person {name: 'Bob', age: 25})");
    run(store, "MATCH (b:Person {name: 'Bob'}) CREATE (b)-[:KNOWS {since: 2021}]->(c:Person {name: 'Charlie', age: 35})");
    run(
        store,
        "MATCH (a:Person {name: 'Alice'}), (c:Person {name: 'Charlie'}) CREATE (a)-[:LIKES]->(c)",
    );
}

// ── Reading clauses ─────────────────────────────────────────────────────────

#[test]
fn match_all_nodes() {
    let store = make_store();
    run(&store, "CREATE (:A), (:B), (:C)");
    let rows = run(&store, "MATCH (n) RETURN count(n) AS c");
    assert_eq!(rows[0]["c"], Value::Int(3));
}

#[test]
fn match_by_label() {
    let store = make_store();
    run(
        &store,
        "CREATE (:Person {name: 'Alice'}), (:Animal {name: 'Rex'})",
    );
    let rows = run(&store, "MATCH (p:Person) RETURN p.name AS name");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], Value::Str("Alice".into()));
}

#[test]
fn match_by_property() {
    let store = make_store();
    run(&store, "CREATE (:N {v: 1}), (:N {v: 2}), (:N {v: 3})");
    let rows = run(&store, "MATCH (n:N {v: 2}) RETURN n.v AS v");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["v"], Value::Int(2));
}

#[test]
fn match_relationship_directed() {
    let store = make_store();
    seed_social_graph(&store);
    let rows = run(
        &store,
        "MATCH (a)-[:KNOWS]->(b) RETURN a.name AS from, b.name AS to ORDER BY from, to",
    );
    assert!(rows.len() >= 2);
}

#[test]
fn match_relationship_undirected() {
    let store = make_store();
    seed_social_graph(&store);
    let rows = run(
        &store,
        "MATCH (a:Person)-[:KNOWS]-(b:Person) RETURN a.name AS a, b.name AS b",
    );
    // Undirected should find both directions
    assert!(rows.len() >= 2);
}

#[test]
fn match_relationship_with_type_filter() {
    let store = make_store();
    seed_social_graph(&store);
    let rows = run(
        &store,
        "MATCH (a:Person)-[:LIKES]->(b:Person) RETURN a.name AS from, b.name AS to",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["from"], Value::Str("Alice".into()));
    assert_eq!(rows[0]["to"], Value::Str("Charlie".into()));
}

#[test]
fn match_multi_hop() {
    let store = make_store();
    seed_social_graph(&store);
    let rows = run(
        &store,
        "MATCH (a:Person {name: 'Alice'})-[:KNOWS]->(b)-[:KNOWS]->(c) RETURN c.name AS name",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], Value::Str("Charlie".into()));
}

#[test]
fn optional_match_with_missing() {
    let store = make_store();
    run(&store, "CREATE (:Person {name: 'Alice'})");
    let rows = run(&store, "MATCH (p:Person) OPTIONAL MATCH (p)-[:KNOWS]->(friend) RETURN p.name AS name, friend.name AS friend");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], Value::Str("Alice".into()));
    assert_eq!(rows[0]["friend"], Value::Null);
}

// ── WHERE clause ────────────────────────────────────────────────────────────

#[test]
fn where_comparison_operators() {
    let store = make_store();
    run(
        &store,
        "CREATE (:N {v: 1}), (:N {v: 2}), (:N {v: 3}), (:N {v: 4}), (:N {v: 5})",
    );

    let gt = run(&store, "MATCH (n:N) WHERE n.v > 3 RETURN count(n) AS c");
    assert_eq!(gt[0]["c"], Value::Int(2));

    let gte = run(&store, "MATCH (n:N) WHERE n.v >= 3 RETURN count(n) AS c");
    assert_eq!(gte[0]["c"], Value::Int(3));

    let lt = run(&store, "MATCH (n:N) WHERE n.v < 3 RETURN count(n) AS c");
    assert_eq!(lt[0]["c"], Value::Int(2));

    let lte = run(&store, "MATCH (n:N) WHERE n.v <= 3 RETURN count(n) AS c");
    assert_eq!(lte[0]["c"], Value::Int(3));

    let neq = run(&store, "MATCH (n:N) WHERE n.v <> 3 RETURN count(n) AS c");
    assert_eq!(neq[0]["c"], Value::Int(4));
}

#[test]
fn where_and_or_not() {
    let store = make_store();
    run(
        &store,
        "CREATE (:N {a: 1, b: true}), (:N {a: 2, b: false}), (:N {a: 3, b: true})",
    );
    let rows = run(
        &store,
        "MATCH (n:N) WHERE n.a > 1 AND n.b = true RETURN n.a AS a",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["a"], Value::Int(3));

    let rows = run(
        &store,
        "MATCH (n:N) WHERE n.a = 1 OR n.a = 3 RETURN count(n) AS c",
    );
    assert_eq!(rows[0]["c"], Value::Int(2));

    let rows = run(&store, "MATCH (n:N) WHERE NOT n.b RETURN count(n) AS c");
    assert_eq!(rows[0]["c"], Value::Int(1));
}

#[test]
fn where_in_list() {
    let store = make_store();
    run(&store, "CREATE (:N {v: 1}), (:N {v: 2}), (:N {v: 3})");
    let rows = run(
        &store,
        "MATCH (n:N) WHERE n.v IN [1, 3] RETURN count(n) AS c",
    );
    assert_eq!(rows[0]["c"], Value::Int(2));
}

#[test]
fn where_starts_with() {
    let store = make_store();
    run(
        &store,
        "CREATE (:N {s: 'hello world'}), (:N {s: 'goodbye'})",
    );
    let rows = run(
        &store,
        "MATCH (n:N) WHERE n.s STARTS WITH 'hello' RETURN n.s AS s",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["s"], Value::Str("hello world".into()));
}

#[test]
fn where_ends_with() {
    let store = make_store();
    run(
        &store,
        "CREATE (:N {s: 'hello world'}), (:N {s: 'goodbye'})",
    );
    let rows = run(
        &store,
        "MATCH (n:N) WHERE n.s ENDS WITH 'world' RETURN n.s AS s",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["s"], Value::Str("hello world".into()));
}

#[test]
fn where_contains() {
    let store = make_store();
    run(
        &store,
        "CREATE (:N {s: 'hello world'}), (:N {s: 'goodbye'})",
    );
    let rows = run(
        &store,
        "MATCH (n:N) WHERE n.s CONTAINS 'lo wo' RETURN n.s AS s",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["s"], Value::Str("hello world".into()));
}

#[test]
fn where_regex() {
    let store = make_store();
    run(&store, "CREATE (:N {s: 'abc123'}), (:N {s: 'xyz'})");
    // Use [0-9] instead of \d since Cypher string escaping intercepts \d
    let rows = run(
        &store,
        "MATCH (n:N) WHERE n.s =~ '.*[0-9]+.*' RETURN n.s AS s",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["s"], Value::Str("abc123".into()));
}

#[test]
fn where_exists_property() {
    let store = make_store();
    run(&store, "CREATE (:N {a: 1}), (:N {b: 2})");
    let rows = run(
        &store,
        "MATCH (n:N) WHERE n.a IS NOT NULL RETURN count(n) AS c",
    );
    assert_eq!(rows[0]["c"], Value::Int(1));
}

// ── Projecting clauses ──────────────────────────────────────────────────────

#[test]
fn return_with_alias() {
    let store = make_store();
    let rows = run(&store, "RETURN 42 AS answer");
    assert_eq!(rows[0]["answer"], Value::Int(42));
}

#[test]
fn return_star() {
    let store = make_store();
    run(&store, "CREATE (:N {v: 1})");
    let rows = run(&store, "MATCH (n:N) RETURN *");
    assert_eq!(rows.len(), 1);
}

#[test]
fn return_distinct() {
    let store = make_store();
    run(&store, "CREATE (:N {v: 1}), (:N {v: 1}), (:N {v: 2})");
    let rows = run(&store, "MATCH (n:N) RETURN DISTINCT n.v AS v ORDER BY v");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["v"], Value::Int(1));
    assert_eq!(rows[1]["v"], Value::Int(2));
}

#[test]
fn order_by_asc_and_desc() {
    let store = make_store();
    run(&store, "CREATE (:N {v: 3}), (:N {v: 1}), (:N {v: 2})");

    let asc = run(&store, "MATCH (n:N) RETURN n.v AS v ORDER BY v ASC");
    assert_eq!(asc[0]["v"], Value::Int(1));
    assert_eq!(asc[2]["v"], Value::Int(3));

    let desc = run(&store, "MATCH (n:N) RETURN n.v AS v ORDER BY v DESC");
    assert_eq!(desc[0]["v"], Value::Int(3));
    assert_eq!(desc[2]["v"], Value::Int(1));
}

#[test]
fn skip_and_limit() {
    let store = make_store();
    run(
        &store,
        "CREATE (:N {v: 1}), (:N {v: 2}), (:N {v: 3}), (:N {v: 4}), (:N {v: 5})",
    );
    let rows = run(
        &store,
        "MATCH (n:N) RETURN n.v AS v ORDER BY v SKIP 1 LIMIT 2",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["v"], Value::Int(2));
    assert_eq!(rows[1]["v"], Value::Int(3));
}

#[test]
fn with_clause_filtering() {
    let store = make_store();
    run(&store, "CREATE (:N {v: 1}), (:N {v: 2}), (:N {v: 3})");
    let rows = run(
        &store,
        "MATCH (n:N) WITH n.v AS v WHERE v > 1 RETURN v ORDER BY v",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["v"], Value::Int(2));
}

#[test]
fn with_aggregation() {
    let store = make_store();
    run(&store, "CREATE (:N {v: 1}), (:N {v: 2}), (:N {v: 3})");
    let rows = run(&store, "MATCH (n:N) WITH count(n) AS total RETURN total");
    assert_eq!(rows[0]["total"], Value::Int(3));
}

#[test]
fn unwind_list() {
    let store = make_store();
    let rows = run(&store, "UNWIND [1, 2, 3] AS x RETURN x");
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["x"], Value::Int(1));
    assert_eq!(rows[2]["x"], Value::Int(3));
}

#[test]
fn unwind_empty_list() {
    let store = make_store();
    let rows = run(&store, "UNWIND [] AS x RETURN x");
    assert_eq!(rows.len(), 0);
}

// ── Writing clauses ─────────────────────────────────────────────────────────

#[test]
fn create_node_with_properties() {
    let store = make_store();
    run(&store, "CREATE (:Person {name: 'Alice', age: 30})");
    let rows = run(
        &store,
        "MATCH (p:Person) RETURN p.name AS name, p.age AS age",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], Value::Str("Alice".into()));
    assert_eq!(rows[0]["age"], Value::Int(30));
}

#[test]
fn create_relationship() {
    let store = make_store();
    run(
        &store,
        "CREATE (a:Person {name: 'A'})-[:KNOWS {year: 2024}]->(b:Person {name: 'B'})",
    );
    let rows = run(&store, "MATCH (a)-[r:KNOWS]->(b) RETURN r.year AS year");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["year"], Value::Int(2024));
}

#[test]
fn create_multiple_in_one_query() {
    let store = make_store();
    run(&store, "CREATE (:N {v: 1}), (:N {v: 2}), (:N {v: 3})");
    let rows = run(&store, "MATCH (n:N) RETURN count(n) AS c");
    assert_eq!(rows[0]["c"], Value::Int(3));
}

#[test]
fn set_property() {
    let store = make_store();
    run(&store, "CREATE (:N {v: 1})");
    run(&store, "MATCH (n:N) SET n.v = 99");
    let rows = run(&store, "MATCH (n:N) RETURN n.v AS v");
    assert_eq!(rows[0]["v"], Value::Int(99));
}

#[test]
fn set_add_label() {
    let store = make_store();
    run(&store, "CREATE (:Person {name: 'Alice'})");
    run(&store, "MATCH (p:Person) SET p:Employee");
    let rows = run(&store, "MATCH (e:Employee) RETURN e.name AS name");
    assert_eq!(rows.len(), 1);
}

#[test]
fn remove_property() {
    let store = make_store();
    run(&store, "CREATE (:N {a: 1, b: 2})");
    run(&store, "MATCH (n:N) REMOVE n.a");
    let rows = run(&store, "MATCH (n:N) RETURN n.a AS a, n.b AS b");
    assert_eq!(rows[0]["a"], Value::Null);
    assert_eq!(rows[0]["b"], Value::Int(2));
}

#[test]
fn remove_label() {
    let store = make_store();
    run(&store, "CREATE (:A:B {v: 1})");
    run(&store, "MATCH (n:A:B) REMOVE n:B");
    let rows_a = run(&store, "MATCH (n:A) RETURN count(n) AS c");
    let rows_b = run(&store, "MATCH (n:B) RETURN count(n) AS c");
    assert_eq!(rows_a[0]["c"], Value::Int(1));
    assert_eq!(rows_b[0]["c"], Value::Int(0));
}

#[test]
fn delete_node() {
    let store = make_store();
    run(&store, "CREATE (:N {v: 1})");
    run(&store, "MATCH (n:N) DELETE n");
    let rows = run(&store, "MATCH (n:N) RETURN count(n) AS c");
    assert_eq!(rows[0]["c"], Value::Int(0));
}

#[test]
fn detach_delete() {
    let store = make_store();
    run(
        &store,
        "CREATE (a:Person {name: 'A'})-[:KNOWS]->(b:Person {name: 'B'})",
    );
    run(&store, "MATCH (a:Person {name: 'A'}) DETACH DELETE a");
    let rows = run(&store, "MATCH (n:Person) RETURN count(n) AS c");
    assert_eq!(rows[0]["c"], Value::Int(1)); // Only B remains
}

#[test]
fn foreach_set() {
    let store = make_store();
    run(&store, "CREATE (:N {v: 1}), (:N {v: 2})");
    run(
        &store,
        "MATCH (n:N) WITH collect(n) AS nodes FOREACH (x IN nodes | SET x.touched = true)",
    );
    let rows = run(
        &store,
        "MATCH (n:N) WHERE n.touched = true RETURN count(n) AS c",
    );
    assert_eq!(rows[0]["c"], Value::Int(2));
}

// ── Aggregation functions ───────────────────────────────────────────────────

#[test]
fn aggregation_count() {
    let store = make_store();
    run(&store, "CREATE (:N {v: 1}), (:N {v: 2}), (:N {v: 3})");
    let rows = run(&store, "MATCH (n:N) RETURN count(n) AS c");
    assert_eq!(rows[0]["c"], Value::Int(3));
}

#[test]
fn aggregation_sum() {
    let store = make_store();
    run(&store, "CREATE (:N {v: 10}), (:N {v: 20}), (:N {v: 30})");
    let rows = run(&store, "MATCH (n:N) RETURN sum(n.v) AS s");
    assert_eq!(rows[0]["s"], Value::Int(60));
}

#[test]
fn aggregation_avg() {
    let store = make_store();
    run(&store, "CREATE (:N {v: 10}), (:N {v: 20}), (:N {v: 30})");
    let rows = run(&store, "MATCH (n:N) RETURN avg(n.v) AS a");
    match &rows[0]["a"] {
        Value::Float(f) => assert!((*f - 20.0).abs() < 1e-9),
        Value::Int(i) => assert_eq!(*i, 20),
        other => panic!("expected Float or Int for avg, got {other:?}"),
    }
}

#[test]
fn aggregation_min_max() {
    let store = make_store();
    run(&store, "CREATE (:N {v: 5}), (:N {v: 1}), (:N {v: 9})");
    let rows = run(&store, "MATCH (n:N) RETURN min(n.v) AS lo, max(n.v) AS hi");
    assert_eq!(rows[0]["lo"], Value::Int(1));
    assert_eq!(rows[0]["hi"], Value::Int(9));
}

#[test]
fn aggregation_collect() {
    let store = make_store();
    run(&store, "CREATE (:N {v: 1}), (:N {v: 2}), (:N {v: 3})");
    let rows = run(&store, "MATCH (n:N) RETURN collect(n.v) AS vals");
    match &rows[0]["vals"] {
        Value::List(l) => assert_eq!(l.len(), 3),
        other => panic!("expected List for collect, got {other:?}"),
    }
}

#[test]
fn count_star() {
    let store = make_store();
    run(&store, "CREATE (:N), (:N), (:N)");
    let rows = run(&store, "MATCH (n:N) RETURN count(*) AS c");
    assert_eq!(rows[0]["c"], Value::Int(3));
}

// ── Scalar functions ────────────────────────────────────────────────────────

#[test]
fn function_id() {
    let store = make_store();
    run(&store, "CREATE (:N {v: 1})");
    let rows = run(&store, "MATCH (n:N) RETURN id(n) AS nid");
    match &rows[0]["nid"] {
        Value::Int(id) => assert!(*id >= 0),
        other => panic!("expected Int for id(), got {other:?}"),
    }
}

#[test]
fn function_labels() {
    let store = make_store();
    run(&store, "CREATE (:Person:Employee {name: 'Alice'})");
    let rows = run(&store, "MATCH (n:Person) RETURN labels(n) AS l");
    match &rows[0]["l"] {
        Value::List(labels) => {
            let strs: Vec<_> = labels
                .iter()
                .map(|v| {
                    if let Value::Str(s) = v {
                        s.to_string()
                    } else {
                        panic!("expected Str")
                    }
                })
                .collect();
            assert!(strs.contains(&"Person".to_string()));
            assert!(strs.contains(&"Employee".to_string()));
        }
        other => panic!("expected List for labels(), got {other:?}"),
    }
}

#[test]
fn function_type_of_relationship() {
    let store = make_store();
    run(&store, "CREATE (:A)-[:KNOWS]->(:B)");
    let rows = run(&store, "MATCH ()-[r]->() RETURN type(r) AS t");
    assert_eq!(rows[0]["t"], Value::Str("KNOWS".into()));
}

#[test]
fn function_coalesce() {
    let store = make_store();
    let rows = run(&store, "RETURN coalesce(null, null, 42, 99) AS v");
    assert_eq!(rows[0]["v"], Value::Int(42));
}

#[test]
fn function_keys() {
    let store = make_store();
    run(&store, "CREATE (:N {a: 1, b: 2})");
    let rows = run(&store, "MATCH (n:N) RETURN keys(n) AS k");
    match &rows[0]["k"] {
        Value::List(keys) => assert!(keys.len() >= 2),
        other => panic!("expected List for keys(), got {other:?}"),
    }
}

#[test]
fn function_properties() {
    let store = make_store();
    run(&store, "CREATE (:N {a: 1, b: 'two'})");
    let rows = run(&store, "MATCH (n:N) RETURN properties(n) AS p");
    match &rows[0]["p"] {
        Value::Map(m) => {
            assert_eq!(m.get("a"), Some(&Value::Int(1)));
            assert_eq!(m.get("b"), Some(&Value::Str("two".into())));
        }
        other => panic!("expected Map for properties(), got {other:?}"),
    }
}

// ── String functions ────────────────────────────────────────────────────────

#[test]
fn function_trim() {
    let store = make_store();
    let rows = run(&store, "RETURN trim('  hello  ') AS s");
    assert_eq!(rows[0]["s"], Value::Str("hello".into()));
}

#[test]
fn function_left_right() {
    let store = make_store();
    let rows = run(
        &store,
        "RETURN left('hello', 3) AS l, right('hello', 3) AS r",
    );
    assert_eq!(rows[0]["l"], Value::Str("hel".into()));
    assert_eq!(rows[0]["r"], Value::Str("llo".into()));
}

#[test]
fn function_substring() {
    let store = make_store();
    let rows = run(&store, "RETURN substring('hello world', 6, 5) AS s");
    assert_eq!(rows[0]["s"], Value::Str("world".into()));
}

#[test]
fn function_replace() {
    let store = make_store();
    let rows = run(
        &store,
        "RETURN replace('hello world', 'world', 'rust') AS s",
    );
    assert_eq!(rows[0]["s"], Value::Str("hello rust".into()));
}

#[test]
fn function_split() {
    let store = make_store();
    let rows = run(&store, "RETURN split('a,b,c', ',') AS parts");
    assert_eq!(
        rows[0]["parts"],
        Value::List(vec![
            Value::Str("a".into()),
            Value::Str("b".into()),
            Value::Str("c".into()),
        ])
    );
}

#[test]
fn function_reverse_string() {
    let store = make_store();
    let rows = run(&store, "RETURN reverse('hello') AS s");
    assert_eq!(rows[0]["s"], Value::Str("olleh".into()));
}

#[test]
fn function_to_string() {
    let store = make_store();
    let rows = run(&store, "RETURN toString(42) AS s");
    assert_eq!(rows[0]["s"], Value::Str("42".into()));
}

// ── Math functions ──────────────────────────────────────────────────────────

#[test]
fn function_abs() {
    let store = make_store();
    let rows = run(&store, "RETURN abs(-42) AS v");
    assert_eq!(rows[0]["v"], Value::Int(42));
}

#[test]
fn function_ceil_floor_round() {
    let store = make_store();
    let rows = run(
        &store,
        "RETURN ceil(2.3) AS c, floor(2.7) AS f, round(2.5) AS r",
    );
    match (&rows[0]["c"], &rows[0]["f"]) {
        (Value::Float(c), Value::Float(f)) => {
            assert!((*c - 3.0).abs() < 1e-9);
            assert!((*f - 2.0).abs() < 1e-9);
        }
        (Value::Int(c), Value::Int(f)) => {
            assert_eq!(*c, 3);
            assert_eq!(*f, 2);
        }
        other => panic!("unexpected types for ceil/floor: {other:?}"),
    }
}

#[test]
fn function_sign() {
    let store = make_store();
    let rows = run(
        &store,
        "RETURN sign(-5) AS neg, sign(0) AS zero, sign(5) AS pos",
    );
    assert_eq!(rows[0]["neg"], Value::Int(-1));
    assert_eq!(rows[0]["zero"], Value::Int(0));
    assert_eq!(rows[0]["pos"], Value::Int(1));
}

#[test]
fn function_to_integer() {
    let store = make_store();
    let rows = run(&store, "RETURN toInteger(3.14) AS v");
    assert_eq!(rows[0]["v"], Value::Int(3));
}

#[test]
fn function_to_float() {
    let store = make_store();
    let rows = run(&store, "RETURN toFloat(42) AS v");
    match &rows[0]["v"] {
        Value::Float(f) => assert!((*f - 42.0).abs() < 1e-9),
        other => panic!("expected Float for toFloat, got {other:?}"),
    }
}

// ── List functions ──────────────────────────────────────────────────────────

#[test]
fn function_range() {
    let store = make_store();
    let rows = run(&store, "RETURN range(1, 5) AS r");
    assert_eq!(
        rows[0]["r"],
        Value::List(vec![
            Value::Int(1),
            Value::Int(2),
            Value::Int(3),
            Value::Int(4),
            Value::Int(5)
        ])
    );
}

#[test]
fn function_head_last_tail() {
    let store = make_store();
    let rows = run(
        &store,
        "RETURN head([1,2,3]) AS h, last([1,2,3]) AS l, tail([1,2,3]) AS t",
    );
    assert_eq!(rows[0]["h"], Value::Int(1));
    assert_eq!(rows[0]["l"], Value::Int(3));
    assert_eq!(
        rows[0]["t"],
        Value::List(vec![Value::Int(2), Value::Int(3)])
    );
}

#[test]
fn function_reverse_list() {
    let store = make_store();
    let rows = run(&store, "RETURN reverse([1, 2, 3]) AS r");
    assert_eq!(
        rows[0]["r"],
        Value::List(vec![Value::Int(3), Value::Int(2), Value::Int(1)])
    );
}

// ── List comprehension and UNWIND ───────────────────────────────────────────

#[test]
fn list_comprehension() {
    let store = make_store();
    let rows = run(&store, "RETURN [x IN [1, 2, 3, 4] WHERE x > 2] AS filtered");
    assert_eq!(
        rows[0]["filtered"],
        Value::List(vec![Value::Int(3), Value::Int(4)])
    );
}

#[test]
fn list_comprehension_with_transform() {
    let store = make_store();
    let rows = run(&store, "RETURN [x IN [1, 2, 3] | x * 2] AS doubled");
    assert_eq!(
        rows[0]["doubled"],
        Value::List(vec![Value::Int(2), Value::Int(4), Value::Int(6)])
    );
}

// ── Variable-length paths ───────────────────────────────────────────────────

#[test]
fn varlen_path_fixed_length() {
    let store = make_store();
    seed_social_graph(&store);
    let rows = run(
        &store,
        "MATCH (a:Person {name: 'Alice'})-[:KNOWS*2]->(c) RETURN c.name AS name",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], Value::Str("Charlie".into()));
}

#[test]
fn varlen_path_range() {
    let store = make_store();
    seed_social_graph(&store);
    let rows = run(
        &store,
        "MATCH (a:Person {name: 'Alice'})-[:KNOWS*1..2]->(b) RETURN b.name AS name ORDER BY name",
    );
    assert!(
        rows.len() >= 2,
        "should find Bob (1 hop) and Charlie (2 hops)"
    );
}

// ── UNION ───────────────────────────────────────────────────────────────────

#[test]
fn union_all() {
    let store = make_store();
    let rows = run(&store, "RETURN 1 AS n UNION ALL RETURN 1 AS n");
    assert_eq!(rows.len(), 2);
}

#[test]
fn union_distinct() {
    let store = make_store();
    let rows = run(&store, "RETURN 1 AS n UNION RETURN 1 AS n");
    assert_eq!(rows.len(), 1);
}

// ── Arithmetic operators ────────────────────────────────────────────────────

#[test]
fn arithmetic_operators() {
    let store = make_store();
    let rows = run(
        &store,
        "RETURN 10 + 3 AS add, 10 - 3 AS sub, 10 * 3 AS mul, 10 / 3 AS div, 10 % 3 AS modulo",
    );
    assert_eq!(rows[0]["add"], Value::Int(13));
    assert_eq!(rows[0]["sub"], Value::Int(7));
    assert_eq!(rows[0]["mul"], Value::Int(30));
    assert_eq!(rows[0]["div"], Value::Int(3)); // integer division
    assert_eq!(rows[0]["modulo"], Value::Int(1));
}

#[test]
fn unary_minus() {
    let store = make_store();
    let rows = run(&store, "RETURN -(5) AS neg");
    assert_eq!(rows[0]["neg"], Value::Int(-5));
}

// ── Existential subqueries / EXISTS patterns ────────────────────────────────

#[test]
fn exists_pattern_in_where() {
    let store = make_store();
    seed_social_graph(&store);
    let rows = run(
        &store,
        "MATCH (p:Person) WHERE EXISTS { (p)-[:KNOWS]->() } RETURN p.name AS name ORDER BY name",
    );
    // Alice and Bob both know someone
    assert!(rows.len() >= 2);
}

// ── Multiple MATCH clauses ──────────────────────────────────────────────────

#[test]
fn multiple_match_clauses() {
    let store = make_store();
    run(&store, "CREATE (:A {v: 1}), (:B {v: 2})");
    let rows = run(
        &store,
        "MATCH (a:A) MATCH (b:B) RETURN a.v AS av, b.v AS bv",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["av"], Value::Int(1));
    assert_eq!(rows[0]["bv"], Value::Int(2));
}

// ── MERGE with relationships ────────────────────────────────────────────────

#[test]
fn merge_creates_relationship() {
    let store = make_store();
    run(
        &store,
        "CREATE (:Person {name: 'Alice'}), (:Person {name: 'Bob'})",
    );
    run(
        &store,
        "MATCH (a:Person {name: 'Alice'}), (b:Person {name: 'Bob'}) MERGE (a)-[:KNOWS]->(b)",
    );
    let rows = run(
        &store,
        "MATCH (a)-[:KNOWS]->(b) RETURN a.name AS from, b.name AS to",
    );
    assert_eq!(rows.len(), 1);
}
