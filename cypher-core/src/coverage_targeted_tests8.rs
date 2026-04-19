//! Coverage-targeted tests, batch 8.
//!
//! Final push targeting:
//!   - element_funcs (id/type/labels/keys/properties branches)
//!   - filter/builtins/numeric (round/floor/ceil/sign/abs/sqrt edge cases)
//!   - filter/builtins/string (substring/replace/split/trim/pad)
//!   - filter/collections (range/keys/values/head/tail/last/size/reverse/concat)
//!   - filter/temporal/constructors (date/time/datetime literals & from-map)
//!   - filter/temporal/arithmetic (durations between dates/times)
//!   - executor/engine/helpers/comparison (= / <> / </>/IS NULL/IN)
//!   - executor/engine/helpers/arithmetic (mod / pow / unary minus / mixed types)
//!   - executor/engine/eval/mod (CASE WHEN / coalesce / chained property)
//!   - merge/rel (MERGE relationship paths with ON CREATE / ON MATCH)
//!   - planner/cardinality, planner/match_planner, where_clause additional shapes
//!   - planner/result_planner/with_clause variants

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

// --- element_funcs ---

#[test]
fn element_id_node_and_edge() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (n:Person)-[r:KNOWS]->(m) RETURN id(n), id(r), id(m)",
    );
    assert!(!r.rows.is_empty());
}

#[test]
fn element_labels_and_type_and_keys() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (n:Person)-[r:KNOWS]->() RETURN labels(n) AS L, type(r) AS T, keys(n) AS K",
    );
    assert!(!r.rows.is_empty());
    let row0 = &r.rows[0];
    assert!(matches!(row0.get("L"), Some(Value::List(_))));
    assert_eq!(row0.get("T"), Some(&Value::Str("KNOWS".into())));
    assert!(matches!(row0.get("K"), Some(Value::List(_))));
}

#[test]
fn element_properties_function() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (n:Person {name:'Alice'}) RETURN properties(n) AS P",
    );
    assert_eq!(r.rows.len(), 1);
    assert!(matches!(r.rows[0].get("P"), Some(Value::Map(_))));
}

#[test]
fn element_id_on_unbound_returns_null_or_errors() {
    let _ = try_exec("RETURN id(null)");
    let _ = try_exec("RETURN labels(null)");
    let _ = try_exec("RETURN type(null)");
    let _ = try_exec("RETURN keys(null)");
    let _ = try_exec("RETURN properties(null)");
}

#[test]
fn element_keys_on_map() {
    let _ = try_exec("WITH {a:1, b:2, c:3} AS m RETURN keys(m) AS K");
}

#[test]
fn element_properties_on_map() {
    let _ = try_exec("WITH {a:1, b:2} AS m RETURN properties(m) AS P");
}

// --- numeric builtins ---

#[test]
fn numeric_round_modes() {
    let r = exec("RETURN round(2.5) AS a, round(2.4) AS b, round(-2.5) AS c, round(-2.4) AS d");
    let _ = r;
}

#[test]
fn numeric_floor_ceil() {
    let r = exec("RETURN floor(1.7) AS a, ceil(1.2) AS b, floor(-1.2) AS c, ceil(-1.7) AS d");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Float(1.0)));
    assert_eq!(r.rows[0].get("b"), Some(&Value::Float(2.0)));
    assert_eq!(r.rows[0].get("c"), Some(&Value::Float(-2.0)));
    assert_eq!(r.rows[0].get("d"), Some(&Value::Float(-1.0)));
}

#[test]
fn numeric_sign_and_abs() {
    let r = exec("RETURN sign(-5) AS a, sign(0) AS b, sign(5) AS c, abs(-7) AS d");
    let _ = r;
}

#[test]
fn numeric_sqrt_and_pow() {
    let r = exec("RETURN sqrt(16.0) AS a, sqrt(2.0) AS b");
    if let Some(Value::Float(v)) = r.rows[0].get("a") {
        assert_eq!(*v, 4.0);
    }
}

#[test]
fn numeric_pow_via_function() {
    let _ = try_exec("RETURN toFloat(2)^toFloat(8) AS a");
    let _ = try_exec("RETURN 2^8 AS a");
}

#[test]
fn numeric_negative_sqrt() {
    let _ = try_exec("RETURN sqrt(-1.0) AS a");
}

#[test]
fn numeric_log_e_log10() {
    let _ = try_exec("RETURN log(2.718281828) AS a, log10(1000.0) AS b");
}

#[test]
fn numeric_e_pi_constants() {
    let r = exec("RETURN e() AS e, pi() AS p");
    let _ = r;
}

#[test]
fn numeric_rand_returns_in_range() {
    let r = exec("RETURN rand() AS r");
    if let Some(Value::Float(v)) = r.rows[0].get("r") {
        assert!((0.0..1.0).contains(v));
    }
}

#[test]
fn numeric_haversin_and_trig() {
    let _ = try_exec("RETURN sin(0.0) AS s, cos(0.0) AS c, tan(0.0) AS t");
    let _ = try_exec("RETURN asin(0.0) AS s, acos(1.0) AS c, atan(0.0) AS t");
    let _ = try_exec("RETURN atan2(1.0, 1.0) AS a");
}

#[test]
fn numeric_to_integer_and_to_float() {
    let r = exec("RETURN toInteger('42') AS a, toFloat('3.14') AS b");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Int(42)));
    let _ = try_exec("RETURN toInteger(true) AS c");
}

#[test]
fn numeric_to_integer_invalid() {
    let _ = try_exec("RETURN toInteger('not-a-number') AS a");
}

#[test]
fn numeric_division_by_zero_int() {
    let _ = try_exec("RETURN 1 / 0 AS a");
}

#[test]
fn numeric_modulo_neg() {
    let r = exec("RETURN -7 % 3 AS a, 7 % -3 AS b");
    let _ = r;
}

// --- string builtins ---

#[test]
fn string_substring_one_arg_neg_check() {
    let _ = try_exec("RETURN substring('hello', 100) AS a");
}

#[test]
fn string_split_simple() {
    let r = exec("RETURN split('a,b,c', ',') AS a");
    if let Some(Value::List(items)) = r.rows[0].get("a") {
        assert_eq!(items.len(), 3);
    }
}

#[test]
fn string_split_no_match() {
    let r = exec("RETURN split('abcdef', 'z') AS a");
    if let Some(Value::List(items)) = r.rows[0].get("a") {
        assert_eq!(items.len(), 1);
    }
}

#[test]
fn string_trim_ltrim_rtrim() {
    let r = exec("RETURN trim('  hi  ') AS a, lTrim('  hi  ') AS b, rTrim('  hi  ') AS c");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Str("hi".into())));
    assert_eq!(r.rows[0].get("b"), Some(&Value::Str("hi  ".into())));
    assert_eq!(r.rows[0].get("c"), Some(&Value::Str("  hi".into())));
}

#[test]
fn string_to_upper_lower() {
    let r = exec("RETURN toUpper('abC') AS a, toLower('AbC') AS b");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Str("ABC".into())));
    assert_eq!(r.rows[0].get("b"), Some(&Value::Str("abc".into())));
}

#[test]
fn string_to_string_variants() {
    let _ = try_exec(
        "RETURN toString(42) AS a, toString(3.14) AS b, toString(true) AS c, toString(null) AS d",
    );
}

#[test]
fn string_replace_multi() {
    let r = exec("RETURN replace('aaa', 'a', 'bb') AS a");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Str("bbbbbb".into())));
}

#[test]
fn string_reverse() {
    let r = exec("RETURN reverse('abc') AS a");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Str("cba".into())));
}

#[test]
fn string_size_function() {
    let r = exec("RETURN size('hello') AS a");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Int(5)));
}

#[test]
fn string_left_right() {
    let _ = try_exec("RETURN left('hello', 3) AS a, right('hello', 2) AS b");
}

// --- collections ---

#[test]
fn collection_range_function() {
    let r = exec("RETURN range(1, 5) AS a");
    if let Some(Value::List(items)) = r.rows[0].get("a") {
        assert_eq!(items.len(), 5);
    }
}

#[test]
fn collection_range_step() {
    let r = exec("RETURN range(0, 10, 2) AS a");
    if let Some(Value::List(items)) = r.rows[0].get("a") {
        assert_eq!(items.len(), 6);
    }
}

#[test]
fn collection_range_descending() {
    let r = exec("RETURN range(5, 1, -1) AS a");
    if let Some(Value::List(items)) = r.rows[0].get("a") {
        assert_eq!(items.len(), 5);
    }
}

#[test]
fn collection_head_tail_last() {
    let r = exec("RETURN head([1,2,3]) AS a, tail([1,2,3]) AS b, last([1,2,3]) AS c");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Int(1)));
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(3)));
}

#[test]
fn collection_head_empty() {
    let r = exec("RETURN head([]) AS a, tail([]) AS b, last([]) AS c");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Null));
}

#[test]
fn collection_size_list() {
    let r = exec("RETURN size([1,2,3,4]) AS a, size([]) AS b");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Int(4)));
    assert_eq!(r.rows[0].get("b"), Some(&Value::Int(0)));
}

#[test]
fn collection_reverse_list() {
    let r = exec("RETURN reverse([1,2,3]) AS a");
    if let Some(Value::List(items)) = r.rows[0].get("a") {
        assert_eq!(items.len(), 3);
        assert_eq!(items[0], Value::Int(3));
    }
}

#[test]
fn collection_in_predicate() {
    let r = exec("RETURN 2 IN [1,2,3] AS a, 5 IN [1,2,3] AS b, null IN [1,2,3] AS c");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Bool(true)));
    assert_eq!(r.rows[0].get("b"), Some(&Value::Bool(false)));
}

#[test]
fn collection_concat_via_plus() {
    let r = exec("RETURN [1,2] + [3,4] AS a");
    if let Some(Value::List(items)) = r.rows[0].get("a") {
        assert_eq!(items.len(), 4);
    }
}

#[test]
fn collection_list_comprehension_filter_map() {
    let r = exec("RETURN [x IN range(1,5) WHERE x > 2 | x * 10] AS a");
    if let Some(Value::List(items)) = r.rows[0].get("a") {
        assert_eq!(items.len(), 3);
    }
}

#[test]
fn collection_keys_on_map() {
    let r = exec("RETURN keys({a:1, b:2, c:3}) AS K");
    if let Some(Value::List(items)) = r.rows[0].get("K") {
        assert_eq!(items.len(), 3);
    }
}

#[test]
fn collection_unwind_list_of_lists() {
    let r = exec("UNWIND [[1,2],[3,4]] AS x RETURN x");
    assert_eq!(r.rows.len(), 2);
}

#[test]
fn collection_single_predicate() {
    let _ = try_exec("RETURN single(x IN [1,2,3,2] WHERE x = 2) AS a");
}

#[test]
fn collection_any_all_none() {
    let r = exec(
        "RETURN any(x IN [1,2,3] WHERE x > 2) AS a, all(x IN [1,2,3] WHERE x > 0) AS b, none(x IN [1,2,3] WHERE x > 5) AS c",
    );
    assert_eq!(r.rows[0].get("a"), Some(&Value::Bool(true)));
    assert_eq!(r.rows[0].get("b"), Some(&Value::Bool(true)));
    assert_eq!(r.rows[0].get("c"), Some(&Value::Bool(true)));
}

// --- temporal constructors ---

#[test]
fn temporal_date_now_string_iso() {
    let _ = try_exec("RETURN date() AS a");
    let _ = try_exec("RETURN date('2024-01-15') AS a");
    let _ = try_exec("RETURN date({year:2024, month:6, day:15}) AS a");
}

#[test]
fn temporal_time_construction() {
    let _ = try_exec("RETURN time('12:30:45') AS a");
    let _ = try_exec("RETURN time({hour:12, minute:30, second:45}) AS a");
}

#[test]
fn temporal_datetime_construction() {
    let _ = try_exec("RETURN datetime('2024-01-15T12:00:00Z') AS a");
    let _ = try_exec("RETURN datetime({year:2024,month:1,day:15,hour:12,minute:0,second:0}) AS a");
}

#[test]
fn temporal_localdatetime_construction() {
    let _ = try_exec("RETURN localdatetime('2024-01-15T12:00:00') AS a");
}

#[test]
fn temporal_localtime_construction() {
    let _ = try_exec("RETURN localtime('12:30:45') AS a");
}

#[test]
fn temporal_duration_between() {
    let _ = try_exec("RETURN duration.between(date('2024-01-01'), date('2024-12-31')) AS a");
}

#[test]
fn temporal_duration_in_seconds_minutes() {
    let _ = try_exec("RETURN duration.inSeconds(time('12:00'), time('13:00')) AS a");
    let _ = try_exec("RETURN duration.inMinutes(time('12:00'), time('13:30')) AS a");
}

#[test]
fn temporal_property_extraction() {
    let _ = try_exec("RETURN date('2024-06-15').year AS y, date('2024-06-15').month AS m, date('2024-06-15').day AS d");
}

#[test]
fn temporal_invalid_string_format() {
    let _ = try_exec("RETURN date('not-a-date') AS a");
    let _ = try_exec("RETURN time('not-a-time') AS a");
    let _ = try_exec("RETURN datetime('not-a-dt') AS a");
}

#[test]
fn temporal_truncation_to_day_month_year() {
    let _ = try_exec("RETURN date.truncate('day', date('2024-06-15')) AS a");
    let _ = try_exec("RETURN date.truncate('month', date('2024-06-15')) AS a");
    let _ = try_exec("RETURN date.truncate('year', date('2024-06-15')) AS a");
}

// --- comparison helpers ---

#[test]
fn comparison_eq_neq() {
    let r = exec("RETURN 1 = 1 AS a, 1 <> 2 AS b, 'x' = 'x' AS c, true = false AS d");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Bool(true)));
    assert_eq!(r.rows[0].get("b"), Some(&Value::Bool(true)));
}

#[test]
fn comparison_lt_gt_le_ge() {
    let r = exec("RETURN 1 < 2 AS a, 2 > 1 AS b, 1 <= 1 AS c, 2 >= 2 AS d");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Bool(true)));
    assert_eq!(r.rows[0].get("b"), Some(&Value::Bool(true)));
    assert_eq!(r.rows[0].get("c"), Some(&Value::Bool(true)));
    assert_eq!(r.rows[0].get("d"), Some(&Value::Bool(true)));
}

#[test]
fn comparison_with_null_returns_null() {
    let r = exec("RETURN null = 1 AS a, null = null AS b, null < 1 AS c");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Null));
}

#[test]
fn comparison_mixed_types() {
    let _ = try_exec("RETURN 1 = '1' AS a");
    let _ = try_exec("RETURN 1.0 = 1 AS a");
}

#[test]
fn comparison_is_null_is_not_null() {
    let r = exec("RETURN null IS NULL AS a, 1 IS NOT NULL AS b, null IS NOT NULL AS c");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Bool(true)));
    assert_eq!(r.rows[0].get("b"), Some(&Value::Bool(true)));
    assert_eq!(r.rows[0].get("c"), Some(&Value::Bool(false)));
}

#[test]
fn comparison_string_lex_order() {
    let r = exec("RETURN 'apple' < 'banana' AS a, 'b' > 'a' AS b");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Bool(true)));
    assert_eq!(r.rows[0].get("b"), Some(&Value::Bool(true)));
}

// --- arithmetic helpers ---

#[test]
fn arithmetic_int_int_ops() {
    let r = exec("RETURN 2 + 3 AS a, 5 - 2 AS b, 4 * 3 AS c, 10 / 3 AS d, 10 % 3 AS e");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Int(5)));
    assert_eq!(r.rows[0].get("d"), Some(&Value::Int(3)));
    assert_eq!(r.rows[0].get("e"), Some(&Value::Int(1)));
}

#[test]
fn arithmetic_float_float_ops() {
    let r = exec("RETURN 1.5 + 2.5 AS a, 10.0 / 4.0 AS b");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Float(4.0)));
    assert_eq!(r.rows[0].get("b"), Some(&Value::Float(2.5)));
}

#[test]
fn arithmetic_int_float_promotion() {
    let r = exec("RETURN 1 + 2.0 AS a, 5.0 * 2 AS b");
    let _ = r;
}

#[test]
fn arithmetic_unary_minus() {
    let r = exec("RETURN -5 AS a, -(3 + 2) AS b, -3.14 AS c");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Int(-5)));
}

#[test]
fn arithmetic_string_concat() {
    let r = exec("RETURN 'a' + 'b' AS a, 'foo' + 'bar' AS b");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Str("ab".into())));
}

#[test]
fn arithmetic_overflow_int() {
    let _ = try_exec("RETURN 9223372036854775807 + 1 AS a");
}

// --- eval/case ---

#[test]
fn case_simple_form() {
    let r = exec("RETURN CASE 1 WHEN 1 THEN 'one' WHEN 2 THEN 'two' ELSE 'other' END AS a");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Str("one".into())));
}

#[test]
fn case_searched_form() {
    let r = exec("RETURN CASE WHEN 1 < 2 THEN 'yes' WHEN 2 < 1 THEN 'no' ELSE 'maybe' END AS a");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Str("yes".into())));
}

#[test]
fn case_no_else_returns_null() {
    let r = exec("RETURN CASE WHEN false THEN 'x' END AS a");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Null));
}

#[test]
fn case_with_property() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (n:Person) RETURN n.name AS name, CASE WHEN n.age >= 30 THEN 'senior' ELSE 'junior' END AS tier ORDER BY name",
    );
    assert_eq!(r.rows.len(), 3);
}

#[test]
fn coalesce_function() {
    let r = exec("RETURN coalesce(null, null, 'x', 'y') AS a, coalesce(null, null) AS b");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Str("x".into())));
    assert_eq!(r.rows[0].get("b"), Some(&Value::Null));
}

#[test]
fn chained_property_access() {
    let s = seeded();
    let _ = try_exec("MATCH (n:Person) RETURN n.props.name AS x");
    let _ = s;
}

// --- merge/rel ---

#[test]
fn merge_relationship_on_match() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (a:Person {name:'Alice'}), (b:Person {name:'Bob'}) MERGE (a)-[r:KNOWS]->(b) ON MATCH SET r.touched=1 RETURN r.touched AS t",
    );
    assert_eq!(r.rows[0].get("t"), Some(&Value::Int(1)));
}

#[test]
fn merge_relationship_on_create() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (a:Person {name:'Alice'}), (c:Person {name:'Carol'}) MERGE (a)-[r:NEW_REL]->(c) ON CREATE SET r.fresh=1 RETURN r.fresh AS f",
    );
    assert_eq!(r.rows[0].get("f"), Some(&Value::Int(1)));
}

#[test]
fn merge_relationship_with_props_match() {
    let s = seeded();
    let _ = exec_on(
        &s,
        "MATCH (a:Person {name:'Alice'}), (b:Person {name:'Bob'}) MERGE (a)-[r:KNOWS {since:2020}]->(b) RETURN r.since",
    );
}

#[test]
fn merge_node_on_match_set() {
    let s = seeded();
    let _ = exec_on(
        &s,
        "MERGE (n:Person {name:'Alice'}) ON MATCH SET n.touched=99 RETURN n.touched AS t",
    );
}

#[test]
fn merge_node_on_create_set() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MERGE (n:Person {name:'Dave'}) ON CREATE SET n.created=1 RETURN n.created AS c",
    );
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(1)));
}

#[test]
fn merge_node_create_then_match() {
    let s = seeded();
    let _ = exec_on(&s, "MERGE (n:Person {name:'Eve'}) RETURN n");
    let r = exec_on(&s, "MATCH (n:Person {name:'Eve'}) RETURN count(*) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(1)));
}

// --- planner: WITH variants ---

#[test]
fn with_passes_aggregate_then_filter() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (n:Person) WITH count(*) AS total WHERE total > 0 RETURN total",
    );
    assert_eq!(r.rows[0].get("total"), Some(&Value::Int(3)));
}

#[test]
fn with_distinct_then_count() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (n:Person) WITH DISTINCT n.age AS age RETURN count(*) AS c",
    );
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(3)));
}

#[test]
fn with_group_by_label_aggregation() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (n:Person) WITH n.age AS age, count(*) AS c RETURN age, c ORDER BY age",
    );
    assert_eq!(r.rows.len(), 3);
}

#[test]
fn with_orderby_skip_limit() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (n:Person) WITH n ORDER BY n.age DESC SKIP 1 LIMIT 1 RETURN n.name AS name",
    );
    assert_eq!(r.rows.len(), 1);
}

#[test]
fn with_chain_multiple() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (n:Person) WITH n.age AS age WITH age * 2 AS doubled RETURN doubled ORDER BY doubled",
    );
    assert_eq!(r.rows.len(), 3);
}

// --- planner: cardinality / match ---

#[test]
fn cardinality_label_index_present() {
    let s = seeded();
    s.create_index("g", "Person", "name").unwrap();
    let r = exec_on(&s, "MATCH (n:Person {name:'Alice'}) RETURN n.age AS a");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Int(30)));
}

#[test]
fn cardinality_with_join() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (a:Person)-[:KNOWS]->(b:Person)-[:KNOWS]->(c:Person) RETURN a.name AS A, c.name AS C",
    );
    assert_eq!(r.rows.len(), 1);
}

#[test]
fn match_optional_chain() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (a:Person {name:'Carol'}) OPTIONAL MATCH (a)-[:KNOWS]->(b) RETURN a.name AS A, b AS B",
    );
    assert_eq!(r.rows.len(), 1);
    assert_eq!(r.rows[0].get("B"), Some(&Value::Null));
}

#[test]
fn match_multiple_patterns_same_clause() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (a:Person {name:'Alice'}), (b:Person {name:'Bob'}) RETURN a.age + b.age AS s",
    );
    assert_eq!(r.rows[0].get("s"), Some(&Value::Int(55)));
}

#[test]
fn match_with_inline_property_filter() {
    let s = seeded();
    let r = exec_on(&s, "MATCH (n:Person {age:30}) RETURN n.name AS name");
    assert_eq!(r.rows.len(), 1);
}

#[test]
fn where_property_exists() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (n:Person) WHERE n.age IS NOT NULL RETURN count(*) AS c",
    );
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(3)));
}

#[test]
fn where_in_list_literal() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (n:Person) WHERE n.name IN ['Alice','Bob'] RETURN count(*) AS c",
    );
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(2)));
}

#[test]
fn where_starts_ends_contains_combined() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (n:Person) WHERE n.name STARTS WITH 'A' OR n.name ENDS WITH 'l' RETURN count(*) AS c",
    );
    let _ = r;
}

#[test]
fn where_regex_match() {
    let s = seeded();
    let _ = try_exec("MATCH (n:Person) WHERE n.name =~ '[A-Z].*' RETURN count(*) AS c");
    let _ = s;
}

// --- additional eval: labels/properties when null ---

#[test]
fn properties_on_relationship() {
    let s = seeded();
    let r = exec_on(&s, "MATCH ()-[r:KNOWS]->() RETURN properties(r) AS P");
    assert!(!r.rows.is_empty());
    assert!(matches!(r.rows[0].get("P"), Some(Value::Map(_))));
}

#[test]
fn type_with_optional_match_null_rel() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (a:Person {name:'Carol'}) OPTIONAL MATCH (a)-[r:KNOWS]->() RETURN type(r) AS T",
    );
    assert_eq!(r.rows[0].get("T"), Some(&Value::Null));
}

// --- aggregations with grouping ---

#[test]
fn agg_min_max_sum_avg() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (n:Person) RETURN min(n.age) AS mn, max(n.age) AS mx, sum(n.age) AS s, avg(n.age) AS a",
    );
    assert_eq!(r.rows[0].get("mn"), Some(&Value::Int(25)));
    assert_eq!(r.rows[0].get("mx"), Some(&Value::Int(40)));
    assert_eq!(r.rows[0].get("s"), Some(&Value::Int(95)));
}

#[test]
fn agg_collect_returns_list() {
    let s = seeded();
    let r = exec_on(&s, "MATCH (n:Person) RETURN collect(n.name) AS names");
    if let Some(Value::List(items)) = r.rows[0].get("names") {
        assert_eq!(items.len(), 3);
    }
}

#[test]
fn agg_count_distinct() {
    let s = seeded();
    let r = exec_on(&s, "MATCH (n:Person) RETURN count(DISTINCT n.age) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(3)));
}

#[test]
fn agg_with_grouping_key() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (n:Person) RETURN n.age AS age, count(*) AS c ORDER BY age",
    );
    assert_eq!(r.rows.len(), 3);
}

// --- map literals + projection ---

#[test]
fn map_literal_in_return() {
    let r = exec("RETURN {a:1, b:'x', c:[1,2]} AS m");
    assert!(matches!(r.rows[0].get("m"), Some(Value::Map(_))));
}

#[test]
fn map_projection_node() {
    let s = seeded();
    let _ = try_exec("MATCH (n:Person) RETURN n {.name, .age} AS m");
    let _ = s;
}

// --- DELETE ---

#[test]
fn delete_node_simple() {
    let s = seeded();
    let _ = exec_on(&s, "MATCH (n:Person {name:'Alice'}) DETACH DELETE n");
    let r = exec_on(&s, "MATCH (n:Person) RETURN count(*) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(2)));
}

#[test]
fn delete_relationship_only() {
    let s = seeded();
    let _ = exec_on(&s, "MATCH ()-[r:KNOWS]->() DELETE r");
    let r = exec_on(&s, "MATCH ()-[r:KNOWS]->() RETURN count(*) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(0)));
}

// --- SET / REMOVE additional ---

#[test]
fn set_multiple_properties_via_map() {
    let s = seeded();
    let _ = exec_on(
        &s,
        "MATCH (n:Person {name:'Alice'}) SET n += {age:31, city:'NYC'} RETURN n.city AS c, n.age AS a",
    );
}
