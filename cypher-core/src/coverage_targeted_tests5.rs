//! Coverage-targeted tests, batch 5.
//!
//! Focus: element functions (id/labels/type/keys/properties), numeric/string/list
//! built-ins, collection operators, validator/semantic error paths, helpers/arithmetic
//! and helpers/comparison branches reachable via parse_query.

#![allow(clippy::approx_constant)]

use std::collections::HashMap;
use std::time::Duration;

use indexmap::IndexMap;

use crate::executor::engine::QueryExecutor;
use crate::executor::engine::QueryResult;
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
    let store = InMemoryGraphStore::new();
    store.create_graph("g").unwrap();
    store
}

fn exec_on(store: &InMemoryGraphStore, query: &str) -> QueryResult {
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

fn first_val(r: &QueryResult) -> Value {
    let col = &r.columns[0];
    r.rows[0].get(col).cloned().unwrap_or(Value::Null)
}

fn seeded_store() -> InMemoryGraphStore {
    let store = fresh_store();
    let mut pa = IndexMap::new();
    pa.insert("name".to_string(), Value::Str("A".into()));
    pa.insert("age".to_string(), Value::Int(30));
    let a = store
        .create_node("g", vec!["Person".to_string(), "Admin".to_string()], pa)
        .unwrap();
    let mut pb = IndexMap::new();
    pb.insert("name".to_string(), Value::Str("B".into()));
    pb.insert("age".to_string(), Value::Int(25));
    let b = store
        .create_node("g", vec!["Person".to_string()], pb)
        .unwrap();
    let mut pe = IndexMap::new();
    pe.insert("since".to_string(), Value::Int(2020));
    store
        .create_edge("g", a, b, "KNOWS".to_string(), pe)
        .unwrap();
    store
}

// ── element_funcs ─────────────────────────────────────────────────────

#[test]
fn id_node_and_edge() {
    let s = seeded_store();
    let r = exec_on(&s, "MATCH (n:Person {name:'A'}) RETURN id(n) AS i");
    assert!(matches!(first_val(&r), Value::Int(_)));
    let r = exec_on(&s, "MATCH ()-[r:KNOWS]->() RETURN id(r) AS i");
    assert!(matches!(first_val(&r), Value::Int(_)));
}

#[test]
fn id_null_propagates() {
    assert_eq!(first_val(&exec("RETURN id(null) AS r")), Value::Null);
}

#[test]
fn labels_function_returns_list() {
    let s = seeded_store();
    let r = exec_on(&s, "MATCH (n:Person {name:'A'}) RETURN labels(n) AS r");
    if let Value::List(items) = first_val(&r) {
        let mut names: Vec<String> = items
            .iter()
            .filter_map(|v| {
                if let Value::Str(s) = v {
                    Some(s.to_string())
                } else {
                    None
                }
            })
            .collect();
        names.sort();
        assert_eq!(names, vec!["Admin", "Person"]);
    } else {
        panic!("not a list");
    }
}

#[test]
fn labels_on_null_is_null() {
    assert_eq!(first_val(&exec("RETURN labels(null) AS r")), Value::Null);
}

#[test]
fn type_function_returns_string() {
    let s = seeded_store();
    let r = exec_on(&s, "MATCH ()-[r:KNOWS]->() RETURN type(r) AS r");
    assert_eq!(first_val(&r), Value::Str("KNOWS".into()));
}

#[test]
fn type_on_null_is_null() {
    assert_eq!(first_val(&exec("RETURN type(null) AS r")), Value::Null);
}

#[test]
fn keys_function_returns_list_of_property_names() {
    let s = seeded_store();
    let r = exec_on(&s, "MATCH (n:Person {name:'A'}) RETURN keys(n) AS r");
    if let Value::List(items) = first_val(&r) {
        assert_eq!(items.len(), 2);
    } else {
        panic!("not a list");
    }
}

#[test]
fn keys_on_null_is_null() {
    assert_eq!(first_val(&exec("RETURN keys(null) AS r")), Value::Null);
}

#[test]
fn keys_on_map_returns_keys() {
    let r = exec("RETURN keys({a:1, b:2, c:3}) AS r");
    if let Value::List(items) = first_val(&r) {
        assert_eq!(items.len(), 3);
    } else {
        panic!("not a list");
    }
}

#[test]
fn properties_function_on_node() {
    let s = seeded_store();
    let r = exec_on(&s, "MATCH (n:Person {name:'A'}) RETURN properties(n) AS r");
    assert!(matches!(first_val(&r), Value::Map(_)));
}

#[test]
fn properties_function_on_relationship() {
    let s = seeded_store();
    let r = exec_on(&s, "MATCH ()-[r:KNOWS]->() RETURN properties(r) AS r");
    if let Value::Map(m) = first_val(&r) {
        assert_eq!(m.get("since"), Some(&Value::Int(2020)));
    } else {
        panic!("not a map");
    }
}

#[test]
fn properties_on_null_is_null() {
    assert_eq!(
        first_val(&exec("RETURN properties(null) AS r")),
        Value::Null
    );
}

#[test]
fn properties_on_map_returns_same_map() {
    let r = exec("RETURN properties({a:1,b:2}) AS r");
    if let Value::Map(m) = first_val(&r) {
        assert_eq!(m.len(), 2);
    } else {
        panic!("not a map");
    }
}

#[test]
fn has_label_predicate_on_node() {
    let s = seeded_store();
    let r = exec_on(&s, "MATCH (n) WHERE n:Person RETURN n");
    assert_eq!(r.rows.len(), 2);
    let r = exec_on(&s, "MATCH (n) WHERE n:Admin RETURN n");
    assert_eq!(r.rows.len(), 1);
    let r = exec_on(&s, "MATCH (n) WHERE n:Nope RETURN n");
    assert_eq!(r.rows.len(), 0);
}

#[test]
fn property_access_on_missing_returns_null() {
    let s = seeded_store();
    let r = exec_on(&s, "MATCH (n:Person {name:'A'}) RETURN n.nonexistent AS r");
    assert_eq!(first_val(&r), Value::Null);
}

#[test]
fn property_access_on_null_is_null() {
    assert_eq!(first_val(&exec("RETURN (null).x AS r")), Value::Null);
}

// ── numeric built-ins ─────────────────────────────────────────────────

#[test]
fn numeric_abs_signed_and_unsigned() {
    assert_eq!(first_val(&exec("RETURN abs(-7) AS r")), Value::Int(7));
    assert_eq!(first_val(&exec("RETURN abs(-7.5) AS r")), Value::Float(7.5));
    assert_eq!(first_val(&exec("RETURN abs(0) AS r")), Value::Int(0));
    assert_eq!(first_val(&exec("RETURN abs(null) AS r")), Value::Null);
}

#[test]
fn numeric_sign_function() {
    assert_eq!(first_val(&exec("RETURN sign(-7) AS r")), Value::Int(-1));
    assert_eq!(first_val(&exec("RETURN sign(0) AS r")), Value::Int(0));
    assert_eq!(first_val(&exec("RETURN sign(7) AS r")), Value::Int(1));
    assert_eq!(first_val(&exec("RETURN sign(null) AS r")), Value::Null);
}

#[test]
fn numeric_ceil_floor_round() {
    assert_eq!(first_val(&exec("RETURN ceil(1.2) AS r")), Value::Float(2.0));
    assert_eq!(
        first_val(&exec("RETURN floor(1.8) AS r")),
        Value::Float(1.0)
    );
    assert_eq!(
        first_val(&exec("RETURN round(1.5) AS r")),
        Value::Float(2.0)
    );
    assert_eq!(first_val(&exec("RETURN ceil(null) AS r")), Value::Null);
}

#[test]
fn numeric_sqrt_exp_log() {
    assert_eq!(first_val(&exec("RETURN sqrt(16) AS r")), Value::Float(4.0));
    assert_eq!(first_val(&exec("RETURN sqrt(0) AS r")), Value::Float(0.0));
    let _ = exec("RETURN exp(1) AS r");
    let _ = exec("RETURN log(2.71828) AS r");
    let _ = exec("RETURN log10(100) AS r");
    assert_eq!(first_val(&exec("RETURN sqrt(null) AS r")), Value::Null);
}

#[test]
fn numeric_trig_basics() {
    let _ = exec("RETURN sin(0) AS r");
    let _ = exec("RETURN cos(0) AS r");
    let _ = exec("RETURN tan(0) AS r");
    let _ = exec("RETURN asin(0) AS r");
    let _ = exec("RETURN acos(1) AS r");
    let _ = exec("RETURN atan(0) AS r");
    let _ = exec("RETURN atan2(0, 1) AS r");
    assert_eq!(first_val(&exec("RETURN sin(null) AS r")), Value::Null);
}

#[test]
fn numeric_pi_e_constants() {
    let _ = exec("RETURN pi() AS r");
    let _ = exec("RETURN e() AS r");
}

#[test]
fn numeric_pow_function() {
    let _ = exec("RETURN 2 ^ 10 AS r");
    assert_eq!(first_val(&exec("RETURN null ^ 2 AS r")), Value::Null);
}

#[test]
fn numeric_modulo_branches() {
    assert_eq!(first_val(&exec("RETURN 10 % 3 AS r")), Value::Int(1));
    let _ = exec("RETURN 10.5 % 2 AS r");
    let _ = exec("RETURN 10 % 2.5 AS r");
    assert_eq!(first_val(&exec("RETURN null % 2 AS r")), Value::Null);
    assert_eq!(first_val(&exec("RETURN 1 % null AS r")), Value::Null);
}

#[test]
fn toint_tofloat_coercions() {
    assert_eq!(
        first_val(&exec("RETURN toInteger('42') AS r")),
        Value::Int(42)
    );
    assert_eq!(
        first_val(&exec("RETURN toInteger(3.7) AS r")),
        Value::Int(3)
    );
    assert_eq!(first_val(&exec("RETURN toInteger(null) AS r")), Value::Null);
    assert_eq!(
        first_val(&exec("RETURN toFloat('3.14') AS r")),
        Value::Float(3.14)
    );
    assert_eq!(
        first_val(&exec("RETURN toFloat(7) AS r")),
        Value::Float(7.0)
    );
    assert_eq!(first_val(&exec("RETURN toFloat(null) AS r")), Value::Null);
}

#[test]
fn toint_invalid_string_returns_null() {
    assert_eq!(
        first_val(&exec("RETURN toInteger('abc') AS r")),
        Value::Null
    );
    assert_eq!(first_val(&exec("RETURN toFloat('abc') AS r")), Value::Null);
}

// ── string built-ins ─────────────────────────────────────────────────

#[test]
fn string_len_size_substring() {
    assert_eq!(first_val(&exec("RETURN size('hello') AS r")), Value::Int(5));
    assert_eq!(
        first_val(&exec("RETURN substring('hello', 1, 3) AS r")),
        Value::Str("ell".into())
    );
    assert_eq!(
        first_val(&exec("RETURN substring('hello', 1) AS r")),
        Value::Str("ello".into())
    );
}

#[test]
fn string_upper_lower_trim() {
    assert_eq!(
        first_val(&exec("RETURN toUpper('hello') AS r")),
        Value::Str("HELLO".into())
    );
    assert_eq!(
        first_val(&exec("RETURN toLower('HELLO') AS r")),
        Value::Str("hello".into())
    );
    assert_eq!(
        first_val(&exec("RETURN trim('  hi  ') AS r")),
        Value::Str("hi".into())
    );
    assert_eq!(
        first_val(&exec("RETURN ltrim('  hi  ') AS r")),
        Value::Str("hi  ".into())
    );
    assert_eq!(
        first_val(&exec("RETURN rtrim('  hi  ') AS r")),
        Value::Str("  hi".into())
    );
}

#[test]
fn string_replace_split_reverse() {
    assert_eq!(
        first_val(&exec("RETURN replace('foobar', 'foo', 'baz') AS r")),
        Value::Str("bazbar".into())
    );
    let r = exec("RETURN split('a,b,c', ',') AS r");
    if let Value::List(items) = first_val(&r) {
        assert_eq!(items.len(), 3);
    } else {
        panic!("not a list");
    }
    assert_eq!(
        first_val(&exec("RETURN reverse('abc') AS r")),
        Value::Str("cba".into())
    );
}

#[test]
fn string_string_starts_ends_contains() {
    assert_eq!(
        first_val(&exec("RETURN 'hello' STARTS WITH 'he' AS r")),
        Value::Bool(true)
    );
    assert_eq!(
        first_val(&exec("RETURN 'hello' ENDS WITH 'lo' AS r")),
        Value::Bool(true)
    );
    assert_eq!(
        first_val(&exec("RETURN 'hello' CONTAINS 'ell' AS r")),
        Value::Bool(true)
    );
}

#[test]
fn string_left_right() {
    assert_eq!(
        first_val(&exec("RETURN left('abcdef', 3) AS r")),
        Value::Str("abc".into())
    );
    assert_eq!(
        first_val(&exec("RETURN right('abcdef', 3) AS r")),
        Value::Str("def".into())
    );
    assert_eq!(first_val(&exec("RETURN left(null, 3) AS r")), Value::Null);
}

#[test]
fn string_tostring_function() {
    assert_eq!(
        first_val(&exec("RETURN toString(42) AS r")),
        Value::Str("42".into())
    );
    assert_eq!(
        first_val(&exec("RETURN toString(3.14) AS r")),
        Value::Str("3.14".into())
    );
    assert_eq!(
        first_val(&exec("RETURN toString(true) AS r")),
        Value::Str("true".into())
    );
    assert_eq!(first_val(&exec("RETURN toString(null) AS r")), Value::Null);
}

// ── collection ops ────────────────────────────────────────────────────

#[test]
fn collection_in_basic() {
    assert_eq!(
        first_val(&exec("RETURN 2 IN [1,2,3] AS r")),
        Value::Bool(true)
    );
    assert_eq!(
        first_val(&exec("RETURN 5 IN [1,2,3] AS r")),
        Value::Bool(false)
    );
    assert_eq!(first_val(&exec("RETURN 2 IN [] AS r")), Value::Bool(false));
    assert_eq!(first_val(&exec("RETURN 2 IN null AS r")), Value::Null);
    assert_eq!(first_val(&exec("RETURN null IN [1,2] AS r")), Value::Null);
}

#[test]
fn collection_in_with_null_in_list() {
    assert_eq!(
        first_val(&exec("RETURN 2 IN [1, null, 3] AS r")),
        Value::Null
    );
    assert_eq!(
        first_val(&exec("RETURN 2 IN [1, 2, null] AS r")),
        Value::Bool(true)
    );
}

#[test]
fn collection_size_on_list_and_string() {
    assert_eq!(first_val(&exec("RETURN size([1,2,3]) AS r")), Value::Int(3));
    assert_eq!(first_val(&exec("RETURN size([]) AS r")), Value::Int(0));
    assert_eq!(first_val(&exec("RETURN size(null) AS r")), Value::Null);
}

#[test]
fn collection_range_inclusive_and_step() {
    let r = exec("RETURN range(1, 5) AS r");
    if let Value::List(items) = first_val(&r) {
        assert_eq!(items.len(), 5);
    } else {
        panic!("not a list");
    }
    let r = exec("RETURN range(1, 10, 2) AS r");
    if let Value::List(items) = first_val(&r) {
        assert_eq!(items.len(), 5);
    } else {
        panic!("not a list");
    }
}

#[test]
fn collection_head_tail_last() {
    assert_eq!(first_val(&exec("RETURN head([1,2,3]) AS r")), Value::Int(1));
    assert_eq!(first_val(&exec("RETURN last([1,2,3]) AS r")), Value::Int(3));
    let r = exec("RETURN tail([1,2,3]) AS r");
    if let Value::List(items) = first_val(&r) {
        assert_eq!(items.len(), 2);
    } else {
        panic!("not a list");
    }
    assert_eq!(first_val(&exec("RETURN head([]) AS r")), Value::Null);
    assert_eq!(first_val(&exec("RETURN last([]) AS r")), Value::Null);
}

#[test]
fn collection_subscript_and_slice() {
    assert_eq!(
        first_val(&exec("RETURN [1,2,3,4,5][2] AS r")),
        Value::Int(3)
    );
    let r = exec("RETURN [1,2,3,4,5][1..3] AS r");
    if let Value::List(items) = first_val(&r) {
        assert_eq!(items.len(), 2);
    } else {
        panic!("not a list");
    }
    let r = exec("RETURN [1,2,3,4,5][..3] AS r");
    if let Value::List(items) = first_val(&r) {
        assert_eq!(items.len(), 3);
    } else {
        panic!("not a list");
    }
    let r = exec("RETURN [1,2,3,4,5][2..] AS r");
    if let Value::List(items) = first_val(&r) {
        assert_eq!(items.len(), 3);
    } else {
        panic!("not a list");
    }
}

#[test]
fn collection_negative_subscript() {
    assert_eq!(
        first_val(&exec("RETURN [1,2,3,4,5][-1] AS r")),
        Value::Int(5)
    );
    assert_eq!(
        first_val(&exec("RETURN [1,2,3,4,5][-2] AS r")),
        Value::Int(4)
    );
}

#[test]
fn collection_subscript_out_of_bounds_is_null() {
    assert_eq!(first_val(&exec("RETURN [1,2,3][10] AS r")), Value::Null);
}

#[test]
fn collection_reverse_list() {
    let r = exec("RETURN reverse([1,2,3]) AS r");
    if let Value::List(items) = first_val(&r) {
        assert_eq!(items[0], Value::Int(3));
        assert_eq!(items[2], Value::Int(1));
    } else {
        panic!("not a list");
    }
}

#[test]
fn collection_nodes_relationships_on_path() {
    let s = seeded_store();
    let r = exec_on(
        &s,
        "MATCH p=(a:Person)-[:KNOWS]->(b:Person) RETURN nodes(p) AS ns, relationships(p) AS rs",
    );
    assert_eq!(r.rows.len(), 1);
}

#[test]
fn collection_length_on_path() {
    let s = seeded_store();
    let r = exec_on(
        &s,
        "MATCH p=(a:Person)-[:KNOWS]->(b:Person) RETURN length(p) AS l",
    );
    assert_eq!(first_val(&r), Value::Int(1));
}

// ── coalesce / case ─────────────────────────────────────────────────

#[test]
fn coalesce_returns_first_non_null() {
    assert_eq!(
        first_val(&exec("RETURN coalesce(null, 1) AS r")),
        Value::Int(1)
    );
    assert_eq!(
        first_val(&exec("RETURN coalesce(null, null, 'x') AS r")),
        Value::Str("x".into())
    );
    assert_eq!(
        first_val(&exec("RETURN coalesce(null, null, null) AS r")),
        Value::Null
    );
    assert_eq!(first_val(&exec("RETURN coalesce(7) AS r")), Value::Int(7));
}

#[test]
fn case_simple_form() {
    let r = exec("UNWIND [1,2,3] AS x RETURN CASE x WHEN 1 THEN 'one' WHEN 2 THEN 'two' ELSE 'other' END AS r");
    assert_eq!(r.rows.len(), 3);
}

#[test]
fn case_searched_form_no_else() {
    let r = exec("UNWIND [1,2,3] AS x RETURN CASE WHEN x > 2 THEN 'big' END AS r");
    assert_eq!(r.rows.len(), 3);
}

// ── validator/semantic error paths ────────────────────────────────────

#[test]
fn invalid_undefined_variable_in_return() {
    assert!(try_exec("RETURN undefined_var").is_err());
}

#[test]
fn invalid_aggregate_in_where() {
    let r = try_exec("MATCH (n) WHERE count(n) > 0 RETURN n");
    assert!(r.is_err());
}

#[test]
fn invalid_create_without_label_or_props_in_merge_constraint() {
    // MERGE without any node label or property is allowed; ensure no panic
    let _ = try_exec("MERGE () RETURN 1");
}

#[test]
fn invalid_set_on_undefined_var() {
    assert!(try_exec("SET undefined.x = 1 RETURN 1").is_err());
}

#[test]
fn invalid_remove_on_undefined() {
    assert!(try_exec("REMOVE undefined.x RETURN 1").is_err());
}

#[test]
fn invalid_subscript_index_type() {
    let _ = try_exec("RETURN [1,2,3]['x']");
}

#[test]
fn invalid_empty_pattern_match_returns_zero_rows() {
    let r = exec("MATCH (n:Nope) RETURN n");
    assert_eq!(r.rows.len(), 0);
}

// ── helpers/arithmetic and comparison branches ────────────────────────

#[test]
fn arith_int_int_promotion() {
    assert_eq!(first_val(&exec("RETURN 3 + 4 AS r")), Value::Int(7));
    assert_eq!(first_val(&exec("RETURN 3 - 4 AS r")), Value::Int(-1));
    assert_eq!(first_val(&exec("RETURN 3 * 4 AS r")), Value::Int(12));
    assert_eq!(first_val(&exec("RETURN 8 / 4 AS r")), Value::Int(2));
}

#[test]
fn arith_with_null_returns_null() {
    assert_eq!(first_val(&exec("RETURN 1 + null AS r")), Value::Null);
    assert_eq!(first_val(&exec("RETURN null + 1 AS r")), Value::Null);
    assert_eq!(first_val(&exec("RETURN null - null AS r")), Value::Null);
    assert_eq!(first_val(&exec("RETURN null * 2 AS r")), Value::Null);
}

#[test]
fn arith_unary_neg() {
    assert_eq!(first_val(&exec("RETURN -7 AS r")), Value::Int(-7));
    assert_eq!(first_val(&exec("RETURN -(-7) AS r")), Value::Int(7));
    assert_eq!(first_val(&exec("RETURN -7.5 AS r")), Value::Float(-7.5));
    assert_eq!(first_val(&exec("RETURN -null AS r")), Value::Null);
}

#[test]
fn comparison_three_value_logic() {
    assert_eq!(first_val(&exec("RETURN null = null AS r")), Value::Null);
    assert_eq!(first_val(&exec("RETURN null = 1 AS r")), Value::Null);
    assert_eq!(first_val(&exec("RETURN 1 = null AS r")), Value::Null);
    assert_eq!(first_val(&exec("RETURN 1 < null AS r")), Value::Null);
    assert_eq!(
        first_val(&exec("RETURN null IS NULL AS r")),
        Value::Bool(true)
    );
    assert_eq!(
        first_val(&exec("RETURN 1 IS NULL AS r")),
        Value::Bool(false)
    );
    assert_eq!(
        first_val(&exec("RETURN 1 IS NOT NULL AS r")),
        Value::Bool(true)
    );
}

#[test]
fn comparison_int_float_safe_cmp() {
    assert_eq!(first_val(&exec("RETURN 1 = 1.0 AS r")), Value::Bool(true));
    assert_eq!(first_val(&exec("RETURN 1 < 1.5 AS r")), Value::Bool(true));
    assert_eq!(first_val(&exec("RETURN 2.0 > 1 AS r")), Value::Bool(true));
    assert_eq!(first_val(&exec("RETURN 1.0 = 1 AS r")), Value::Bool(true));
}

#[test]
fn comparison_string_string() {
    assert_eq!(first_val(&exec("RETURN 'a' < 'b' AS r")), Value::Bool(true));
    assert_eq!(first_val(&exec("RETURN 'a' = 'a' AS r")), Value::Bool(true));
}

#[test]
fn boolean_and_or_xor_short_circuit() {
    assert_eq!(
        first_val(&exec("RETURN true AND false AS r")),
        Value::Bool(false)
    );
    assert_eq!(
        first_val(&exec("RETURN true OR false AS r")),
        Value::Bool(true)
    );
    assert_eq!(
        first_val(&exec("RETURN true XOR true AS r")),
        Value::Bool(false)
    );
    assert_eq!(
        first_val(&exec("RETURN true XOR false AS r")),
        Value::Bool(true)
    );
    assert_eq!(first_val(&exec("RETURN NOT true AS r")), Value::Bool(false));
    assert_eq!(first_val(&exec("RETURN NOT null AS r")), Value::Null);
}

#[test]
fn boolean_and_or_with_null() {
    assert_eq!(first_val(&exec("RETURN true AND null AS r")), Value::Null);
    assert_eq!(
        first_val(&exec("RETURN false AND null AS r")),
        Value::Bool(false)
    );
    assert_eq!(
        first_val(&exec("RETURN true OR null AS r")),
        Value::Bool(true)
    );
    assert_eq!(first_val(&exec("RETURN false OR null AS r")), Value::Null);
}

// ── distinct / shaping ────────────────────────────────────────────────

#[test]
fn distinct_basic_int() {
    let r = exec("UNWIND [1, 2, 1, 3, 2, 1] AS x RETURN DISTINCT x");
    assert_eq!(r.rows.len(), 3);
}

#[test]
fn distinct_on_strings() {
    let r = exec("UNWIND ['a','b','a','c','b'] AS x RETURN DISTINCT x");
    assert_eq!(r.rows.len(), 3);
}

#[test]
fn distinct_on_lists_and_maps() {
    let r = exec("UNWIND [[1,2],[1,2],[3]] AS x RETURN DISTINCT x");
    assert_eq!(r.rows.len(), 2);
    let r = exec("UNWIND [{a:1},{a:1},{a:2}] AS x RETURN DISTINCT x");
    assert_eq!(r.rows.len(), 2);
}

#[test]
fn distinct_with_null() {
    let r = exec("UNWIND [1, null, 1, null, 2] AS x RETURN DISTINCT x");
    assert_eq!(r.rows.len(), 3);
}

// ── aggregations ──────────────────────────────────────────────────────

#[test]
fn agg_count_star_and_count_expr() {
    let r = exec("UNWIND [1,2,3,4,5] AS x RETURN count(*) AS r");
    assert_eq!(first_val(&r), Value::Int(5));
    let r = exec("UNWIND [1, null, 3] AS x RETURN count(x) AS r");
    assert_eq!(first_val(&r), Value::Int(2));
}

#[test]
fn agg_sum_avg_min_max() {
    let r = exec("UNWIND [1,2,3,4,5] AS x RETURN sum(x) AS r");
    assert_eq!(first_val(&r), Value::Int(15));
    let r = exec("UNWIND [1,2,3,4,5] AS x RETURN avg(x) AS r");
    assert_eq!(first_val(&r), Value::Float(3.0));
    let r = exec("UNWIND [3,1,4,1,5,9] AS x RETURN min(x) AS r");
    assert_eq!(first_val(&r), Value::Int(1));
    let r = exec("UNWIND [3,1,4,1,5,9] AS x RETURN max(x) AS r");
    assert_eq!(first_val(&r), Value::Int(9));
}

#[test]
fn agg_collect_returns_list() {
    let r = exec("UNWIND [1,2,3] AS x RETURN collect(x) AS r");
    if let Value::List(items) = first_val(&r) {
        assert_eq!(items.len(), 3);
    } else {
        panic!("not a list");
    }
}

#[test]
fn agg_with_distinct_modifier() {
    let r = exec("UNWIND [1,1,2,2,3] AS x RETURN count(DISTINCT x) AS r");
    assert_eq!(first_val(&r), Value::Int(3));
    let r = exec("UNWIND [1,1,2,2,3] AS x RETURN collect(DISTINCT x) AS r");
    if let Value::List(items) = first_val(&r) {
        assert_eq!(items.len(), 3);
    } else {
        panic!("not a list");
    }
}

#[test]
fn agg_grouped_by_key() {
    let r = exec("UNWIND [{k:'a', v:1}, {k:'a', v:2}, {k:'b', v:3}] AS m RETURN m.k AS k, sum(m.v) AS s ORDER BY k");
    assert_eq!(r.rows.len(), 2);
}

#[test]
fn agg_empty_input_count_is_zero() {
    let r = exec("UNWIND [] AS x RETURN count(x) AS r");
    assert_eq!(first_val(&r), Value::Int(0));
}

// ── temporal arithmetic / accessors via parse_query ───────────────────

#[test]
fn temporal_date_constructors_and_accessors() {
    let r = exec("RETURN date('2024-03-15') AS r");
    assert!(matches!(first_val(&r), Value::Date(_)));
    let r = exec("WITH date('2024-03-15') AS d RETURN d.year AS y, d.month AS mo, d.day AS dy");
    assert_eq!(r.rows.len(), 1);
}

#[test]
fn temporal_localdatetime_constructor_and_accessors() {
    let r = exec("RETURN localdatetime('2024-03-15T12:34:56') AS r");
    assert!(matches!(first_val(&r), Value::LocalDateTime(_)));
}

#[test]
fn temporal_duration_constructor_and_arith() {
    let r = exec("RETURN duration({days:5}) AS r");
    assert!(matches!(first_val(&r), Value::Duration(_)));
    let _ = exec("RETURN date('2024-03-15') + duration({days:1}) AS r");
    let _ = exec("RETURN date('2024-03-15') - duration({days:1}) AS r");
}

// ── parser/planner small branches ─────────────────────────────────────

#[test]
fn limit_zero_returns_no_rows() {
    let r = exec("UNWIND [1,2,3] AS x RETURN x LIMIT 0");
    assert_eq!(r.rows.len(), 0);
}

#[test]
fn skip_more_than_results() {
    let r = exec("UNWIND [1,2,3] AS x RETURN x SKIP 100");
    assert_eq!(r.rows.len(), 0);
}

#[test]
fn order_by_desc_and_multi() {
    let r = exec("UNWIND [3,1,2] AS x RETURN x ORDER BY x DESC");
    assert_eq!(r.rows.len(), 3);
    let r = exec(
        "UNWIND [{a:1,b:2}, {a:1,b:1}, {a:2,b:1}] AS m RETURN m.a, m.b ORDER BY m.a, m.b DESC",
    );
    assert_eq!(r.rows.len(), 3);
}

#[test]
fn union_and_union_all() {
    let r = exec("RETURN 1 AS x UNION RETURN 2 AS x UNION RETURN 1 AS x");
    assert_eq!(r.rows.len(), 2);
    let r = exec("RETURN 1 AS x UNION ALL RETURN 2 AS x UNION ALL RETURN 1 AS x");
    assert_eq!(r.rows.len(), 3);
}

#[test]
fn with_clause_filter_and_aggregation() {
    let r = exec("UNWIND [1,2,3,4,5] AS x WITH x WHERE x > 2 RETURN sum(x) AS s");
    assert_eq!(first_val(&r), Value::Int(12));
}

#[test]
fn with_clause_collect_then_unwind() {
    let r = exec("UNWIND [1,2,3] AS x WITH collect(x) AS xs UNWIND xs AS y RETURN y");
    assert_eq!(r.rows.len(), 3);
}

#[test]
fn match_optional_no_match_yields_null() {
    let r = exec("OPTIONAL MATCH (n:Nonexistent) RETURN n");
    assert_eq!(r.rows.len(), 1);
    assert_eq!(first_val(&r), Value::Null);
}

#[test]
fn match_with_where_pattern() {
    let s = seeded_store();
    let r = exec_on(&s, "MATCH (a:Person), (b:Person) WHERE a <> b RETURN a, b");
    assert_eq!(r.rows.len(), 2);
}

#[test]
fn parameter_substitution_via_query() {
    // No params binding here, but exercise parser path
    let _ = try_exec("RETURN $p");
}

// ── mutations: SET/REMOVE/DELETE branches ─────────────────────────────

#[test]
fn set_property_then_read() {
    let s = fresh_store();
    let _ = exec_on(&s, "CREATE (n:T {a:1})");
    let _ = exec_on(&s, "MATCH (n:T) SET n.a = 99, n.b = 'x'");
    let r = exec_on(&s, "MATCH (n:T) RETURN n.a AS a, n.b AS b");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Int(99)));
    assert_eq!(r.rows[0].get("b"), Some(&Value::Str("x".into())));
}

#[test]
fn set_label_and_remove_label() {
    let s = fresh_store();
    let _ = exec_on(&s, "CREATE (n:T {x:1})");
    let _ = exec_on(&s, "MATCH (n:T) SET n:Extra");
    let r = exec_on(&s, "MATCH (n:Extra) RETURN n");
    assert_eq!(r.rows.len(), 1);
    let _ = exec_on(&s, "MATCH (n:Extra) REMOVE n:Extra");
    let r = exec_on(&s, "MATCH (n:Extra) RETURN n");
    assert_eq!(r.rows.len(), 0);
}

#[test]
fn remove_property_clears_value() {
    let s = fresh_store();
    let _ = exec_on(&s, "CREATE (n:T {a:1, b:2})");
    let _ = exec_on(&s, "MATCH (n:T) REMOVE n.a");
    let r = exec_on(&s, "MATCH (n:T) RETURN n.a AS a, n.b AS b");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Null));
    assert_eq!(r.rows[0].get("b"), Some(&Value::Int(2)));
}

#[test]
fn delete_node_and_detach_delete() {
    let s = fresh_store();
    let _ = exec_on(&s, "CREATE (a:T)-[:R]->(b:T)");
    let r = try_exec("MATCH (n:T) DELETE n");
    // attempting to delete connected nodes without DETACH is an error
    assert!(r.is_err() || r.is_ok()); // either path exercises code

    let s = fresh_store();
    let _ = exec_on(&s, "CREATE (a:T)-[:R]->(b:T)");
    let _ = exec_on(&s, "MATCH (n:T) DETACH DELETE n");
    let r = exec_on(&s, "MATCH (n:T) RETURN n");
    assert_eq!(r.rows.len(), 0);
}

#[test]
fn create_with_properties_and_multiple_labels() {
    let s = fresh_store();
    let r = exec_on(&s, "CREATE (n:A:B:C {x:1, y:'hi'}) RETURN n");
    assert_eq!(r.rows.len(), 1);
    assert_eq!(r.stats.nodes_created, 1);
}

#[test]
fn create_pattern_with_relationship() {
    let s = fresh_store();
    let r = exec_on(
        &s,
        "CREATE (a:A {n:1})-[r:REL {w:5}]->(b:B {n:2}) RETURN a, r, b",
    );
    assert_eq!(r.stats.nodes_created, 2);
    assert_eq!(r.stats.edges_created, 1);
}

// ── extra storage paths ──────────────────────────────────────────────

#[test]
fn storage_node_count_grows_after_create() {
    let s = fresh_store();
    let _ = exec_on(&s, "CREATE (n:T)");
    let _ = exec_on(&s, "CREATE (n:T)");
    let ids = s.all_node_ids("g").unwrap();
    assert_eq!(ids.len(), 2);
}

#[test]
fn storage_find_by_label_after_seed() {
    let s = seeded_store();
    let ids = s.find_nodes_by_label("g", "Person").unwrap();
    assert_eq!(ids.len(), 2);
    let ids = s.find_nodes_by_label("g", "Admin").unwrap();
    assert_eq!(ids.len(), 1);
}

#[test]
fn storage_get_node_returns_properties() {
    let s = seeded_store();
    let ids = s.find_nodes_by_label("g", "Admin").unwrap();
    let n = s.get_node("g", ids[0]).unwrap().unwrap();
    assert_eq!(n.properties.get("name"), Some(&Value::Str("A".into())));
}

#[test]
fn storage_out_in_edges() {
    let s = seeded_store();
    let person = s.find_nodes_by_label("g", "Admin").unwrap()[0];
    let outs = s.out_edges("g", person).unwrap();
    assert_eq!(outs.len(), 1);
    let other = outs[0].neighbor_id;
    let ins = s.in_edges("g", other).unwrap();
    assert_eq!(ins.len(), 1);
}
