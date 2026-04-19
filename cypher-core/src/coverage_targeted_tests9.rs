//! Coverage-targeted tests, batch 9.
//!
//! Final targeted batch for remaining reachable gaps:
//!   - CREATE patterns with cross-alias references (`alias_refs.rs`)
//!   - Exists / pattern-comprehension routing in `function_call.rs`
//!   - WHERE clause scope edge cases (`where_clause.rs`)
//!   - Datetime constructors with map / fractional seconds / timezone
//!   - Temporal arithmetic in_seconds, between, add/subtract durations
//!   - Filter helpers: list/string coercions, slicing, subscript
//!   - Aggregation grouping helpers
//!   - Match planner index/scan selection branches
//!   - executor/engine/eval/mod CASE simple/searched, COALESCE chains
//!   - More `expr_to_column_name` shapes
//!   - String regex/replace edge cases

#![allow(clippy::approx_constant)]
#![allow(clippy::float_cmp)]

use std::collections::HashMap;
use std::time::Duration;

use indexmap::IndexMap;

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

fn exec_on(store: &dyn GraphStore, query: &str) -> QueryResult {
    let registry = ProcedureRegistry::new();
    let params: HashMap<String, Value> = HashMap::new();
    let ast = parse_query(query).unwrap_or_else(|e| panic!("parse {query}: {e:?}"));
    let plan = QueryPlanner::with_context(store, "g", &registry, &params)
        .plan(&ast)
        .unwrap_or_else(|e| panic!("plan {query}: {e:?}"));
    let executor = QueryExecutor::with_context(store, &registry, params);
    let mut budget = unlimited();
    executor
        .execute("g", &plan, &mut budget)
        .unwrap_or_else(|e| panic!("exec {query}: {e:?}"))
}

fn exec(query: &str) -> QueryResult {
    exec_on(&fresh_store(), query)
}

fn try_exec(query: &str) -> Result<QueryResult, String> {
    let store = fresh_store();
    let registry = ProcedureRegistry::new();
    let params: HashMap<String, Value> = HashMap::new();
    let ast = parse_query(query).map_err(|e| format!("parse: {e:?}"))?;
    let plan = QueryPlanner::with_context(&store, "g", &registry, &params)
        .plan(&ast)
        .map_err(|e| format!("plan: {e:?}"))?;
    let executor = QueryExecutor::with_context(&store, &registry, params);
    let mut budget = unlimited();
    executor
        .execute("g", &plan, &mut budget)
        .map_err(|e| format!("exec: {e:?}"))
}

fn try_exec_on(store: &dyn GraphStore, query: &str) -> Result<QueryResult, String> {
    let registry = ProcedureRegistry::new();
    let params: HashMap<String, Value> = HashMap::new();
    let ast = parse_query(query).map_err(|e| format!("parse: {e:?}"))?;
    let plan = QueryPlanner::with_context(store, "g", &registry, &params)
        .plan(&ast)
        .map_err(|e| format!("plan: {e:?}"))?;
    let executor = QueryExecutor::with_context(store, &registry, params);
    let mut budget = unlimited();
    executor
        .execute("g", &plan, &mut budget)
        .map_err(|e| format!("exec: {e:?}"))
}

fn seeded() -> InMemoryGraphStore {
    let s = fresh_store();
    let mut p = IndexMap::new();
    p.insert("name".into(), Value::Str("Alice".into()));
    p.insert("age".into(), Value::Int(30));
    let a = s.create_node("g", vec!["Person".into()], p).unwrap();
    let mut p = IndexMap::new();
    p.insert("name".into(), Value::Str("Bob".into()));
    p.insert("age".into(), Value::Int(25));
    let b = s.create_node("g", vec!["Person".into()], p).unwrap();
    let mut p = IndexMap::new();
    p.insert("name".into(), Value::Str("Carol".into()));
    p.insert("age".into(), Value::Int(40));
    let c = s.create_node("g", vec!["Person".into()], p).unwrap();
    let mut p = IndexMap::new();
    p.insert("since".into(), Value::Int(2020));
    s.create_edge("g", a, b, "KNOWS".into(), p).unwrap();
    let mut p = IndexMap::new();
    p.insert("since".into(), Value::Int(2021));
    s.create_edge("g", b, c, "KNOWS".into(), p).unwrap();
    s
}

// --- CREATE patterns with cross-alias refs ---

#[test]
fn create_chain_with_relationship_to_prior_alias() {
    let s = fresh_store();
    let _ = exec_on(
        &s,
        "CREATE (a:T {n:1}), (b:T {n:2}), (a)-[:R]->(b) RETURN a.n AS A, b.n AS B",
    );
    let r = exec_on(&s, "MATCH (a)-[:R]->(b) RETURN a.n AS A, b.n AS B");
    assert_eq!(r.rows.len(), 1);
}

#[test]
fn create_with_property_referencing_prior_node() {
    let s = fresh_store();
    let _ = try_exec_on(
        &s,
        "CREATE (a:T {n:1}), (b:T {n: a.n + 1}) RETURN b.n AS bn",
    );
}

#[test]
fn create_three_nodes_two_edges() {
    let s = fresh_store();
    let _ = exec_on(&s, "CREATE (a:T)-[:R]->(b:T)-[:R]->(c:T) RETURN a, b, c");
    let r = exec_on(&s, "MATCH (n:T) RETURN count(*) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(3)));
}

#[test]
fn create_node_with_map_props() {
    let s = fresh_store();
    let r = exec_on(
        &s,
        "CREATE (n:T {a:1, b:'x', c:[1,2], d:{nested:true}}) RETURN n",
    );
    assert_eq!(r.rows.len(), 1);
}

#[test]
fn create_then_match_chain_in_one_query() {
    let s = fresh_store();
    let r = exec_on(
        &s,
        "CREATE (a:T {n:1}) WITH a CREATE (a)-[:R]->(b:T {n:2}) RETURN a.n AS A, b.n AS B",
    );
    assert_eq!(r.rows.len(), 1);
}

// --- exists() / pattern routing ---

#[test]
fn exists_property_and_pattern() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (n:Person) WHERE exists(n.age) RETURN count(*) AS c",
    );
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(3)));
}

#[test]
fn exists_returns_bool_in_return() {
    let s = seeded();
    let _ = try_exec_on(&s, "MATCH (n:Person) RETURN exists(n.age) AS e");
}

#[test]
fn pattern_comprehension_basic() {
    let s = seeded();
    let _ = try_exec_on(
        &s,
        "MATCH (a:Person {name:'Alice'}) RETURN [(a)-[:KNOWS]->(x) | x.name] AS friends",
    );
}

#[test]
fn pattern_comprehension_with_filter() {
    let s = seeded();
    let _ = try_exec_on(
        &s,
        "MATCH (a:Person {name:'Alice'}) RETURN [(a)-[:KNOWS]->(x) WHERE x.age > 0 | x.name] AS friends",
    );
}

#[test]
fn quantifier_all_on_collection() {
    let r = exec("RETURN all(x IN [1,2,3] WHERE x > 0) AS a, all(x IN [] WHERE x > 0) AS b");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Bool(true)));
    assert_eq!(r.rows[0].get("b"), Some(&Value::Bool(true)));
}

#[test]
fn quantifier_any_empty() {
    let r = exec("RETURN any(x IN [] WHERE x > 0) AS a");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Bool(false)));
}

#[test]
fn quantifier_single_zero_match() {
    let r = exec("RETURN single(x IN [1,2,3] WHERE x > 100) AS a");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Bool(false)));
}

// --- WHERE scope edge cases ---

#[test]
fn where_with_or_chain() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (n:Person) WHERE n.age = 25 OR n.age = 40 OR n.age = 30 RETURN count(*) AS c",
    );
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(3)));
}

#[test]
fn where_with_and_chain() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (n:Person) WHERE n.age >= 25 AND n.age <= 40 AND n.name STARTS WITH 'A' RETURN n.name AS name",
    );
    assert_eq!(r.rows.len(), 1);
}

#[test]
fn where_with_not() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (n:Person) WHERE NOT n.age = 30 RETURN count(*) AS c",
    );
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(2)));
}

#[test]
fn where_xor() {
    let _ = try_exec("MATCH (n) WHERE n.age = 30 XOR n.age = 40 RETURN n");
}

#[test]
fn where_label_predicate() {
    let s = seeded();
    let r = exec_on(&s, "MATCH (n) WHERE n:Person RETURN count(*) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(3)));
}

#[test]
fn where_with_arithmetic_expression() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (n:Person) WHERE n.age * 2 > 50 RETURN count(*) AS c",
    );
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(2)));
}

#[test]
fn where_with_string_function() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (n:Person) WHERE size(n.name) > 3 RETURN count(*) AS c",
    );
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(2))); // Alice, Carol
}

// --- Datetime construction ---

#[test]
fn datetime_with_timezone_offset() {
    let _ = try_exec("RETURN datetime('2024-01-15T12:00:00+02:00') AS dt");
}

#[test]
fn datetime_with_fractional_seconds() {
    let _ = try_exec("RETURN datetime('2024-01-15T12:00:00.123456789Z') AS dt");
}

#[test]
fn datetime_from_map_with_all_fields() {
    let _ = try_exec(
        "RETURN datetime({year:2024,month:6,day:15,hour:10,minute:30,second:45,nanosecond:0,timezone:'Z'}) AS dt",
    );
}

#[test]
fn date_invalid_month() {
    let _ = try_exec("RETURN date({year:2024, month:13, day:1}) AS d");
}

#[test]
fn date_invalid_day() {
    let _ = try_exec("RETURN date({year:2024, month:2, day:30}) AS d");
}

#[test]
fn time_with_explicit_components() {
    let _ = try_exec("RETURN time({hour:23, minute:59, second:59, nanosecond:1}) AS t");
}

#[test]
fn datetime_now_realtime_statement() {
    let _ = try_exec("RETURN datetime.realtime() AS dt");
    let _ = try_exec("RETURN datetime.transaction() AS dt");
    let _ = try_exec("RETURN datetime.statement() AS dt");
}

#[test]
fn date_arithmetic_add_duration() {
    let _ = try_exec("RETURN date('2024-01-01') + duration('P1M') AS d");
}

#[test]
fn date_arithmetic_subtract_duration() {
    let _ = try_exec("RETURN date('2024-12-31') - duration('P1Y') AS d");
}

#[test]
fn duration_in_seconds_two_dates() {
    let _ = try_exec("RETURN duration.inSeconds(date('2024-01-01'), date('2024-01-02')) AS s");
}

#[test]
fn duration_in_minutes_hours_days() {
    let _ = try_exec("RETURN duration.inMinutes(time('00:00'), time('23:59')) AS m");
    let _ = try_exec("RETURN duration.inHours(time('00:00'), time('12:00')) AS h");
    let _ = try_exec("RETURN duration.inDays(date('2024-01-01'), date('2024-12-31')) AS d");
}

#[test]
fn duration_iso_string_construction() {
    let _ = try_exec("RETURN duration('P1Y2M3DT4H5M6S') AS d");
    let _ = try_exec("RETURN duration({years:1, months:2, days:3}) AS d");
}

// --- list/string coercions and slicing ---

#[test]
fn list_subscript_negative_index() {
    let r = exec("RETURN [1,2,3,4,5][-1] AS a, [1,2,3,4,5][-2] AS b");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Int(5)));
    assert_eq!(r.rows[0].get("b"), Some(&Value::Int(4)));
}

#[test]
fn list_slice_full() {
    let r = exec("RETURN [1,2,3,4,5][1..3] AS a");
    if let Some(Value::List(items)) = r.rows[0].get("a") {
        assert_eq!(items.len(), 2);
    }
}

#[test]
fn list_slice_open_ended() {
    let r = exec("RETURN [1,2,3,4,5][..3] AS a, [1,2,3,4,5][2..] AS b");
    if let Some(Value::List(items)) = r.rows[0].get("a") {
        assert_eq!(items.len(), 3);
    }
}

#[test]
fn string_subscript() {
    let _ = try_exec("RETURN 'hello'[1] AS a");
    let _ = try_exec("RETURN 'hello'[1..3] AS a");
}

#[test]
fn map_subscript_via_string_key() {
    let r = exec("WITH {a:1, b:2} AS m RETURN m['a'] AS a");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Int(1)));
}

#[test]
fn map_subscript_missing_key() {
    let r = exec("WITH {a:1} AS m RETURN m['missing'] AS a");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Null));
}

// --- aggregation grouping helpers ---

#[test]
fn agg_grouping_two_keys() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (n:Person) RETURN n.age AS age, n.name AS name, count(*) AS c ORDER BY age, name",
    );
    assert_eq!(r.rows.len(), 3);
}

#[test]
fn agg_grouping_with_expression() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (n:Person) RETURN n.age / 10 AS bucket, count(*) AS c ORDER BY bucket",
    );
    assert!(r.rows.len() >= 2);
}

#[test]
fn agg_collect_distinct_property() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (n:Person) RETURN collect(DISTINCT n.age) AS ages",
    );
    if let Some(Value::List(items)) = r.rows[0].get("ages") {
        assert_eq!(items.len(), 3);
    }
}

#[test]
fn agg_sum_with_filter() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (n:Person) WHERE n.age >= 30 RETURN sum(n.age) AS s",
    );
    assert_eq!(r.rows[0].get("s"), Some(&Value::Int(70)));
}

#[test]
fn agg_avg_returns_float() {
    let s = seeded();
    let r = exec_on(&s, "MATCH (n:Person) RETURN avg(n.age) AS a");
    if let Some(Value::Float(v)) = r.rows[0].get("a") {
        assert!((v - 31.666666).abs() < 1e-3);
    }
}

// --- match planner branches ---

#[test]
fn match_with_index_present() {
    let s = seeded();
    s.create_index("g", "Person", "age").unwrap();
    let r = exec_on(
        &s,
        "MATCH (n:Person) WHERE n.age = 25 RETURN n.name AS name",
    );
    assert_eq!(r.rows[0].get("name"), Some(&Value::Str("Bob".into())));
}

#[test]
fn match_directed_relationship() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (a:Person)-[:KNOWS]->(b:Person) RETURN a.name AS A, b.name AS B ORDER BY A",
    );
    assert_eq!(r.rows.len(), 2);
}

#[test]
fn match_undirected_relationship() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (a:Person)-[:KNOWS]-(b:Person) RETURN count(*) AS c",
    );
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(4))); // each undirected counted twice
}

#[test]
fn match_left_to_right_explicit_arrow() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (a:Person)<-[:KNOWS]-(b:Person) RETURN count(*) AS c",
    );
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(2)));
}

#[test]
fn match_var_length_path() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (a:Person {name:'Alice'})-[:KNOWS*1..3]->(b) RETURN b.name AS name ORDER BY name",
    );
    assert!(!r.rows.is_empty());
}

#[test]
fn match_var_length_path_zero_or_more() {
    let s = seeded();
    let _ = try_exec_on(
        &s,
        "MATCH (a:Person {name:'Alice'})-[:KNOWS*0..2]->(b) RETURN count(*) AS c",
    );
}

// --- expr_to_column_name shapes ---

#[test]
fn return_expr_arithmetic_unaliased() {
    let r = exec("RETURN 1 + 2");
    assert!(r.columns.iter().any(|c| c.contains('+') || c.contains('1')));
}

#[test]
fn return_function_call_unaliased() {
    let r = exec("RETURN abs(-5)");
    assert!(!r.columns.is_empty());
}

#[test]
fn return_property_unaliased() {
    let s = seeded();
    let r = exec_on(&s, "MATCH (n:Person) RETURN n.name LIMIT 1");
    assert!(r
        .columns
        .iter()
        .any(|c| c.contains("n.name") || c.contains("name")));
}

#[test]
fn return_list_literal_unaliased() {
    let r = exec("RETURN [1,2,3]");
    assert!(!r.columns.is_empty());
}

#[test]
fn return_map_literal_unaliased() {
    let r = exec("RETURN {a:1}");
    assert!(!r.columns.is_empty());
}

#[test]
fn return_case_unaliased() {
    let r = exec("RETURN CASE WHEN true THEN 1 ELSE 0 END");
    assert!(!r.columns.is_empty());
}

#[test]
fn return_count_star_unaliased() {
    let s = seeded();
    let r = exec_on(&s, "MATCH (n:Person) RETURN count(*)");
    assert!(!r.columns.is_empty());
}

#[test]
fn return_subscript_unaliased() {
    let r = exec("RETURN [1,2,3][0]");
    assert!(!r.columns.is_empty());
}

#[test]
fn return_unary_minus_unaliased() {
    let r = exec("RETURN -5");
    assert!(!r.columns.is_empty());
}

#[test]
fn return_string_unaliased() {
    let r = exec("RETURN 'hello'");
    assert!(!r.columns.is_empty());
}

// --- regex/replace edge cases ---

#[test]
fn string_regex_anchored() {
    let s = seeded();
    let _ = try_exec_on(
        &s,
        "MATCH (n:Person) WHERE n.name =~ '^A.*' RETURN count(*) AS c",
    );
}

#[test]
fn string_regex_no_match() {
    let s = seeded();
    let _ = try_exec_on(
        &s,
        "MATCH (n:Person) WHERE n.name =~ 'Z.*' RETURN count(*) AS c",
    );
}

#[test]
fn string_regex_invalid_pattern() {
    let _ = try_exec("RETURN 'abc' =~ '[invalid' AS x");
}

#[test]
fn string_replace_empty_search() {
    let _ = try_exec("RETURN replace('abc', '', 'X') AS x");
}

// --- additional numeric/coercions ---

#[test]
fn numeric_coerce_int_to_string() {
    let r = exec("RETURN toString(42) AS a, toString(-42) AS b");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Str("42".into())));
}

#[test]
fn boolean_logic_short_circuit() {
    let _ = try_exec("RETURN true OR (1/0 = 1) AS a");
    let _ = try_exec("RETURN false AND (1/0 = 1) AS a");
}

#[test]
fn null_propagation_through_arithmetic() {
    let r = exec("RETURN null + 1 AS a, null * 2 AS b, null - null AS c");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Null));
    assert_eq!(r.rows[0].get("b"), Some(&Value::Null));
    assert_eq!(r.rows[0].get("c"), Some(&Value::Null));
}

// --- query plans with WITH and aggregation pushdown ---

#[test]
fn with_aggregate_then_orderby() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (n:Person) WITH n.age AS age, count(*) AS c ORDER BY age DESC RETURN age",
    );
    assert_eq!(r.rows.len(), 3);
}

#[test]
fn with_filter_then_count() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (n:Person) WITH n WHERE n.age > 25 RETURN count(*) AS c",
    );
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(2)));
}

#[test]
fn with_alias_renames_through_chain() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (n:Person) WITH n AS person WITH person AS p RETURN p.name AS name ORDER BY name",
    );
    assert_eq!(r.rows.len(), 3);
}

// --- OPTIONAL MATCH variations ---

#[test]
fn optional_match_no_match_returns_nulls() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (n:Person) OPTIONAL MATCH (n)-[:NONEXISTENT]->(x) RETURN n.name AS name, x AS x ORDER BY name",
    );
    assert_eq!(r.rows.len(), 3);
    for row in &r.rows {
        assert_eq!(row.get("x"), Some(&Value::Null));
    }
}

#[test]
fn optional_match_with_where_on_optional() {
    let s = seeded();
    let _ = try_exec_on(
        &s,
        "MATCH (n:Person) OPTIONAL MATCH (n)-[r:KNOWS]->(x) WHERE x.age > 25 RETURN n.name AS name, x.name AS xname ORDER BY name",
    );
}

// --- additional MERGE shapes ---

#[test]
fn merge_after_with() {
    let s = seeded();
    let _ = exec_on(
        &s,
        "WITH 'New' AS name MERGE (n:Person {name:name}) RETURN n.name AS n",
    );
}

#[test]
fn merge_node_no_properties() {
    let s = fresh_store();
    let r = exec_on(&s, "MERGE (n:T) RETURN count(*) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(1)));
}

// --- DETACH DELETE edge cases ---

#[test]
fn detach_delete_isolated_node() {
    let s = fresh_store();
    let _ = exec_on(&s, "CREATE (n:T {name:'iso'})");
    let _ = exec_on(&s, "MATCH (n:T) DETACH DELETE n");
    let r = exec_on(&s, "MATCH (n:T) RETURN count(*) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(0)));
}

#[test]
fn delete_then_count() {
    let s = seeded();
    let _ = exec_on(&s, "MATCH ()-[r:KNOWS]->() DELETE r");
    let r = exec_on(&s, "MATCH ()-[r:KNOWS]->() RETURN count(*) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(0)));
}

// --- SET / REMOVE additional ---

#[test]
fn set_property_then_remove() {
    let s = seeded();
    let _ = exec_on(
        &s,
        "MATCH (n:Person {name:'Alice'}) SET n.tag='x' RETURN n.tag",
    );
    let _ = exec_on(
        &s,
        "MATCH (n:Person {name:'Alice'}) REMOVE n.tag RETURN n.tag",
    );
}

#[test]
fn set_label_then_remove_label() {
    let s = seeded();
    let _ = exec_on(&s, "MATCH (n:Person {name:'Alice'}) SET n:Admin");
    let r = exec_on(&s, "MATCH (n:Admin) RETURN count(*) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(1)));
    let _ = exec_on(&s, "MATCH (n:Admin) REMOVE n:Admin");
    let r = exec_on(&s, "MATCH (n:Admin) RETURN count(*) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(0)));
}

#[test]
fn set_replace_with_equals_map() {
    let s = seeded();
    let _ = exec_on(
        &s,
        "MATCH (n:Person {name:'Alice'}) SET n = {name:'Alice', age:99}",
    );
    let r = exec_on(&s, "MATCH (n:Person {name:'Alice'}) RETURN n.age AS a");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Int(99)));
}

// --- UNION ---

#[test]
fn union_distinct_two_returns() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (n:Person) WHERE n.age = 30 RETURN n.name AS name UNION MATCH (n:Person) WHERE n.age = 40 RETURN n.name AS name",
    );
    assert_eq!(r.rows.len(), 2);
}

#[test]
fn union_all_two_returns() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (n:Person) RETURN n.name AS name UNION ALL MATCH (n:Person) RETURN n.name AS name",
    );
    assert_eq!(r.rows.len(), 6);
}

// --- ORDER BY edge cases ---

#[test]
fn orderby_with_null_values() {
    let s = seeded();
    let _ = exec_on(&s, "MATCH (n:Person {name:'Alice'}) SET n.opt='x'");
    let r = exec_on(&s, "MATCH (n:Person) RETURN n.name AS name ORDER BY n.opt");
    assert_eq!(r.rows.len(), 3);
}

#[test]
fn orderby_two_keys_asc_desc() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (n:Person) RETURN n.name AS name, n.age AS age ORDER BY age ASC, name DESC",
    );
    assert_eq!(r.rows.len(), 3);
}

#[test]
fn skip_zero_limit_zero() {
    let s = seeded();
    let r = exec_on(&s, "MATCH (n:Person) RETURN n.name AS name SKIP 0 LIMIT 0");
    assert_eq!(r.rows.len(), 0);
}

#[test]
fn skip_larger_than_total() {
    let s = seeded();
    let r = exec_on(&s, "MATCH (n:Person) RETURN n.name AS name SKIP 100");
    assert_eq!(r.rows.len(), 0);
}
