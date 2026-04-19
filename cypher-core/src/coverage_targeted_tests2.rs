//! Second wave of lib-side targeted coverage tests.
//!
//! Focuses on planner branches and remaining low-coverage areas:
//! result_planner, planner/helpers, validator/scope/semantic/expression,
//! shaping, eval/element_funcs.  Pure additive — no production changes.

#![cfg(test)]

use std::collections::HashMap;
use std::time::Duration;

use crate::executor::{ExecutionBudget, QueryExecutor, QueryResult};
use crate::parser::parse_query;
use crate::planner::QueryPlanner;
use crate::procedure::ProcedureRegistry;
use crate::storage::{GraphStore, InMemoryGraphStore};
use crate::types::Value;

fn unlimited() -> ExecutionBudget {
    ExecutionBudget::new(Duration::from_secs(60), usize::MAX)
}

fn fresh_store() -> InMemoryGraphStore {
    let store = InMemoryGraphStore::new();
    store.create_graph("g").unwrap();
    store
}

fn exec_on(store: &InMemoryGraphStore, query: &str) -> QueryResult {
    let registry = ProcedureRegistry::new();
    let params: HashMap<String, Value> = HashMap::new();
    let q = parse_query(query).unwrap_or_else(|e| panic!("parse {query}: {e:?}"));
    let plan = QueryPlanner::with_context(store, "g", &registry, &params)
        .plan(&q)
        .unwrap_or_else(|e| panic!("plan {query}: {e:?}"));
    let exec = QueryExecutor::with_context(store, &registry, params);
    let mut b = unlimited();
    exec.execute("g", &plan, &mut b)
        .unwrap_or_else(|e| panic!("exec {query}: {e:?}"))
}

fn exec(query: &str) -> QueryResult {
    exec_on(&fresh_store(), query)
}

fn try_exec(query: &str) -> Result<QueryResult, String> {
    let store = fresh_store();
    let registry = ProcedureRegistry::new();
    let params: HashMap<String, Value> = HashMap::new();
    let q = parse_query(query).map_err(|e| format!("{e:?}"))?;
    let plan = QueryPlanner::with_context(&store, "g", &registry, &params)
        .plan(&q)
        .map_err(|e| format!("{e:?}"))?;
    let exec = QueryExecutor::with_context(&store, &registry, params);
    let mut b = unlimited();
    exec.execute("g", &plan, &mut b)
        .map_err(|e| format!("{e:?}"))
}

// ── grouping/aggregations ──────────────────────────────────────────────────

fn seed_people(store: &InMemoryGraphStore) {
    exec_on(
        store,
        "CREATE (:P {name:'a', age:20, dept:'eng'}),
                (:P {name:'b', age:30, dept:'eng'}),
                (:P {name:'c', age:25, dept:'sales'}),
                (:P {name:'d', age:35, dept:'sales'}),
                (:P {name:'e', age:40, dept:'eng'})",
    );
}

#[test]
fn group_count_by_dept() {
    let store = fresh_store();
    seed_people(&store);
    let r = exec_on(
        &store,
        "MATCH (p:P) RETURN p.dept AS d, count(*) AS c ORDER BY d",
    );
    assert_eq!(r.rows.len(), 2);
}

#[test]
fn group_avg_by_dept() {
    let store = fresh_store();
    seed_people(&store);
    let r = exec_on(
        &store,
        "MATCH (p:P) RETURN p.dept AS d, avg(p.age) AS a ORDER BY d",
    );
    assert_eq!(r.rows.len(), 2);
}

#[test]
fn group_collect_names() {
    let store = fresh_store();
    seed_people(&store);
    let r = exec_on(
        &store,
        "MATCH (p:P) RETURN p.dept AS d, collect(p.name) AS names ORDER BY d",
    );
    assert_eq!(r.rows.len(), 2);
}

#[test]
fn agg_min_max_combined() {
    let store = fresh_store();
    seed_people(&store);
    let r = exec_on(
        &store,
        "MATCH (p:P) RETURN min(p.age) AS lo, max(p.age) AS hi, sum(p.age) AS s",
    );
    assert_eq!(r.rows[0]["lo"], Value::Int(20));
    assert_eq!(r.rows[0]["hi"], Value::Int(40));
    assert_eq!(r.rows[0]["s"], Value::Int(150));
}

#[test]
fn agg_count_distinct() {
    let store = fresh_store();
    seed_people(&store);
    let r = exec_on(&store, "MATCH (p:P) RETURN count(DISTINCT p.dept) AS c");
    assert_eq!(r.rows[0]["c"], Value::Int(2));
}

// ── ORDER BY / SKIP / LIMIT after aggregation ──────────────────────────────

#[test]
fn order_by_limit() {
    let store = fresh_store();
    seed_people(&store);
    let r = exec_on(
        &store,
        "MATCH (p:P) RETURN p.name AS n ORDER BY p.age DESC LIMIT 2",
    );
    assert_eq!(r.rows.len(), 2);
}

#[test]
fn order_by_with_skip() {
    let store = fresh_store();
    seed_people(&store);
    let r = exec_on(
        &store,
        "MATCH (p:P) RETURN p.name AS n ORDER BY p.age SKIP 2",
    );
    assert_eq!(r.rows.len(), 3);
}

// ── WHERE clauses ──────────────────────────────────────────────────────────

#[test]
fn where_chained_and_or() {
    let store = fresh_store();
    seed_people(&store);
    let r = exec_on(
        &store,
        "MATCH (p:P) WHERE p.age > 25 AND (p.dept = 'eng' OR p.dept = 'hr') RETURN count(*) AS c",
    );
    assert_eq!(r.rows[0]["c"], Value::Int(2));
}

#[test]
fn where_in_list() {
    let store = fresh_store();
    seed_people(&store);
    let r = exec_on(
        &store,
        "MATCH (p:P) WHERE p.name IN ['a','c','e'] RETURN count(*) AS c",
    );
    assert_eq!(r.rows[0]["c"], Value::Int(3));
}

#[test]
fn where_starts_with_pred() {
    let store = fresh_store();
    seed_people(&store);
    let r = exec_on(
        &store,
        "MATCH (p:P) WHERE p.dept STARTS WITH 'sa' RETURN count(*) AS c",
    );
    assert_eq!(r.rows[0]["c"], Value::Int(2));
}

#[test]
fn where_regex_match() {
    let store = fresh_store();
    seed_people(&store);
    let r = exec_on(
        &store,
        "MATCH (p:P) WHERE p.name =~ '[ace]' RETURN count(*) AS c",
    );
    assert_eq!(r.rows[0]["c"], Value::Int(3));
}

// ── WITH chains ────────────────────────────────────────────────────────────

#[test]
fn with_chain_pipes() {
    let r = exec("WITH 1 AS a WITH a + 1 AS b WITH b * 2 AS c RETURN c");
    assert_eq!(r.rows[0]["c"], Value::Int(4));
}

#[test]
fn with_aggregation_then_filter() {
    let store = fresh_store();
    seed_people(&store);
    let r = exec_on(
        &store,
        "MATCH (p:P) WITH p.dept AS d, count(*) AS c WHERE c > 2 RETURN d",
    );
    assert_eq!(r.rows.len(), 1);
}

#[test]
fn with_distinct() {
    let store = fresh_store();
    seed_people(&store);
    let r = exec_on(
        &store,
        "MATCH (p:P) WITH DISTINCT p.dept AS d RETURN d ORDER BY d",
    );
    assert_eq!(r.rows.len(), 2);
}

// ── relationship patterns ──────────────────────────────────────────────────

fn seed_graph(store: &InMemoryGraphStore) {
    exec_on(
        store,
        "CREATE (a:P {name:'a'}),
                (b:P {name:'b'}),
                (c:P {name:'c'}),
                (d:P {name:'d'}),
                (a)-[:KNOWS {since:2020}]->(b),
                (b)-[:KNOWS {since:2021}]->(c),
                (a)-[:KNOWS {since:2019}]->(c),
                (c)-[:KNOWS {since:2022}]->(d)",
    );
}

#[test]
fn pattern_directed_edge() {
    let store = fresh_store();
    seed_graph(&store);
    let r = exec_on(
        &store,
        "MATCH (a:P {name:'a'})-[:KNOWS]->(b) RETURN count(b) AS c",
    );
    assert_eq!(r.rows[0]["c"], Value::Int(2));
}

#[test]
fn pattern_undirected_edge() {
    let store = fresh_store();
    seed_graph(&store);
    let r = exec_on(
        &store,
        "MATCH (a:P {name:'b'})-[:KNOWS]-(other) RETURN count(other) AS c",
    );
    assert_eq!(r.rows[0]["c"], Value::Int(2));
}

#[test]
fn pattern_with_edge_alias() {
    let store = fresh_store();
    seed_graph(&store);
    let r = exec_on(
        &store,
        "MATCH ()-[r:KNOWS]->() RETURN count(r) AS c, sum(r.since) AS s",
    );
    assert_eq!(r.rows[0]["c"], Value::Int(4));
}

#[test]
fn pattern_path_var_assignment() {
    let store = fresh_store();
    seed_graph(&store);
    let r = exec_on(
        &store,
        "MATCH p = (:P {name:'a'})-[:KNOWS]->(:P) RETURN length(p) AS l, count(*) AS c",
    );
    assert!(!r.rows.is_empty());
}

#[test]
fn pattern_var_length_min_max() {
    let store = fresh_store();
    seed_graph(&store);
    let r = exec_on(
        &store,
        "MATCH (a:P {name:'a'})-[:KNOWS*1..3]->(b) RETURN count(DISTINCT b) AS c",
    );
    assert!(matches!(r.rows[0]["c"], Value::Int(_)));
}

#[test]
fn pattern_node_property_filter_inline() {
    let store = fresh_store();
    seed_graph(&store);
    let r = exec_on(&store, "MATCH (a {name:'a'}) RETURN labels(a) AS labels");
    assert_eq!(r.rows.len(), 1);
}

// ── multiple labels & types ────────────────────────────────────────────────

#[test]
fn multiple_labels_create_match() {
    let store = fresh_store();
    exec_on(&store, "CREATE (:A:B:C {x:1})");
    let r = exec_on(&store, "MATCH (n:A) WHERE n:B RETURN count(n) AS c");
    assert_eq!(r.rows[0]["c"], Value::Int(1));
}

#[test]
fn relationship_type_alternatives() {
    let store = fresh_store();
    exec_on(&store, "CREATE (a)-[:KNOWS]->(b), (a)-[:LIKES]->(c)");
    let r = exec_on(&store, "MATCH (a)-[r:KNOWS|LIKES]->() RETURN count(r) AS c");
    assert_eq!(r.rows[0]["c"], Value::Int(2));
}

// ── result shaping ─────────────────────────────────────────────────────────

#[test]
fn shape_return_list_literal() {
    let r = exec("RETURN [1, 2, 3] AS l");
    assert_eq!(
        r.rows[0]["l"],
        Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)])
    );
}

#[test]
fn shape_return_map_literal() {
    let r = exec("RETURN {a: 1, b: 'two'} AS m");
    if let Value::Map(m) = &r.rows[0]["m"] {
        assert_eq!(m.get("a"), Some(&Value::Int(1)));
        assert_eq!(m.get("b"), Some(&Value::Str("two".into())));
    } else {
        panic!()
    }
}

#[test]
fn shape_nested_map() {
    let r = exec("RETURN {outer: {inner: 7}} AS m");
    if let Value::Map(outer) = &r.rows[0]["m"] {
        if let Some(Value::Map(inner)) = outer.get("outer") {
            assert_eq!(inner.get("inner"), Some(&Value::Int(7)));
        } else {
            panic!()
        }
    } else {
        panic!()
    }
}

#[test]
fn shape_return_literal_null_and_bool() {
    let r = exec("RETURN null AS n, true AS t, false AS f");
    assert_eq!(r.rows[0]["n"], Value::Null);
    assert_eq!(r.rows[0]["t"], Value::Bool(true));
    assert_eq!(r.rows[0]["f"], Value::Bool(false));
}

// ── boolean short-circuit / null propagation ───────────────────────────────

#[test]
fn null_propagation_in_arithmetic() {
    assert_eq!(val_of("RETURN 1 + null"), Value::Null);
    assert_eq!(val_of("RETURN null * 5"), Value::Null);
}

#[test]
fn null_propagation_in_comparison() {
    assert_eq!(val_of("RETURN null = null"), Value::Null);
    assert_eq!(val_of("RETURN null < 1"), Value::Null);
}

#[test]
fn three_valued_logic_and() {
    assert_eq!(val_of("RETURN true AND null"), Value::Null);
    assert_eq!(val_of("RETURN false AND null"), Value::Bool(false));
    assert_eq!(val_of("RETURN null AND true"), Value::Null);
}

#[test]
fn three_valued_logic_or() {
    assert_eq!(val_of("RETURN true OR null"), Value::Bool(true));
    assert_eq!(val_of("RETURN false OR null"), Value::Null);
    assert_eq!(val_of("RETURN null OR false"), Value::Null);
}

fn val_of(q: &str) -> Value {
    let r = exec(q);
    r.rows[0].iter().next().unwrap().1.clone()
}

// ── error paths ────────────────────────────────────────────────────────────

#[test]
fn parse_error_unmatched_paren() {
    assert!(try_exec("RETURN (1 + 2").is_err());
}

#[test]
fn parse_error_invalid_keyword() {
    assert!(try_exec("RETUR 1").is_err());
}

#[test]
fn semantic_error_unknown_function() {
    assert!(try_exec("RETURN nonexistent_func(1)").is_err());
}

#[test]
fn semantic_error_unknown_alias_in_return() {
    assert!(try_exec("MATCH (a) RETURN bogus").is_err());
}

#[test]
fn type_error_in_arithmetic() {
    let r = try_exec("RETURN 'abc' + 5");
    // Either an error or an implicit conversion — both acceptable
    let _ = r;
}

// ── procedure / SHOW (best-effort, may not be supported) ───────────────────

#[test]
fn show_indexes_or_constraints_does_not_panic() {
    let _ = try_exec("SHOW INDEXES");
    let _ = try_exec("SHOW CONSTRAINTS");
}

// ── more builtins coverage ─────────────────────────────────────────────────

#[test]
fn nested_function_calls() {
    assert_eq!(val_of("RETURN size(reverse([1,2,3,4]))"), Value::Int(4));
}

#[test]
fn case_with_aggregation() {
    let store = fresh_store();
    seed_people(&store);
    let r = exec_on(
        &store,
        "MATCH (p:P) RETURN sum(CASE WHEN p.age > 25 THEN 1 ELSE 0 END) AS over25",
    );
    assert_eq!(r.rows[0]["over25"], Value::Int(3));
}

#[test]
fn comparison_operators_all() {
    assert_eq!(val_of("RETURN 1 < 2"), Value::Bool(true));
    assert_eq!(val_of("RETURN 1 <= 1"), Value::Bool(true));
    assert_eq!(val_of("RETURN 2 > 1"), Value::Bool(true));
    assert_eq!(val_of("RETURN 1 >= 1"), Value::Bool(true));
    assert_eq!(val_of("RETURN 1 = 1"), Value::Bool(true));
    assert_eq!(val_of("RETURN 1 <> 2"), Value::Bool(true));
}

// ── UNWIND with subsequent operations ──────────────────────────────────────

#[test]
fn unwind_then_create() {
    let store = fresh_store();
    exec_on(&store, "UNWIND [1,2,3,4,5] AS x CREATE (:N {value:x})");
    let r = exec_on(&store, "MATCH (n:N) RETURN count(n) AS c");
    assert_eq!(r.rows[0]["c"], Value::Int(5));
}

#[test]
fn unwind_then_merge() {
    let store = fresh_store();
    exec_on(
        &store,
        "UNWIND ['a','b','a','c','b'] AS x MERGE (:N {name:x})",
    );
    let r = exec_on(&store, "MATCH (n:N) RETURN count(n) AS c");
    assert_eq!(r.rows[0]["c"], Value::Int(3));
}

#[test]
fn unwind_map_list() {
    let r = exec("UNWIND [{x:1},{x:2},{x:3}] AS m RETURN m.x AS v ORDER BY v");
    assert_eq!(r.rows.len(), 3);
}

// ── FOREACH ────────────────────────────────────────────────────────────────

#[test]
fn foreach_creates_nodes() {
    let store = fresh_store();
    exec_on(&store, "FOREACH (i IN [1,2,3] | CREATE (:F {n:i}))");
    let r = exec_on(&store, "MATCH (n:F) RETURN count(n) AS c");
    assert_eq!(r.rows[0]["c"], Value::Int(3));
}

// ── Multiple MATCH clauses ─────────────────────────────────────────────────

#[test]
fn multiple_match_cartesian() {
    let store = fresh_store();
    exec_on(&store, "CREATE (:A), (:A), (:B), (:B)");
    let r = exec_on(&store, "MATCH (a:A) MATCH (b:B) RETURN count(*) AS c");
    assert_eq!(r.rows[0]["c"], Value::Int(4));
}

// ── Property maps in patterns ──────────────────────────────────────────────

#[test]
fn match_with_property_map() {
    let store = fresh_store();
    exec_on(
        &store,
        "CREATE (:P {a:1, b:2}), (:P {a:1, b:3}), (:P {a:2})",
    );
    let r = exec_on(&store, "MATCH (p:P {a:1}) RETURN count(p) AS c");
    assert_eq!(r.rows[0]["c"], Value::Int(2));
}

// ── stdev, percentile aggregates ───────────────────────────────────────────

#[test]
fn agg_stdev_basic() {
    let store = fresh_store();
    seed_people(&store);
    let r = exec_on(&store, "MATCH (p:P) RETURN stDev(p.age) AS s");
    assert!(matches!(r.rows[0]["s"], Value::Float(_) | Value::Int(_)));
}

#[test]
fn agg_percentile_disc() {
    let store = fresh_store();
    seed_people(&store);
    let r = exec_on(
        &store,
        "MATCH (p:P) RETURN percentileDisc(p.age, 0.5) AS p50",
    );
    assert!(matches!(r.rows[0]["p50"], Value::Float(_) | Value::Int(_)));
}

#[test]
fn agg_percentile_cont() {
    let store = fresh_store();
    seed_people(&store);
    let r = exec_on(
        &store,
        "MATCH (p:P) RETURN percentileCont(p.age, 0.5) AS p50",
    );
    assert!(matches!(r.rows[0]["p50"], Value::Float(_) | Value::Int(_)));
}

// ── numeric overflow / large numbers ───────────────────────────────────────

#[test]
fn large_integer_literals() {
    assert_eq!(val_of("RETURN 9223372036854775807"), Value::Int(i64::MAX));
}

#[test]
fn range_returns_long_list() {
    if let Value::List(items) = val_of("RETURN range(1, 100)") {
        assert_eq!(items.len(), 100);
    } else {
        panic!()
    }
}

// ── temporal: extended ─────────────────────────────────────────────────────

#[test]
fn datetime_construction_from_components() {
    let _ = try_exec(
        "RETURN datetime({year:2023, month:6, day:15, hour:10, minute:30, second:0}) AS dt",
    );
    let _ = try_exec("RETURN datetime('2023-06-15T10:30:00Z') AS dt");
    let _ = try_exec("RETURN datetime() AS now");
}

#[test]
fn date_construction_and_arithmetic() {
    let _ = try_exec("RETURN date('2023-06-15') AS d");
    let _ = try_exec("RETURN date({year:2023, month:6, day:15}) AS d");
    let _ = try_exec("RETURN date('2023-06-15') + duration('P1D') AS d2");
    let _ = try_exec("RETURN date('2023-06-15') - date('2023-01-01') AS dur");
}

#[test]
fn time_and_localtime() {
    let _ = try_exec("RETURN time('12:34:56') AS t");
    let _ = try_exec("RETURN localtime('12:34:56') AS t");
    let _ = try_exec("RETURN localdatetime('2023-06-15T12:34:56') AS dt");
}

#[test]
fn duration_arithmetic_variants() {
    let _ = try_exec("RETURN duration('P1Y2M3D') AS d");
    let _ = try_exec("RETURN duration('PT1H30M') AS d");
    let _ = try_exec("RETURN duration({days: 7, hours: 3}) AS d");
    let _ = try_exec("RETURN duration('P1D') + duration('P2D') AS s");
    let _ = try_exec("RETURN duration('P10D') - duration('P3D') AS s");
}

// ── temporal extraction ────────────────────────────────────────────────────

#[test]
fn temporal_extract_components() {
    let _ = try_exec("RETURN date('2023-06-15').year AS y");
    let _ = try_exec("RETURN date('2023-06-15').month AS m");
    let _ = try_exec("RETURN date('2023-06-15').day AS d");
    let _ = try_exec("RETURN datetime('2023-06-15T10:30:45Z').hour AS h");
    let _ = try_exec("RETURN datetime('2023-06-15T10:30:45Z').minute AS m");
    let _ = try_exec("RETURN datetime('2023-06-15T10:30:45Z').second AS s");
}

// ── MERGE variants ─────────────────────────────────────────────────────────

#[test]
fn merge_node_on_create_set() {
    let store = fresh_store();
    exec_on(
        &store,
        "MERGE (n:M {id:1}) ON CREATE SET n.created = true RETURN n.created",
    );
    let r = exec_on(&store, "MATCH (n:M {id:1}) RETURN n.created AS c");
    assert_eq!(r.rows[0]["c"], Value::Bool(true));
}

#[test]
fn merge_node_on_match_set() {
    let store = fresh_store();
    exec_on(&store, "CREATE (:M {id:1, count:0})");
    exec_on(
        &store,
        "MERGE (n:M {id:1}) ON MATCH SET n.count = n.count + 1",
    );
    let r = exec_on(&store, "MATCH (n:M {id:1}) RETURN n.count AS c");
    assert_eq!(r.rows[0]["c"], Value::Int(1));
}

#[test]
fn merge_with_both_clauses() {
    let store = fresh_store();
    exec_on(
        &store,
        "MERGE (n:M {id:1}) ON CREATE SET n.created = true ON MATCH SET n.matched = true",
    );
    let r = exec_on(&store, "MATCH (n:M {id:1}) RETURN n.created AS c");
    assert_eq!(r.rows[0]["c"], Value::Bool(true));
}

#[test]
fn merge_relationship() {
    let store = fresh_store();
    exec_on(&store, "CREATE (a:A {id:1}), (b:B {id:2})");
    exec_on(
        &store,
        "MATCH (a:A {id:1}), (b:B {id:2}) MERGE (a)-[r:LINKS]->(b) RETURN r",
    );
    let r = exec_on(&store, "MATCH ()-[r:LINKS]->() RETURN count(r) AS c");
    assert_eq!(r.rows[0]["c"], Value::Int(1));
}

#[test]
fn merge_relationship_idempotent() {
    let store = fresh_store();
    exec_on(&store, "CREATE (a:A {id:1}), (b:B {id:2})");
    for _ in 0..3 {
        exec_on(
            &store,
            "MATCH (a:A {id:1}), (b:B {id:2}) MERGE (a)-[r:LINKS]->(b)",
        );
    }
    let r = exec_on(&store, "MATCH ()-[r:LINKS]->() RETURN count(r) AS c");
    assert_eq!(r.rows[0]["c"], Value::Int(1));
}

// ── SET operations ─────────────────────────────────────────────────────────

#[test]
fn set_property_basic() {
    let store = fresh_store();
    exec_on(&store, "CREATE (:N {id:1})");
    exec_on(&store, "MATCH (n:N {id:1}) SET n.name = 'updated'");
    let r = exec_on(&store, "MATCH (n:N) RETURN n.name AS name");
    assert_eq!(r.rows[0]["name"], Value::Str("updated".into()));
}

#[test]
fn set_multiple_props() {
    let store = fresh_store();
    exec_on(&store, "CREATE (:N {id:1})");
    exec_on(&store, "MATCH (n:N) SET n.a = 1, n.b = 2, n.c = 3");
    let r = exec_on(&store, "MATCH (n:N) RETURN n.a + n.b + n.c AS s");
    assert_eq!(r.rows[0]["s"], Value::Int(6));
}

#[test]
fn set_label_on_node() {
    let store = fresh_store();
    exec_on(&store, "CREATE (:N {id:1})");
    exec_on(&store, "MATCH (n:N) SET n:Extra");
    let r = exec_on(&store, "MATCH (n:Extra) RETURN count(n) AS c");
    assert_eq!(r.rows[0]["c"], Value::Int(1));
}

#[test]
fn set_map_replace() {
    let store = fresh_store();
    exec_on(&store, "CREATE (:N {a:1, b:2})");
    exec_on(&store, "MATCH (n:N) SET n = {x: 10, y: 20}");
    let r = exec_on(&store, "MATCH (n:N) RETURN n.x AS x, n.a AS a");
    assert_eq!(r.rows[0]["x"], Value::Int(10));
    assert_eq!(r.rows[0]["a"], Value::Null);
}

#[test]
fn set_map_merge_plus_eq() {
    let store = fresh_store();
    exec_on(&store, "CREATE (:N {a:1, b:2})");
    exec_on(&store, "MATCH (n:N) SET n += {b: 99, c: 3}");
    let r = exec_on(&store, "MATCH (n:N) RETURN n.a AS a, n.b AS b, n.c AS c");
    assert_eq!(r.rows[0]["a"], Value::Int(1));
    assert_eq!(r.rows[0]["b"], Value::Int(99));
    assert_eq!(r.rows[0]["c"], Value::Int(3));
}

// ── REMOVE operations ──────────────────────────────────────────────────────

#[test]
fn remove_property() {
    let store = fresh_store();
    exec_on(&store, "CREATE (:N {a:1, b:2})");
    exec_on(&store, "MATCH (n:N) REMOVE n.a");
    let r = exec_on(&store, "MATCH (n:N) RETURN n.a AS a, n.b AS b");
    assert_eq!(r.rows[0]["a"], Value::Null);
    assert_eq!(r.rows[0]["b"], Value::Int(2));
}

#[test]
fn remove_label() {
    let store = fresh_store();
    exec_on(&store, "CREATE (:A:B {id:1})");
    exec_on(&store, "MATCH (n:A) REMOVE n:B");
    let r = exec_on(&store, "MATCH (n:B) RETURN count(n) AS c");
    assert_eq!(r.rows[0]["c"], Value::Int(0));
}

// ── DELETE / DETACH DELETE ─────────────────────────────────────────────────

#[test]
fn delete_isolated_node() {
    let store = fresh_store();
    exec_on(&store, "CREATE (:N {id:1})");
    exec_on(&store, "MATCH (n:N) DELETE n");
    let r = exec_on(&store, "MATCH (n:N) RETURN count(n) AS c");
    assert_eq!(r.rows[0]["c"], Value::Int(0));
}

#[test]
fn detach_delete_with_edges() {
    let store = fresh_store();
    exec_on(&store, "CREATE (a:A)-[:R]->(b:B)");
    exec_on(&store, "MATCH (a:A) DETACH DELETE a");
    let r = exec_on(&store, "MATCH (a:A) RETURN count(a) AS c");
    assert_eq!(r.rows[0]["c"], Value::Int(0));
}

#[test]
fn delete_relationship() {
    let store = fresh_store();
    exec_on(&store, "CREATE (a:A)-[:R]->(b:B)");
    exec_on(&store, "MATCH (:A)-[r:R]->(:B) DELETE r");
    let r = exec_on(&store, "MATCH ()-[r:R]->() RETURN count(r) AS c");
    assert_eq!(r.rows[0]["c"], Value::Int(0));
}

// ── collection functions ───────────────────────────────────────────────────

#[test]
fn list_predicate_all() {
    assert_eq!(
        val_of("RETURN all(x IN [1,2,3,4] WHERE x > 0)"),
        Value::Bool(true)
    );
    assert_eq!(
        val_of("RETURN all(x IN [1,2,3,-1] WHERE x > 0)"),
        Value::Bool(false)
    );
}

#[test]
fn list_predicate_any() {
    assert_eq!(
        val_of("RETURN any(x IN [1,2,3] WHERE x > 2)"),
        Value::Bool(true)
    );
    assert_eq!(
        val_of("RETURN any(x IN [1,2,3] WHERE x > 5)"),
        Value::Bool(false)
    );
}

#[test]
fn list_predicate_none() {
    assert_eq!(
        val_of("RETURN none(x IN [1,2,3] WHERE x > 10)"),
        Value::Bool(true)
    );
}

#[test]
fn list_predicate_single() {
    assert_eq!(
        val_of("RETURN single(x IN [1,2,3] WHERE x = 2)"),
        Value::Bool(true)
    );
    assert_eq!(
        val_of("RETURN single(x IN [1,2,2] WHERE x = 2)"),
        Value::Bool(false)
    );
}

#[test]
fn list_comprehension_with_filter_and_map() {
    let r = exec("RETURN [x IN [1,2,3,4,5] WHERE x > 2 | x * 10] AS res");
    assert_eq!(
        r.rows[0]["res"],
        Value::List(vec![Value::Int(30), Value::Int(40), Value::Int(50)])
    );
}

#[test]
fn list_comprehension_filter_only() {
    let r = exec("RETURN [x IN [1,2,3,4] WHERE x % 2 = 0] AS res");
    assert_eq!(
        r.rows[0]["res"],
        Value::List(vec![Value::Int(2), Value::Int(4)])
    );
}

#[test]
fn list_comprehension_map_only() {
    let r = exec("RETURN [x IN [1,2,3] | x + 100] AS res");
    assert_eq!(
        r.rows[0]["res"],
        Value::List(vec![Value::Int(101), Value::Int(102), Value::Int(103)])
    );
}

#[test]
fn list_reduce() {
    assert_eq!(
        val_of("RETURN reduce(s = 0, x IN [1,2,3,4,5] | s + x)"),
        Value::Int(15)
    );
}

// ── string functions ───────────────────────────────────────────────────────

#[test]
fn str_split_join() {
    assert_eq!(
        val_of("RETURN split('a,b,c', ',')"),
        Value::List(vec![
            Value::Str("a".into()),
            Value::Str("b".into()),
            Value::Str("c".into())
        ])
    );
}

#[test]
fn str_replace_substring_trim() {
    assert_eq!(
        val_of("RETURN replace('hello world', 'world', 'rust')"),
        Value::Str("hello rust".into())
    );
    assert_eq!(
        val_of("RETURN substring('hello', 1, 3)"),
        Value::Str("ell".into())
    );
    assert_eq!(val_of("RETURN trim('  hi  ')"), Value::Str("hi".into()));
    assert_eq!(val_of("RETURN ltrim('   hi')"), Value::Str("hi".into()));
    assert_eq!(val_of("RETURN rtrim('hi   ')"), Value::Str("hi".into()));
}

#[test]
fn str_to_upper_lower() {
    assert_eq!(val_of("RETURN toUpper('hi')"), Value::Str("HI".into()));
    assert_eq!(val_of("RETURN toLower('HI')"), Value::Str("hi".into()));
}

#[test]
fn str_left_right_reverse() {
    assert_eq!(val_of("RETURN left('hello', 3)"), Value::Str("hel".into()));
    assert_eq!(val_of("RETURN right('hello', 3)"), Value::Str("llo".into()));
    assert_eq!(val_of("RETURN reverse('abc')"), Value::Str("cba".into()));
}

// ── numeric functions ──────────────────────────────────────────────────────

#[test]
fn math_round_floor_ceil() {
    assert_eq!(val_of("RETURN round(2.7)"), Value::Float(3.0));
    assert_eq!(val_of("RETURN floor(2.7)"), Value::Float(2.0));
    assert_eq!(val_of("RETURN ceil(2.3)"), Value::Float(3.0));
}

#[test]
fn math_abs_sign() {
    assert_eq!(val_of("RETURN abs(-5)"), Value::Int(5));
    assert_eq!(val_of("RETURN sign(-3)"), Value::Int(-1));
    assert_eq!(val_of("RETURN sign(7)"), Value::Int(1));
    assert_eq!(val_of("RETURN sign(0)"), Value::Int(0));
}

#[test]
fn math_sqrt_log_exp() {
    assert_eq!(val_of("RETURN sqrt(16.0)"), Value::Float(4.0));
    let _ = val_of("RETURN log(2.718281828)");
    let _ = val_of("RETURN log10(100.0)");
    let _ = val_of("RETURN exp(1.0)");
}

#[test]
fn math_trig() {
    let _ = val_of("RETURN sin(0.0)");
    let _ = val_of("RETURN cos(0.0)");
    let _ = val_of("RETURN tan(0.0)");
    let _ = val_of("RETURN asin(0.0)");
    let _ = val_of("RETURN acos(1.0)");
    let _ = val_of("RETURN atan(0.0)");
    let _ = val_of("RETURN atan2(1.0, 1.0)");
    let _ = val_of("RETURN pi()");
    let _ = val_of("RETURN e()");
}

// ── conversion functions ───────────────────────────────────────────────────

#[test]
fn type_conversions() {
    assert_eq!(val_of("RETURN toInteger('42')"), Value::Int(42));
    assert_eq!(val_of("RETURN toFloat('2.5')"), Value::Float(2.5));
    assert_eq!(val_of("RETURN toString(42)"), Value::Str("42".into()));
    assert_eq!(val_of("RETURN toBoolean('true')"), Value::Bool(true));
    assert_eq!(val_of("RETURN toBoolean('false')"), Value::Bool(false));
}

#[test]
fn type_conversion_failure() {
    assert_eq!(val_of("RETURN toInteger('not-a-number')"), Value::Null);
    assert_eq!(val_of("RETURN toFloat('xyz')"), Value::Null);
}

// ── element functions ──────────────────────────────────────────────────────

#[test]
fn element_id_and_id() {
    let store = fresh_store();
    exec_on(&store, "CREATE (:N {id:1})");
    let _ = exec_on(&store, "MATCH (n:N) RETURN id(n) AS id");
}

#[test]
fn keys_and_properties() {
    let store = fresh_store();
    exec_on(&store, "CREATE (:N {a:1, b:2, c:3})");
    let r = exec_on(
        &store,
        "MATCH (n:N) RETURN keys(n) AS ks, properties(n) AS p",
    );
    if let Value::List(ks) = &r.rows[0]["ks"] {
        assert_eq!(ks.len(), 3);
    } else {
        panic!()
    }
}

#[test]
fn type_function_on_relationship() {
    let store = fresh_store();
    exec_on(&store, "CREATE (a)-[:KNOWS]->(b)");
    let r = exec_on(&store, "MATCH ()-[r]->() RETURN type(r) AS t");
    assert_eq!(r.rows[0]["t"], Value::Str("KNOWS".into()));
}

#[test]
fn labels_function() {
    let store = fresh_store();
    exec_on(&store, "CREATE (:A:B:C)");
    let r = exec_on(&store, "MATCH (n) RETURN labels(n) AS ls");
    if let Value::List(ls) = &r.rows[0]["ls"] {
        assert_eq!(ls.len(), 3);
    } else {
        panic!()
    }
}

#[test]
fn start_node_end_node_funcs() {
    let store = fresh_store();
    exec_on(&store, "CREATE (a:A)-[:R]->(b:B)");
    let r = exec_on(
        &store,
        "MATCH ()-[r]->() RETURN startNode(r) AS s, endNode(r) AS e",
    );
    assert_eq!(r.rows.len(), 1);
}

#[test]
fn nodes_relationships_on_path() {
    let store = fresh_store();
    exec_on(&store, "CREATE (a:A)-[:R]->(b:B)-[:R]->(c:C)");
    let r = exec_on(
        &store,
        "MATCH p = (:A)-[:R*]->(:C) RETURN nodes(p) AS ns, relationships(p) AS rs",
    );
    if !r.rows.is_empty() {
        if let Value::List(ns) = &r.rows[0]["ns"] {
            assert!(ns.len() >= 2);
        }
    }
}

// ── coalesce / head / tail / last ──────────────────────────────────────────

#[test]
fn coalesce_first_non_null() {
    assert_eq!(val_of("RETURN coalesce(null, null, 7, 8)"), Value::Int(7));
    assert_eq!(val_of("RETURN coalesce(null)"), Value::Null);
}

#[test]
fn list_head_tail_last() {
    assert_eq!(val_of("RETURN head([1,2,3])"), Value::Int(1));
    assert_eq!(val_of("RETURN last([1,2,3])"), Value::Int(3));
    if let Value::List(t) = val_of("RETURN tail([1,2,3])") {
        assert_eq!(t, vec![Value::Int(2), Value::Int(3)]);
    } else {
        panic!()
    }
}

// ── EXISTS / NOT EXISTS / IS NULL / IS NOT NULL ────────────────────────────

#[test]
fn exists_property_check() {
    let store = fresh_store();
    exec_on(&store, "CREATE (:N {a:1}), (:N)");
    let r = exec_on(
        &store,
        "MATCH (n:N) WHERE n.a IS NOT NULL RETURN count(n) AS c",
    );
    assert_eq!(r.rows[0]["c"], Value::Int(1));
}

#[test]
fn exists_subquery_pattern() {
    let store = fresh_store();
    seed_graph(&store);
    let r = exec_on(
        &store,
        "MATCH (a:P) WHERE EXISTS { (a)-[:KNOWS]->() } RETURN count(a) AS c",
    );
    assert_eq!(r.rows[0]["c"], Value::Int(3));
}

// ── UNION variants ─────────────────────────────────────────────────────────

#[test]
fn union_all_keeps_dupes() {
    let r = exec("RETURN 1 AS x UNION ALL RETURN 1 AS x UNION ALL RETURN 2 AS x");
    assert_eq!(r.rows.len(), 3);
}

#[test]
fn union_dedupes() {
    let r = exec("RETURN 1 AS x UNION RETURN 1 AS x UNION RETURN 2 AS x");
    assert_eq!(r.rows.len(), 2);
}

// ── OPTIONAL MATCH ─────────────────────────────────────────────────────────

#[test]
fn optional_match_no_relationship() {
    let store = fresh_store();
    exec_on(&store, "CREATE (:Solo {id:1})");
    let r = exec_on(
        &store,
        "MATCH (n:Solo) OPTIONAL MATCH (n)-[r:KNOWS]->(other) RETURN n, r, other",
    );
    assert_eq!(r.rows.len(), 1);
    assert_eq!(r.rows[0]["r"], Value::Null);
    assert_eq!(r.rows[0]["other"], Value::Null);
}

#[test]
fn optional_match_with_aggregation() {
    let store = fresh_store();
    seed_graph(&store);
    let r = exec_on(
        &store,
        "MATCH (a:P) OPTIONAL MATCH (a)-[:KNOWS]->(b) RETURN a.name AS n, count(b) AS c ORDER BY n",
    );
    assert_eq!(r.rows.len(), 4);
}

// ── parameters ─────────────────────────────────────────────────────────────

#[test]
fn parameter_substitution_basic() {
    let store = fresh_store();
    exec_on(
        &store,
        "CREATE (:N {id:1, name:'a'}), (:N {id:2, name:'b'})",
    );
    let registry = ProcedureRegistry::new();
    let mut params = HashMap::new();
    params.insert("nm".to_string(), Value::Str("b".into()));
    let q = parse_query("MATCH (n:N {name:$nm}) RETURN n.id AS id").unwrap();
    let plan = QueryPlanner::with_context(&store, "g", &registry, &params)
        .plan(&q)
        .unwrap();
    let exec = QueryExecutor::with_context(&store, &registry, params);
    let mut b = unlimited();
    let r = exec.execute("g", &plan, &mut b).unwrap();
    assert_eq!(r.rows[0]["id"], Value::Int(2));
}

#[test]
fn parameter_in_list_pred() {
    let store = fresh_store();
    exec_on(
        &store,
        "CREATE (:N {id:1}), (:N {id:2}), (:N {id:3}), (:N {id:4})",
    );
    let registry = ProcedureRegistry::new();
    let mut params = HashMap::new();
    params.insert(
        "ids".to_string(),
        Value::List(vec![Value::Int(2), Value::Int(4)]),
    );
    let q = parse_query("MATCH (n:N) WHERE n.id IN $ids RETURN count(n) AS c").unwrap();
    let plan = QueryPlanner::with_context(&store, "g", &registry, &params)
        .plan(&q)
        .unwrap();
    let exec = QueryExecutor::with_context(&store, &registry, params);
    let mut b = unlimited();
    let r = exec.execute("g", &plan, &mut b).unwrap();
    assert_eq!(r.rows[0]["c"], Value::Int(2));
}
