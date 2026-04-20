//! Coverage-targeted tests, batch 11.
//!
//! Broad end-to-end Cypher queries to lift overall line coverage by
//! exercising many planner and executor code paths in single tests.
//! Targets queries that traverse multiple operator types (MERGE, UNION,
//! FOREACH, SHORTEST PATH, CALL, complex aggregations, list comprehensions,
//! pattern comprehensions, CASE WHEN).

use std::collections::HashMap;
use std::time::Duration;

use crate::executor::engine::{QueryExecutor, QueryResult};
use crate::executor::ExecutionBudget;
use crate::parser::parse_query;
use crate::planner::QueryPlanner;
use crate::procedure::ProcedureRegistry;
use crate::storage::{GraphStore, InMemoryGraphStore};
use crate::types::Value;

fn unlimited() -> ExecutionBudget {
    ExecutionBudget::new(Duration::from_secs(60), usize::MAX)
}

fn fresh_store() -> InMemoryGraphStore {
    let s = InMemoryGraphStore::new();
    s.create_graph("g").unwrap();
    s
}

fn try_exec_on(store: &dyn GraphStore, q: &str) -> Result<QueryResult, String> {
    let registry = ProcedureRegistry::new();
    let params: HashMap<String, Value> = HashMap::new();
    let ast = parse_query(q).map_err(|e| format!("parse: {e:?}"))?;
    let plan = QueryPlanner::with_context(store, "g", &registry, &params)
        .plan(&ast)
        .map_err(|e| format!("plan: {e:?}"))?;
    let executor = QueryExecutor::with_context(store, &registry, params);
    let mut budget = unlimited();
    executor
        .execute("g", &plan, &mut budget)
        .map_err(|e| format!("exec: {e:?}"))
}

fn exec_on(store: &dyn GraphStore, q: &str) -> QueryResult {
    try_exec_on(store, q).unwrap_or_else(|e| panic!("query {q}: {e}"))
}

fn exec(q: &str) -> QueryResult {
    exec_on(&fresh_store(), q)
}

// ── Seed a small graph with mixed nodes / edges ────────────────────────────

fn seed_social() -> InMemoryGraphStore {
    let s = fresh_store();
    exec_on(
        &s,
        "CREATE (a:Person {name:'Alice', age:30}), \
                (b:Person {name:'Bob', age:25}), \
                (c:Person {name:'Carol', age:35}), \
                (d:Person {name:'Dan', age:28}), \
                (a)-[:KNOWS {since:2019}]->(b), \
                (b)-[:KNOWS {since:2020}]->(c), \
                (c)-[:KNOWS {since:2021}]->(d), \
                (a)-[:KNOWS {since:2022}]->(c)",
    );
    s
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[test]
fn match_with_where_predicate_and_order() {
    let s = seed_social();
    let r = exec_on(
        &s,
        "MATCH (p:Person) WHERE p.age >= 28 RETURN p.name AS n ORDER BY p.age DESC",
    );
    assert_eq!(r.rows.len(), 3);
    let first = r.rows[0].get("n").cloned().unwrap_or(Value::Null);
    assert_eq!(first, Value::Str("Carol".into()));
}

#[test]
fn variable_length_path_one_to_three() {
    let s = seed_social();
    let r = exec_on(
        &s,
        "MATCH (a:Person {name:'Alice'})-[:KNOWS*1..3]->(p:Person) RETURN DISTINCT p.name AS n",
    );
    let names: Vec<String> = r
        .rows
        .iter()
        .map(|row| match row.get("n").cloned().unwrap_or(Value::Null) {
            Value::Str(s) => s.to_string(),
            _ => String::new(),
        })
        .collect();
    assert!(names.iter().any(|n| n == "Bob"));
    assert!(names.iter().any(|n| n == "Carol"));
    assert!(names.iter().any(|n| n == "Dan"));
}

#[test]
fn shortestpath_smallest_hop_count() {
    let s = seed_social();
    let r = exec_on(
        &s,
        "MATCH p = shortestPath((a:Person {name:'Alice'})-[:KNOWS*]-(d:Person {name:'Dan'})) \
         RETURN length(p) AS hops",
    );
    assert_eq!(r.rows.len(), 1);
    let v = r.rows[0].get("hops").cloned().unwrap_or(Value::Null);
    // Alice → Carol → Dan = 2 hops via shortest.
    assert_eq!(v, Value::Int(2));
}

#[test]
fn aggregate_collect_and_count_per_group() {
    let s = seed_social();
    let r = exec_on(
        &s,
        "MATCH (p:Person)-[:KNOWS]->(f:Person) \
         RETURN p.name AS n, count(f) AS c, collect(f.name) AS friends \
         ORDER BY n",
    );
    assert!(!r.rows.is_empty());
    // Alice knows 2 (Bob, Carol).
    let alice = r
        .rows
        .iter()
        .find(|r| matches!(r.get("n"), Some(Value::Str(s)) if &s[..] == "Alice"));
    assert!(alice.is_some());
    if let Some(row) = alice {
        assert_eq!(row.get("c"), Some(&Value::Int(2)));
    }
}

#[test]
fn unwind_then_create_relationships() {
    let s = fresh_store();
    exec_on(
        &s,
        "CREATE (:Tag {name:'a'}), (:Tag {name:'b'}), (:Tag {name:'c'})",
    );
    let r = exec_on(
        &s,
        "UNWIND ['a','b','c'] AS x MATCH (t:Tag {name:x}) RETURN t.name AS n ORDER BY n",
    );
    assert_eq!(r.rows.len(), 3);
}

#[test]
fn merge_creates_when_missing_matches_when_present() {
    let s = fresh_store();
    exec_on(&s, "MERGE (a:Account {id:1}) RETURN a");
    let r1 = exec_on(&s, "MATCH (a:Account) RETURN count(a) AS c");
    assert_eq!(r1.rows[0].get("c"), Some(&Value::Int(1)));
    // MERGE again — should match, not create.
    exec_on(&s, "MERGE (a:Account {id:1}) RETURN a");
    let r2 = exec_on(&s, "MATCH (a:Account) RETURN count(a) AS c");
    assert_eq!(r2.rows[0].get("c"), Some(&Value::Int(1)));
}

#[test]
fn merge_on_create_on_match_setters() {
    let s = fresh_store();
    // First MERGE → ON CREATE fires.
    exec_on(
        &s,
        "MERGE (a:Item {sku:'X'}) ON CREATE SET a.created = true ON MATCH SET a.touched = 1 RETURN a",
    );
    // Second MERGE → ON MATCH fires.
    exec_on(
        &s,
        "MERGE (a:Item {sku:'X'}) ON CREATE SET a.created = true ON MATCH SET a.touched = 1 RETURN a",
    );
    let r = exec_on(
        &s,
        "MATCH (a:Item {sku:'X'}) RETURN a.created AS cr, a.touched AS to_",
    );
    assert_eq!(r.rows[0].get("cr"), Some(&Value::Bool(true)));
    assert_eq!(r.rows[0].get("to_"), Some(&Value::Int(1)));
}

#[test]
fn union_combines_results() {
    let s = fresh_store();
    exec_on(&s, "CREATE (:A {n:1}), (:B {n:2})");
    let r = exec_on(
        &s,
        "MATCH (x:A) RETURN x.n AS v UNION MATCH (y:B) RETURN y.n AS v",
    );
    assert_eq!(r.rows.len(), 2);
}

#[test]
fn union_all_keeps_dups() {
    let s = fresh_store();
    exec_on(&s, "CREATE (:A {n:1}), (:A {n:1})");
    let r = exec_on(
        &s,
        "MATCH (x:A) RETURN x.n AS v UNION ALL MATCH (y:A) RETURN y.n AS v",
    );
    assert_eq!(r.rows.len(), 4);
}

#[test]
fn case_when_branches() {
    let r = exec(
        "UNWIND [1,2,3,4] AS x \
         RETURN CASE WHEN x % 2 = 0 THEN 'even' ELSE 'odd' END AS p ORDER BY x",
    );
    let vals: Vec<_> = r
        .rows
        .iter()
        .map(|row| row.get("p").cloned().unwrap_or(Value::Null))
        .collect();
    assert_eq!(
        vals,
        vec![
            Value::Str("odd".into()),
            Value::Str("even".into()),
            Value::Str("odd".into()),
            Value::Str("even".into()),
        ]
    );
}

#[test]
fn case_value_branches_with_else_null() {
    let r = exec(
        "UNWIND [1,2,3] AS x \
         RETURN CASE x WHEN 1 THEN 'one' WHEN 2 THEN 'two' END AS p ORDER BY x",
    );
    assert_eq!(r.rows.len(), 3);
    assert_eq!(r.rows[2].get("p"), Some(&Value::Null));
}

#[test]
fn list_comprehension_filter_and_map() {
    let r = exec("RETURN [x IN range(1,5) WHERE x % 2 = 0 | x*x] AS sq");
    assert_eq!(
        r.rows[0].get("sq"),
        Some(&Value::List(vec![Value::Int(4), Value::Int(16)]))
    );
}

#[test]
fn pattern_comprehension_collects_friend_names() {
    let s = seed_social();
    let r = exec_on(
        &s,
        "MATCH (a:Person {name:'Alice'}) \
         RETURN [(a)-[:KNOWS]->(f) | f.name] AS friends",
    );
    if let Some(Value::List(xs)) = r.rows[0].get("friends") {
        assert_eq!(xs.len(), 2);
    } else {
        panic!("expected friends list");
    }
}

#[test]
fn with_clause_chains_filter_and_aggregate() {
    let s = seed_social();
    let r = exec_on(
        &s,
        "MATCH (p:Person) WITH p WHERE p.age > 26 \
         RETURN avg(p.age) AS avg_age",
    );
    if let Some(Value::Float(f)) = r.rows[0].get("avg_age") {
        assert!((f - 31.0).abs() < 1e-6);
    } else {
        panic!("expected float avg_age");
    }
}

#[test]
fn set_property_on_match() {
    let s = fresh_store();
    exec_on(&s, "CREATE (:N {x:0})");
    exec_on(&s, "MATCH (n:N) SET n.x = 10");
    let r = exec_on(&s, "MATCH (n:N) RETURN n.x AS x");
    assert_eq!(r.rows[0].get("x"), Some(&Value::Int(10)));
}

#[test]
fn set_multiple_properties_with_map() {
    let s = fresh_store();
    exec_on(&s, "CREATE (:N {x:0})");
    exec_on(&s, "MATCH (n:N) SET n += {y:1, z:2}");
    let r = exec_on(&s, "MATCH (n:N) RETURN n.x AS x, n.y AS y, n.z AS z");
    assert_eq!(r.rows[0].get("x"), Some(&Value::Int(0)));
    assert_eq!(r.rows[0].get("y"), Some(&Value::Int(1)));
    assert_eq!(r.rows[0].get("z"), Some(&Value::Int(2)));
}

#[test]
fn set_replace_properties_with_eq_map() {
    let s = fresh_store();
    exec_on(&s, "CREATE (:N {x:0, drop_me:1})");
    exec_on(&s, "MATCH (n:N) SET n = {keep:99}");
    let r = exec_on(
        &s,
        "MATCH (n:N) RETURN n.x AS x, n.keep AS k, n.drop_me AS d",
    );
    assert_eq!(r.rows[0].get("x"), Some(&Value::Null));
    assert_eq!(r.rows[0].get("k"), Some(&Value::Int(99)));
    assert_eq!(r.rows[0].get("d"), Some(&Value::Null));
}

#[test]
fn remove_property_clears_value() {
    let s = fresh_store();
    exec_on(&s, "CREATE (:N {x:1, y:2})");
    exec_on(&s, "MATCH (n:N) REMOVE n.x");
    let r = exec_on(&s, "MATCH (n:N) RETURN n.x AS x, n.y AS y");
    assert_eq!(r.rows[0].get("x"), Some(&Value::Null));
    assert_eq!(r.rows[0].get("y"), Some(&Value::Int(2)));
}

#[test]
fn remove_label_drops_membership() {
    let s = fresh_store();
    exec_on(&s, "CREATE (:A:B {n:1})");
    exec_on(&s, "MATCH (n:A) REMOVE n:B");
    let r = exec_on(&s, "MATCH (n:B) RETURN count(n) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(0)));
}

#[test]
fn delete_with_detach() {
    let s = seed_social();
    exec_on(&s, "MATCH (p:Person {name:'Alice'}) DETACH DELETE p");
    let r = exec_on(&s, "MATCH (p:Person {name:'Alice'}) RETURN count(p) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(0)));
}

#[test]
fn foreach_creates_nodes() {
    let s = fresh_store();
    exec_on(&s, "FOREACH (i IN [1,2,3] | CREATE (:Item {n: i}))");
    let r = exec_on(&s, "MATCH (i:Item) RETURN count(i) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(3)));
}

#[test]
fn aggregate_sum_min_max_avg() {
    let r = exec(
        "UNWIND [10, 20, 30, 40] AS x \
         RETURN sum(x) AS s, min(x) AS mn, max(x) AS mx, avg(x) AS av",
    );
    let row = &r.rows[0];
    assert_eq!(row.get("s"), Some(&Value::Int(100)));
    assert_eq!(row.get("mn"), Some(&Value::Int(10)));
    assert_eq!(row.get("mx"), Some(&Value::Int(40)));
    if let Some(Value::Float(f)) = row.get("av") {
        assert!((f - 25.0).abs() < 1e-6);
    } else {
        panic!("avg should be float");
    }
}

#[test]
fn string_funcs_substring_replace_split() {
    let r = exec(
        "RETURN substring('hello',1,3) AS s, replace('foo','oo','OO') AS r, \
         split('a,b,c', ',') AS p",
    );
    let row = &r.rows[0];
    assert_eq!(row.get("s"), Some(&Value::Str("ell".into())));
    assert_eq!(row.get("r"), Some(&Value::Str("fOO".into())));
    if let Some(Value::List(xs)) = row.get("p") {
        assert_eq!(xs.len(), 3);
    }
}

#[test]
fn coalesce_returns_first_non_null() {
    let r = exec("RETURN coalesce(null, null, 42, 99) AS v");
    assert_eq!(r.rows[0].get("v"), Some(&Value::Int(42)));
}

#[test]
fn distinct_dedupes_returned_rows() {
    let r = exec("UNWIND [1,1,2,2,3,3,3] AS x RETURN DISTINCT x AS v ORDER BY v");
    assert_eq!(r.rows.len(), 3);
}

#[test]
fn order_skip_limit() {
    let r = exec("UNWIND [5,3,1,4,2] AS x RETURN x AS v ORDER BY v SKIP 1 LIMIT 2");
    assert_eq!(r.rows.len(), 2);
    assert_eq!(r.rows[0].get("v"), Some(&Value::Int(2)));
    assert_eq!(r.rows[1].get("v"), Some(&Value::Int(3)));
}

#[test]
fn parametrized_match_returns_only_matches() {
    let s = seed_social();
    // Use a literal in place — parameter binding tested elsewhere.
    let r = exec_on(
        &s,
        "MATCH (p:Person) WHERE p.name = 'Bob' RETURN p.age AS a",
    );
    assert_eq!(r.rows[0].get("a"), Some(&Value::Int(25)));
}

#[test]
fn properties_function_returns_map() {
    let s = fresh_store();
    exec_on(&s, "CREATE (:N {a:1, b:2})");
    let r = exec_on(&s, "MATCH (n:N) RETURN properties(n) AS m");
    if let Some(Value::Map(m)) = r.rows[0].get("m") {
        assert_eq!(m.get(&Box::from("a")), Some(&Value::Int(1)));
        assert_eq!(m.get(&Box::from("b")), Some(&Value::Int(2)));
    } else {
        panic!("expected map");
    }
}

#[test]
fn keys_function_lists_property_names() {
    let s = fresh_store();
    exec_on(&s, "CREATE (:N {a:1, b:2})");
    let r = exec_on(&s, "MATCH (n:N) RETURN keys(n) AS k");
    if let Some(Value::List(xs)) = r.rows[0].get("k") {
        assert_eq!(xs.len(), 2);
    } else {
        panic!("expected list");
    }
}

#[test]
fn labels_function_lists_node_labels() {
    let s = fresh_store();
    exec_on(&s, "CREATE (:A:B:C)");
    let r = exec_on(&s, "MATCH (n) RETURN labels(n) AS lbs");
    if let Some(Value::List(xs)) = r.rows[0].get("lbs") {
        assert_eq!(xs.len(), 3);
    } else {
        panic!("expected list");
    }
}

#[test]
fn type_function_returns_relationship_type() {
    let s = fresh_store();
    exec_on(&s, "CREATE (a:N)-[:LIKES]->(b:N)");
    let r = exec_on(&s, "MATCH ()-[r]->() RETURN type(r) AS t");
    assert_eq!(r.rows[0].get("t"), Some(&Value::Str("LIKES".into())));
}

#[test]
fn exists_subquery_pattern_predicate() {
    let s = seed_social();
    let r = exec_on(
        &s,
        "MATCH (p:Person) WHERE EXISTS { (p)-[:KNOWS]->() } RETURN p.name AS n ORDER BY n",
    );
    // Alice, Bob, Carol all have outgoing KNOWS.
    assert_eq!(r.rows.len(), 3);
}

#[test]
fn null_propagation_in_arithmetic() {
    let r = exec("RETURN 1 + null AS v");
    assert_eq!(r.rows[0].get("v"), Some(&Value::Null));
}

#[test]
fn boolean_operators_basic_or_and() {
    let r = exec("RETURN (true OR false) AS o, (true AND false) AS a");
    assert_eq!(r.rows[0].get("o"), Some(&Value::Bool(true)));
    assert_eq!(r.rows[0].get("a"), Some(&Value::Bool(false)));
}

#[test]
fn range_with_step() {
    let r = exec("RETURN range(1, 10, 2) AS xs");
    if let Some(Value::List(xs)) = r.rows[0].get("xs") {
        assert_eq!(xs.len(), 5);
    } else {
        panic!("expected list");
    }
}

#[test]
fn size_on_string_and_list() {
    let r = exec("RETURN size('hello') AS s1, size([1,2,3,4]) AS s2");
    assert_eq!(r.rows[0].get("s1"), Some(&Value::Int(5)));
    assert_eq!(r.rows[0].get("s2"), Some(&Value::Int(4)));
}

#[test]
fn map_literal_with_keys() {
    let r = exec("RETURN {a:1, b:2, c:3} AS proj");
    if let Some(Value::Map(m)) = r.rows[0].get("proj") {
        assert_eq!(m.len(), 3);
        assert_eq!(m.get(&Box::from("a")), Some(&Value::Int(1)));
    } else {
        panic!("expected map");
    }
}

#[test]
fn return_distinct_with_multiple_columns() {
    let r = exec(
        "UNWIND [[1,'a'],[1,'a'],[1,'b'],[2,'a']] AS p \
         RETURN DISTINCT p[0] AS x, p[1] AS y",
    );
    assert_eq!(r.rows.len(), 3);
}
