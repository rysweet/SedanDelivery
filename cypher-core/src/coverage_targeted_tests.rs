//! Lib-side targeted coverage tests written from scratch.
//!
//! Goal: exercise paths in low-coverage files (parser/expression.rs,
//! executor/filter/builtins.rs, executor/engine/eval/element_funcs.rs,
//! executor/engine/shaping.rs, planner/mod.rs, planner/validator/*) via
//! the public `parse → plan → execute` pipeline.
//!
//! Pure additive: no production code changes.

#![cfg(test)]

use std::collections::HashMap;
use std::time::Duration;

use crate::executor::{ExecutionBudget, QueryExecutor, QueryResult};
use crate::parser::parse_query;
use crate::planner::{collect_graph_statistics, estimate_cardinality, QueryPlanner};
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
    let q = parse_query(query).expect(query);
    let plan = QueryPlanner::with_context(store, "g", &registry, &params)
        .plan(&q)
        .expect(query);
    let exec = QueryExecutor::with_context(store, &registry, params);
    let mut b = unlimited();
    exec.execute("g", &plan, &mut b).expect(query)
}

fn exec(query: &str) -> QueryResult {
    exec_on(&fresh_store(), query)
}

fn val(query: &str) -> Value {
    let r = exec(query);
    assert_eq!(r.rows.len(), 1, "expected 1 row from {query}");
    let (_, v) = r.rows[0].iter().next().unwrap();
    v.clone()
}

// ── parser corners ─────────────────────────────────────────────────────────

#[test]
fn parse_unary_minus_chain() {
    assert_eq!(val("RETURN -(-5)"), Value::Int(5));
}

#[test]
fn parse_unary_plus() {
    assert_eq!(val("RETURN +7"), Value::Int(7));
}

#[test]
fn parse_power_basic() {
    if let Value::Float(f) = val("RETURN 2 ^ 3") {
        assert!((f - 8.0).abs() < 1e-9);
    } else if let Value::Int(i) = val("RETURN 2 ^ 3") {
        assert_eq!(i, 8);
    } else {
        panic!()
    }
}

#[test]
fn parse_xor_operator() {
    assert_eq!(val("RETURN true XOR false"), Value::Bool(true));
    assert_eq!(val("RETURN true XOR true"), Value::Bool(false));
}

#[test]
fn parse_not_operator() {
    assert_eq!(val("RETURN NOT false"), Value::Bool(true));
    assert_eq!(val("RETURN NOT NOT true"), Value::Bool(true));
}

#[test]
fn parse_string_predicates() {
    assert_eq!(val("RETURN 'hello' STARTS WITH 'he'"), Value::Bool(true));
    assert_eq!(val("RETURN 'hello' ENDS WITH 'lo'"), Value::Bool(true));
    assert_eq!(val("RETURN 'hello' CONTAINS 'ell'"), Value::Bool(true));
    assert_eq!(val("RETURN 'hello' STARTS WITH 'xx'"), Value::Bool(false));
}

#[test]
fn parse_in_predicate() {
    assert_eq!(val("RETURN 2 IN [1,2,3]"), Value::Bool(true));
    assert_eq!(val("RETURN 9 IN [1,2,3]"), Value::Bool(false));
    assert_eq!(val("RETURN null IN [1,2,3]"), Value::Null);
}

#[test]
fn parse_is_null_and_not_null() {
    assert_eq!(val("RETURN null IS NULL"), Value::Bool(true));
    assert_eq!(val("RETURN 1 IS NULL"), Value::Bool(false));
    assert_eq!(val("RETURN null IS NOT NULL"), Value::Bool(false));
    assert_eq!(val("RETURN 1 IS NOT NULL"), Value::Bool(true));
}

#[test]
fn parse_case_simple() {
    assert_eq!(
        val("RETURN CASE 1 WHEN 1 THEN 'one' WHEN 2 THEN 'two' END"),
        Value::Str("one".into())
    );
}

#[test]
fn parse_case_searched() {
    assert_eq!(
        val("RETURN CASE WHEN 1 < 2 THEN 'yes' ELSE 'no' END"),
        Value::Str("yes".into())
    );
}

#[test]
fn parse_case_else_default() {
    assert_eq!(
        val("RETURN CASE 99 WHEN 1 THEN 'a' ELSE 'fallback' END"),
        Value::Str("fallback".into())
    );
}

#[test]
fn parse_case_no_match_no_else() {
    assert_eq!(val("RETURN CASE 99 WHEN 1 THEN 'a' END"), Value::Null);
}

#[test]
fn parse_list_literal_nested() {
    assert_eq!(
        val("RETURN [[1,2],[3,4]]"),
        Value::List(vec![
            Value::List(vec![Value::Int(1), Value::Int(2)]),
            Value::List(vec![Value::Int(3), Value::Int(4)]),
        ])
    );
}

#[test]
fn parse_list_comp_with_filter_and_map() {
    let v = val("RETURN [x IN [1,2,3,4,5] WHERE x > 2 | x * 10]");
    assert_eq!(
        v,
        Value::List(vec![Value::Int(30), Value::Int(40), Value::Int(50)])
    );
}

#[test]
fn parse_list_comp_filter_only() {
    let v = val("RETURN [x IN [1,2,3,4] WHERE x % 2 = 0]");
    assert_eq!(v, Value::List(vec![Value::Int(2), Value::Int(4)]));
}

#[test]
fn parse_extract_desugar() {
    let v = val("RETURN [x IN [1,2,3] | x + 100]");
    assert_eq!(
        v,
        Value::List(vec![Value::Int(101), Value::Int(102), Value::Int(103)])
    );
}

#[test]
fn parse_reduce() {
    let v = val("RETURN reduce(s = 0, x IN [1,2,3,4] | s + x)");
    assert_eq!(v, Value::Int(10));
}

#[test]
fn parse_all_quantifier() {
    assert_eq!(
        val("RETURN all(x IN [1,2,3] WHERE x > 0)"),
        Value::Bool(true)
    );
    assert_eq!(
        val("RETURN all(x IN [1,2,3] WHERE x > 2)"),
        Value::Bool(false)
    );
}

#[test]
fn parse_any_quantifier() {
    assert_eq!(
        val("RETURN any(x IN [1,2,3] WHERE x > 2)"),
        Value::Bool(true)
    );
    assert_eq!(
        val("RETURN any(x IN [1,2,3] WHERE x > 9)"),
        Value::Bool(false)
    );
}

#[test]
fn parse_none_quantifier() {
    assert_eq!(
        val("RETURN none(x IN [1,2,3] WHERE x > 9)"),
        Value::Bool(true)
    );
    assert_eq!(
        val("RETURN none(x IN [1,2,3] WHERE x > 2)"),
        Value::Bool(false)
    );
}

#[test]
fn parse_single_quantifier() {
    assert_eq!(
        val("RETURN single(x IN [1,2,3] WHERE x = 2)"),
        Value::Bool(true)
    );
    assert_eq!(
        val("RETURN single(x IN [1,2,3] WHERE x > 0)"),
        Value::Bool(false)
    );
}

#[test]
fn parse_subscript_negative_index() {
    assert_eq!(val("RETURN [10,20,30][-1]"), Value::Int(30));
    assert_eq!(val("RETURN [10,20,30][-3]"), Value::Int(10));
}

#[test]
fn parse_subscript_oob_returns_null() {
    assert_eq!(val("RETURN [10,20,30][9]"), Value::Null);
    assert_eq!(val("RETURN [10,20,30][-9]"), Value::Null);
}

#[test]
fn parse_slice_full() {
    let v = val("RETURN [1,2,3,4,5][1..3]");
    assert_eq!(v, Value::List(vec![Value::Int(2), Value::Int(3)]));
}

#[test]
fn parse_slice_open_start() {
    let v = val("RETURN [1,2,3][..2]");
    assert_eq!(v, Value::List(vec![Value::Int(1), Value::Int(2)]));
}

#[test]
fn parse_slice_open_end() {
    let v = val("RETURN [1,2,3][1..]");
    assert_eq!(v, Value::List(vec![Value::Int(2), Value::Int(3)]));
}

#[test]
fn parse_slice_negative_bounds() {
    let v = val("RETURN [1,2,3,4,5][-3..-1]");
    assert_eq!(v, Value::List(vec![Value::Int(3), Value::Int(4)]));
}

#[test]
fn parse_map_literal_subscript() {
    assert_eq!(val("RETURN {a: 1, b: 2}['b']"), Value::Int(2));
}

#[test]
fn parse_map_literal_dot_access() {
    assert_eq!(val("RETURN {a: 1, b: 2}.a"), Value::Int(1));
}

#[test]
fn parse_arithmetic_modulo() {
    assert_eq!(val("RETURN 10 % 3"), Value::Int(1));
}

#[test]
fn parse_arithmetic_integer_div() {
    assert_eq!(val("RETURN 10 / 3"), Value::Int(3));
}

#[test]
fn parse_float_division() {
    let v = val("RETURN 10.0 / 4.0");
    if let Value::Float(f) = v {
        assert!((f - 2.5).abs() < 1e-9);
    } else {
        panic!()
    }
}

#[test]
fn parse_concat_strings() {
    assert_eq!(val("RETURN 'a' + 'b'"), Value::Str("ab".into()));
}

#[test]
fn parse_concat_lists() {
    assert_eq!(
        val("RETURN [1,2] + [3,4]"),
        Value::List(vec![
            Value::Int(1),
            Value::Int(2),
            Value::Int(3),
            Value::Int(4),
        ])
    );
}

#[test]
fn parse_string_literal_escapes() {
    assert_eq!(
        val(r"RETURN 'line\nbreak'"),
        Value::Str("line\nbreak".into())
    );
    assert_eq!(val(r"RETURN 'tab\there'"), Value::Str("tab\there".into()));
}

#[test]
fn parse_double_quoted_string() {
    assert_eq!(val("RETURN \"hello\""), Value::Str("hello".into()));
}

#[test]
fn parse_negative_float_literal() {
    if let Value::Float(f) = val("RETURN -2.5") {
        assert!((f + 2.5).abs() < 1e-9);
    } else {
        panic!()
    }
}

#[test]
fn parse_scientific_notation() {
    if let Value::Float(f) = val("RETURN 1.5e2") {
        assert!((f - 150.0).abs() < 1e-9);
    } else {
        panic!()
    }
}

#[test]
fn parse_hex_literal() {
    assert_eq!(val("RETURN 0xFF"), Value::Int(255));
}

#[test]
fn parse_octal_literal() {
    assert_eq!(val("RETURN 0o17"), Value::Int(15));
}

#[test]
fn parse_with_clause() {
    let r = exec("WITH 1 AS x, 2 AS y RETURN x + y AS s");
    assert_eq!(r.rows.len(), 1);
    assert_eq!(r.rows[0]["s"], Value::Int(3));
}

#[test]
fn parse_with_where() {
    let r = exec("UNWIND [1,2,3,4] AS x WITH x WHERE x > 2 RETURN x");
    let xs: Vec<i64> = r
        .rows
        .iter()
        .map(|row| match &row["x"] {
            Value::Int(i) => *i,
            _ => -1,
        })
        .collect();
    assert_eq!(xs, vec![3, 4]);
}

#[test]
fn parse_unwind_basic() {
    let r = exec("UNWIND [1,2,3] AS x RETURN x");
    assert_eq!(r.rows.len(), 3);
}

#[test]
fn parse_unwind_empty() {
    let r = exec("UNWIND [] AS x RETURN x");
    assert_eq!(r.rows.len(), 0);
}

#[test]
fn parse_unwind_null() {
    let r = exec("UNWIND null AS x RETURN x");
    // Unwinding null produces a single row with null binding
    assert!(r.rows.len() <= 1);
}

#[test]
fn parse_order_by_asc_desc() {
    let r = exec("UNWIND [3,1,2] AS x RETURN x ORDER BY x ASC");
    let xs: Vec<i64> = r
        .rows
        .iter()
        .map(|row| match &row["x"] {
            Value::Int(i) => *i,
            _ => -1,
        })
        .collect();
    assert_eq!(xs, vec![1, 2, 3]);

    let r2 = exec("UNWIND [3,1,2] AS x RETURN x ORDER BY x DESC");
    let xs2: Vec<i64> = r2
        .rows
        .iter()
        .map(|row| match &row["x"] {
            Value::Int(i) => *i,
            _ => -1,
        })
        .collect();
    assert_eq!(xs2, vec![3, 2, 1]);
}

#[test]
fn parse_skip_limit() {
    let r = exec("UNWIND [1,2,3,4,5] AS x RETURN x SKIP 1 LIMIT 2");
    assert_eq!(r.rows.len(), 2);
}

#[test]
fn parse_distinct() {
    let r = exec("UNWIND [1,2,2,3,3,3] AS x RETURN DISTINCT x");
    assert_eq!(r.rows.len(), 3);
}

#[test]
fn parse_aggregation_count() {
    let r = exec("UNWIND [1,2,3,4,5] AS x RETURN count(x) AS c");
    assert_eq!(r.rows[0]["c"], Value::Int(5));
}

#[test]
fn parse_aggregation_sum() {
    let r = exec("UNWIND [1,2,3,4,5] AS x RETURN sum(x) AS s");
    assert_eq!(r.rows[0]["s"], Value::Int(15));
}

#[test]
fn parse_aggregation_avg() {
    let r = exec("UNWIND [2,4,6] AS x RETURN avg(x) AS a");
    if let Value::Float(f) = &r.rows[0]["a"] {
        assert!((f - 4.0).abs() < 1e-9);
    } else {
        panic!()
    }
}

#[test]
fn parse_aggregation_min_max() {
    let r = exec("UNWIND [5,2,8,1] AS x RETURN min(x) AS lo, max(x) AS hi");
    assert_eq!(r.rows[0]["lo"], Value::Int(1));
    assert_eq!(r.rows[0]["hi"], Value::Int(8));
}

#[test]
fn parse_aggregation_collect() {
    let r = exec("UNWIND [1,2,3] AS x RETURN collect(x) AS l");
    assert_eq!(
        r.rows[0]["l"],
        Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)])
    );
}

// ── element function tests via Cypher ──────────────────────────────────────

#[test]
fn elem_id_of_node() {
    let store = fresh_store();
    exec_on(&store, "CREATE (a:A {name:'x'})");
    let r = exec_on(&store, "MATCH (n:A) RETURN id(n) AS i");
    assert_eq!(r.rows.len(), 1);
    assert!(matches!(r.rows[0]["i"], Value::Int(_)));
}

#[test]
fn elem_labels_of_node() {
    let store = fresh_store();
    exec_on(&store, "CREATE (a:A:B {name:'x'})");
    let r = exec_on(&store, "MATCH (n:A) RETURN labels(n) AS l");
    if let Value::List(items) = &r.rows[0]["l"] {
        let names: Vec<String> = items
            .iter()
            .filter_map(|v| match v {
                Value::Str(s) => Some(s.to_string()),
                _ => None,
            })
            .collect();
        assert!(names.contains(&"A".to_string()));
        assert!(names.contains(&"B".to_string()));
    } else {
        panic!()
    }
}

#[test]
fn elem_type_of_relationship() {
    let store = fresh_store();
    exec_on(&store, "CREATE (a:A)-[:KNOWS]->(b:B)");
    let r = exec_on(&store, "MATCH ()-[r:KNOWS]->() RETURN type(r) AS t");
    assert_eq!(r.rows[0]["t"], Value::Str("KNOWS".into()));
}

#[test]
fn elem_properties_of_node() {
    let store = fresh_store();
    exec_on(&store, "CREATE (a:A {x: 1, y: 'two'})");
    let r = exec_on(&store, "MATCH (n:A) RETURN properties(n) AS p");
    if let Value::Map(m) = &r.rows[0]["p"] {
        assert_eq!(m.get("x"), Some(&Value::Int(1)));
        assert_eq!(m.get("y"), Some(&Value::Str("two".into())));
    } else {
        panic!()
    }
}

#[test]
fn elem_keys_of_node() {
    let store = fresh_store();
    exec_on(&store, "CREATE (a:A {x: 1, y: 2, z: 3})");
    let r = exec_on(&store, "MATCH (n:A) RETURN keys(n) AS k");
    if let Value::List(items) = &r.rows[0]["k"] {
        assert_eq!(items.len(), 3);
    } else {
        panic!()
    }
}

#[test]
fn elem_property_access_missing_returns_null() {
    let store = fresh_store();
    exec_on(&store, "CREATE (a:A {x: 1})");
    let r = exec_on(&store, "MATCH (n:A) RETURN n.missing AS m");
    assert_eq!(r.rows[0]["m"], Value::Null);
}

#[test]
fn elem_has_label_predicate() {
    let store = fresh_store();
    exec_on(&store, "CREATE (a:A:B), (c:C)");
    let r = exec_on(&store, "MATCH (n) WHERE n:A RETURN count(n) AS c");
    assert_eq!(r.rows[0]["c"], Value::Int(1));
}

#[test]
fn elem_has_label_compound() {
    let store = fresh_store();
    exec_on(&store, "CREATE (a:A:B), (c:A)");
    let r = exec_on(&store, "MATCH (n) WHERE n:A AND n:B RETURN count(n) AS c");
    assert_eq!(r.rows[0]["c"], Value::Int(1));
}

// ── shaping/projection tests ───────────────────────────────────────────────

#[test]
fn shaping_alias_required() {
    let r = exec("RETURN 1 + 2 AS sum");
    assert_eq!(r.rows[0]["sum"], Value::Int(3));
}

#[test]
fn shaping_multiple_columns() {
    let r = exec("RETURN 1 AS a, 'x' AS b, true AS c");
    assert_eq!(r.rows[0]["a"], Value::Int(1));
    assert_eq!(r.rows[0]["b"], Value::Str("x".into()));
    assert_eq!(r.rows[0]["c"], Value::Bool(true));
}

#[test]
fn shaping_star_projection() {
    let r = exec("WITH 1 AS a, 2 AS b RETURN *");
    assert_eq!(r.rows.len(), 1);
    assert_eq!(r.rows[0]["a"], Value::Int(1));
    assert_eq!(r.rows[0]["b"], Value::Int(2));
}

// ── mutation/MERGE tests ───────────────────────────────────────────────────

#[test]
fn merge_creates_when_missing() {
    let store = fresh_store();
    exec_on(&store, "MERGE (a:Person {name:'Alice'})");
    let r = exec_on(
        &store,
        "MATCH (p:Person {name:'Alice'}) RETURN count(p) AS c",
    );
    assert_eq!(r.rows[0]["c"], Value::Int(1));
}

#[test]
fn merge_matches_when_present() {
    let store = fresh_store();
    exec_on(&store, "CREATE (a:Person {name:'Bob'})");
    exec_on(&store, "MERGE (a:Person {name:'Bob'})");
    let r = exec_on(&store, "MATCH (p:Person {name:'Bob'}) RETURN count(p) AS c");
    assert_eq!(r.rows[0]["c"], Value::Int(1));
}

#[test]
fn merge_on_create_set() {
    let store = fresh_store();
    exec_on(
        &store,
        "MERGE (a:Person {name:'Carol'}) ON CREATE SET a.age = 30",
    );
    let r = exec_on(&store, "MATCH (p:Person) RETURN p.age AS age");
    assert_eq!(r.rows[0]["age"], Value::Int(30));
}

#[test]
fn merge_on_match_set() {
    let store = fresh_store();
    exec_on(&store, "CREATE (a:Person {name:'Dan', age: 20})");
    exec_on(
        &store,
        "MERGE (a:Person {name:'Dan'}) ON MATCH SET a.age = 99",
    );
    let r = exec_on(&store, "MATCH (p:Person) RETURN p.age AS age");
    assert_eq!(r.rows[0]["age"], Value::Int(99));
}

#[test]
fn merge_relationship_creates_endpoints() {
    let store = fresh_store();
    exec_on(
        &store,
        "MERGE (a:A {name:'a'}) MERGE (b:B {name:'b'}) MERGE (a)-[:R]->(b)",
    );
    let r = exec_on(&store, "MATCH (a)-[:R]->(b) RETURN count(*) AS c");
    assert_eq!(r.rows[0]["c"], Value::Int(1));
}

#[test]
fn create_then_match_basic() {
    let store = fresh_store();
    exec_on(&store, "CREATE (:N {x:1}), (:N {x:2}), (:N {x:3})");
    let r = exec_on(&store, "MATCH (n:N) RETURN count(n) AS c");
    assert_eq!(r.rows[0]["c"], Value::Int(3));
}

#[test]
fn set_property_simple() {
    let store = fresh_store();
    exec_on(&store, "CREATE (a:A {v: 1})");
    exec_on(&store, "MATCH (a:A) SET a.v = 99");
    let r = exec_on(&store, "MATCH (a:A) RETURN a.v AS v");
    assert_eq!(r.rows[0]["v"], Value::Int(99));
}

#[test]
fn set_label() {
    let store = fresh_store();
    exec_on(&store, "CREATE (a:A)");
    exec_on(&store, "MATCH (a:A) SET a:B");
    let r = exec_on(&store, "MATCH (a:B) RETURN count(a) AS c");
    assert_eq!(r.rows[0]["c"], Value::Int(1));
}

#[test]
fn remove_property() {
    let store = fresh_store();
    exec_on(&store, "CREATE (a:A {x: 1, y: 2})");
    exec_on(&store, "MATCH (a:A) REMOVE a.x");
    let r = exec_on(&store, "MATCH (a:A) RETURN a.x AS x, a.y AS y");
    assert_eq!(r.rows[0]["x"], Value::Null);
    assert_eq!(r.rows[0]["y"], Value::Int(2));
}

#[test]
fn remove_label() {
    let store = fresh_store();
    exec_on(&store, "CREATE (a:A:B)");
    exec_on(&store, "MATCH (a:A) REMOVE a:B");
    let r = exec_on(&store, "MATCH (a:B) RETURN count(a) AS c");
    assert_eq!(r.rows[0]["c"], Value::Int(0));
}

#[test]
fn delete_node() {
    let store = fresh_store();
    exec_on(&store, "CREATE (a:Tmp), (b:Tmp), (c:Keep)");
    exec_on(&store, "MATCH (a:Tmp) DELETE a");
    let r = exec_on(&store, "MATCH (n) RETURN count(n) AS c");
    assert_eq!(r.rows[0]["c"], Value::Int(1));
}

#[test]
fn detach_delete_with_edges() {
    let store = fresh_store();
    exec_on(&store, "CREATE (a:A)-[:R]->(b:B)");
    exec_on(&store, "MATCH (a:A) DETACH DELETE a");
    let r = exec_on(&store, "MATCH (n) RETURN count(n) AS c");
    assert_eq!(r.rows[0]["c"], Value::Int(1));
}

// ── parameter tests ────────────────────────────────────────────────────────

#[test]
fn param_substitution() {
    let store = fresh_store();
    let registry = ProcedureRegistry::new();
    let mut params = HashMap::new();
    params.insert("n".to_string(), Value::Int(42));
    let q = parse_query("RETURN $n AS x").unwrap();
    let plan = QueryPlanner::with_context(&store, "g", &registry, &params)
        .plan(&q)
        .unwrap();
    let exec = QueryExecutor::with_context(&store, &registry, params);
    let mut b = unlimited();
    let r = exec.execute("g", &plan, &mut b).unwrap();
    assert_eq!(r.rows[0]["x"], Value::Int(42));
}

// ── temporal tests ─────────────────────────────────────────────────────────

#[test]
fn temporal_date_literal() {
    let v = val("RETURN date('2024-01-15')");
    assert!(matches!(v, Value::Date(_)));
}

#[test]
fn temporal_datetime_literal() {
    let v = val("RETURN datetime('2024-01-15T10:30:00Z')");
    assert!(matches!(v, Value::DateTime(_)));
}

#[test]
fn temporal_localdatetime_literal() {
    let v = val("RETURN localdatetime('2024-01-15T10:30:00')");
    assert!(matches!(v, Value::LocalDateTime(_)));
}

#[test]
fn temporal_time_literal() {
    let v = val("RETURN time('10:30:00')");
    assert!(matches!(v, Value::Time(_, _)));
}

#[test]
fn temporal_localtime_literal() {
    let v = val("RETURN localtime('10:30:00')");
    assert!(matches!(v, Value::LocalTime(_)));
}

#[test]
fn temporal_duration_literal() {
    let v = val("RETURN duration('P1Y2M3D')");
    assert!(matches!(v, Value::Duration(_)));
}

#[test]
fn temporal_duration_between_dates() {
    let v = val("RETURN duration.between(date('2024-01-01'), date('2024-12-31'))");
    assert!(matches!(v, Value::Duration(_)));
}

#[test]
fn temporal_duration_in_days() {
    let v = val("RETURN duration.inDays(date('2024-01-01'), date('2024-01-11'))");
    assert!(matches!(v, Value::Duration(_)));
}

#[test]
fn temporal_duration_in_seconds() {
    let v = val(
        "RETURN duration.inSeconds(datetime('2024-01-01T00:00:00Z'), datetime('2024-01-01T00:01:00Z'))",
    );
    assert!(matches!(v, Value::Duration(_)));
}

#[test]
fn temporal_date_arithmetic_add_duration() {
    let v = val("RETURN date('2024-01-01') + duration('P10D')");
    assert!(matches!(v, Value::Date(_)));
}

#[test]
fn temporal_date_arithmetic_sub_duration() {
    let v = val("RETURN date('2024-01-15') - duration('P5D')");
    assert!(matches!(v, Value::Date(_)));
}

#[test]
fn temporal_date_truncate_year() {
    let v = val("RETURN date.truncate('year', date('2024-07-15'))");
    assert!(matches!(v, Value::Date(_)));
}

#[test]
fn temporal_date_truncate_month() {
    let v = val("RETURN date.truncate('month', date('2024-07-15'))");
    assert!(matches!(v, Value::Date(_)));
}

#[test]
fn temporal_date_components() {
    let store = fresh_store();
    let r = exec_on(
        &store,
        "WITH date('2024-07-15') AS d RETURN d.year AS y, d.month AS m, d.day AS dy",
    );
    assert_eq!(r.rows[0]["y"], Value::Int(2024));
    assert_eq!(r.rows[0]["m"], Value::Int(7));
    assert_eq!(r.rows[0]["dy"], Value::Int(15));
}

// ── builtins additional coverage ───────────────────────────────────────────

#[test]
fn builtin_left_right() {
    assert_eq!(val("RETURN left('hello', 3)"), Value::Str("hel".into()));
    assert_eq!(val("RETURN right('hello', 3)"), Value::Str("llo".into()));
}

#[test]
fn builtin_substring_no_length() {
    assert_eq!(
        val("RETURN substring('hello', 2)"),
        Value::Str("llo".into())
    );
}

#[test]
fn builtin_substring_with_length() {
    assert_eq!(
        val("RETURN substring('hello', 1, 3)"),
        Value::Str("ell".into())
    );
}

#[test]
fn builtin_replace() {
    assert_eq!(
        val("RETURN replace('hello', 'l', 'X')"),
        Value::Str("heXXo".into())
    );
}

#[test]
fn builtin_split() {
    assert_eq!(
        val("RETURN split('a,b,c', ',')"),
        Value::List(vec![
            Value::Str("a".into()),
            Value::Str("b".into()),
            Value::Str("c".into()),
        ])
    );
}

#[test]
fn builtin_trim_variants() {
    assert_eq!(val("RETURN trim('  hi  ')"), Value::Str("hi".into()));
    assert_eq!(val("RETURN ltrim('  hi  ')"), Value::Str("hi  ".into()));
    assert_eq!(val("RETURN rtrim('  hi  ')"), Value::Str("  hi".into()));
}

#[test]
fn builtin_upper_lower() {
    assert_eq!(val("RETURN toUpper('abc')"), Value::Str("ABC".into()));
    assert_eq!(val("RETURN toLower('ABC')"), Value::Str("abc".into()));
}

#[test]
fn builtin_to_string() {
    assert_eq!(val("RETURN toString(123)"), Value::Str("123".into()));
    assert_eq!(val("RETURN toString(true)"), Value::Str("true".into()));
}

#[test]
fn builtin_to_integer_from_string() {
    assert_eq!(val("RETURN toInteger('42')"), Value::Int(42));
    assert_eq!(val("RETURN toInteger('3.7')"), Value::Int(3));
    assert_eq!(val("RETURN toInteger('abc')"), Value::Null);
}

#[test]
fn builtin_to_float_from_string() {
    if let Value::Float(f) = val("RETURN toFloat('1.5')") {
        assert!((f - 1.5).abs() < 1e-9);
    } else {
        panic!()
    }
    assert_eq!(val("RETURN toFloat('xyz')"), Value::Null);
}

#[test]
fn builtin_to_boolean() {
    assert_eq!(val("RETURN toBoolean('true')"), Value::Bool(true));
    assert_eq!(val("RETURN toBoolean('FALSE')"), Value::Bool(false));
    assert_eq!(val("RETURN toBoolean('xyz')"), Value::Null);
}

#[test]
fn builtin_atan2() {
    if let Value::Float(f) = val("RETURN atan2(0, 1)") {
        assert!(f.abs() < 1e-9);
    } else {
        panic!()
    }
}

#[test]
fn builtin_degrees_radians_roundtrip() {
    if let Value::Float(f) = val("RETURN degrees(radians(180))") {
        assert!((f - 180.0).abs() < 1e-6);
    } else {
        panic!()
    }
}

#[test]
fn builtin_haversin_zero() {
    if let Value::Float(f) = val("RETURN haversin(0)") {
        assert!(f.abs() < 1e-9);
    } else {
        panic!()
    }
}

#[test]
fn builtin_size_str_unicode() {
    // Size counts characters, not bytes
    assert_eq!(val("RETURN size('héllo')"), Value::Int(5));
}

#[test]
fn builtin_keys_of_map() {
    let r = val("RETURN keys({a: 1, b: 2, c: 3})");
    if let Value::List(items) = r {
        assert_eq!(items.len(), 3);
    } else {
        panic!()
    }
}

#[test]
fn builtin_range_descending() {
    assert_eq!(
        val("RETURN range(5, 1, -1)"),
        Value::List(vec![
            Value::Int(5),
            Value::Int(4),
            Value::Int(3),
            Value::Int(2),
            Value::Int(1),
        ])
    );
}

// ── planner-level helpers ──────────────────────────────────────────────────

#[test]
fn planner_collect_and_estimate() {
    let store = fresh_store();
    exec_on(&store, "CREATE (:A), (:A), (:B)-[:R]->(:B)");
    let stats = collect_graph_statistics(&store, "g").unwrap();
    assert!(stats.total_nodes >= 4);
    assert!(stats.total_edges >= 1);

    // Plan a simple query and estimate cardinality.
    let registry = ProcedureRegistry::new();
    let params = HashMap::new();
    let q = parse_query("MATCH (n:A) RETURN n").unwrap();
    let plan = QueryPlanner::with_context(&store, "g", &registry, &params)
        .plan(&q)
        .unwrap();
    let est = estimate_cardinality(&plan, &stats);
    assert!(!est.is_empty());
}

// ── union and combinators ──────────────────────────────────────────────────

#[test]
fn union_combines_rows() {
    let r = exec("RETURN 1 AS x UNION RETURN 2 AS x");
    assert_eq!(r.rows.len(), 2);
}

#[test]
fn union_all_keeps_duplicates() {
    let r = exec("RETURN 1 AS x UNION ALL RETURN 1 AS x");
    assert_eq!(r.rows.len(), 2);
}

// ── optional match ─────────────────────────────────────────────────────────

#[test]
fn optional_match_yields_null_when_missing() {
    let store = fresh_store();
    exec_on(&store, "CREATE (:A)");
    let r = exec_on(
        &store,
        "MATCH (a:A) OPTIONAL MATCH (a)-[r:R]->(b) RETURN a, r, b",
    );
    assert_eq!(r.rows.len(), 1);
    assert_eq!(r.rows[0]["r"], Value::Null);
    assert_eq!(r.rows[0]["b"], Value::Null);
}

// ── exists subquery ────────────────────────────────────────────────────────

#[test]
fn exists_subquery_true() {
    let store = fresh_store();
    exec_on(&store, "CREATE (a:A)-[:R]->(b:B)");
    let r = exec_on(&store, "MATCH (a:A) RETURN exists { (a)-[:R]->() } AS e");
    assert_eq!(r.rows[0]["e"], Value::Bool(true));
}

#[test]
fn exists_subquery_false() {
    let store = fresh_store();
    exec_on(&store, "CREATE (a:A)");
    let r = exec_on(&store, "MATCH (a:A) RETURN exists { (a)-[:R]->() } AS e");
    assert_eq!(r.rows[0]["e"], Value::Bool(false));
}

// ── varlen path ────────────────────────────────────────────────────────────

#[test]
fn varlen_expand_path() {
    let store = fresh_store();
    exec_on(
        &store,
        "CREATE (a:N {n:1})-[:R]->(b:N {n:2})-[:R]->(c:N {n:3})",
    );
    let r = exec_on(
        &store,
        "MATCH (a:N {n:1})-[:R*1..3]->(z:N) RETURN z.n AS n ORDER BY n",
    );
    assert_eq!(r.rows.len(), 2);
}

#[test]
fn shortest_path_simple() {
    let store = fresh_store();
    exec_on(
        &store,
        "CREATE (a:N {n:1})-[:R]->(b:N {n:2})-[:R]->(c:N {n:3})",
    );
    let r = exec_on(
        &store,
        "MATCH p = shortestPath((a:N {n:1})-[:R*]->(c:N {n:3})) RETURN length(p) AS l",
    );
    assert_eq!(r.rows[0]["l"], Value::Int(2));
}

// ── pattern comprehension ──────────────────────────────────────────────────

#[test]
fn pattern_comprehension() {
    let store = fresh_store();
    exec_on(
        &store,
        "CREATE (a:A)-[:R]->(b:B {name:'x'}), (a)-[:R]->(c:B {name:'y'})",
    );
    let r = exec_on(
        &store,
        "MATCH (a:A) RETURN [(a)-[:R]->(b) | b.name] AS names",
    );
    if let Value::List(items) = &r.rows[0]["names"] {
        assert_eq!(items.len(), 2);
    } else {
        panic!()
    }
}

// ── coalesce builtin chain ─────────────────────────────────────────────────

#[test]
fn coalesce_chain() {
    assert_eq!(val("RETURN coalesce(null, null, 5)"), Value::Int(5));
    assert_eq!(val("RETURN coalesce(null, null, null)"), Value::Null);
}

#[test]
fn isnull_function() {
    assert_eq!(val("RETURN isNull(null)"), Value::Bool(true));
    assert_eq!(val("RETURN isNull(1)"), Value::Bool(false));
}
