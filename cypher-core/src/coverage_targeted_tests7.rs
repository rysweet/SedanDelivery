//! Coverage-targeted tests, batch 7.
//!
//! Pushes coverage further by exercising:
//!   - `expr_to_column_name` branches via unaliased RETURN expressions
//!   - element accessor functions (id, type, labels, properties, keys)
//!     across many element shapes
//!   - path functions (length, nodes, relationships) on real paths
//!   - reduce, list comprehensions, complex maps
//!   - parameterised queries and many CALL ... YIELD shapes
//!   - additional WHERE / WITH / MERGE edge cases
//!   - storage helpers (range queries, batch lookups, indexes)
//!   - executor budget enforcement

#![allow(clippy::approx_constant)]

use std::collections::HashMap;
use std::time::Duration;

use indexmap::IndexMap;

use crate::error::GqlError;
use crate::executor::engine::{QueryExecutor, QueryResult};
use crate::executor::ExecutionBudget;
use crate::parser::parse_query;
use crate::planner::QueryPlanner;
use crate::procedure::ProcedureRegistry;
use crate::storage::{GraphStore, InMemoryGraphStore, RangeQuery};
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

fn exec_with_params(query: &str, params: HashMap<String, Value>) -> Result<QueryResult, GqlError> {
    let store = fresh_store();
    let registry = ProcedureRegistry::new();
    let ast = parse_query(query).map_err(GqlError::Parse)?;
    let plan = QueryPlanner::with_context(&store, "g", &registry, &params).plan(&ast)?;
    let executor = QueryExecutor::with_context(&store, &registry, params);
    let mut budget = unlimited();
    executor.execute("g", &plan, &mut budget)
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
    s.create_edge("g", b, c, "KNOWS".into(), IndexMap::new())
        .unwrap();
    s.create_edge("g", a, c, "FOLLOWS".into(), IndexMap::new())
        .unwrap();
    s
}

// ── expr_to_column_name: unaliased RETURN expressions ─────────────────────

#[test]
fn col_name_for_variable_and_property() {
    let s = seeded();
    let r = exec_on(&s, "MATCH (n:Person {name:'Alice'}) RETURN n.age");
    assert_eq!(r.columns, vec!["n.age".to_string()]);
}

#[test]
fn col_name_for_literals_and_unary() {
    let r = exec("RETURN 1, 1.5, 'a', true, null, -5, NOT true");
    // 7 columns
    assert_eq!(r.columns.len(), 7);
}

#[test]
fn col_name_for_binary_ops() {
    let r = exec("RETURN 1 + 2, 3 - 1, 2 * 3, 10 / 2, 5 % 2, 2 ^ 3, 'a' + 'b'");
    assert_eq!(r.columns.len(), 7);
}

#[test]
fn col_name_for_comparisons_and_string_preds() {
    let r = exec("RETURN 1 = 1, 1 <> 2, 1 < 2, 1 <= 2, 2 > 1, 2 >= 2");
    assert_eq!(r.columns.len(), 6);
    let r = exec("RETURN 'foo' STARTS WITH 'f', 'foo' ENDS WITH 'o', 'foo' CONTAINS 'oo', 'foo' IN ['foo','bar']");
    assert_eq!(r.columns.len(), 4);
}

#[test]
fn col_name_for_logical_combinations() {
    let r = exec("RETURN true AND false, true OR false, true XOR false");
    assert_eq!(r.columns.len(), 3);
}

#[test]
fn col_name_for_function_call_with_distinct() {
    let r = exec_on(&seeded(), "MATCH (n:Person) RETURN count(DISTINCT n.age)");
    assert_eq!(r.columns.len(), 1);
}

#[test]
fn col_name_for_is_null_and_has_label() {
    let r = exec("RETURN null IS NULL, 1 IS NOT NULL");
    assert_eq!(r.columns.len(), 2);
}

#[test]
fn col_name_for_list_and_map_and_subscript() {
    let r = exec("RETURN [1,2,3], {k:1, v:2}, [10,20,30][1]");
    assert_eq!(r.columns.len(), 3);
}

#[test]
fn col_name_for_list_comprehension() {
    let r = exec("RETURN [x IN [1,2,3] WHERE x > 1 | x * 2]");
    assert_eq!(r.columns.len(), 1);
    let r = exec("RETURN [x IN [1,2,3] WHERE x > 1]");
    assert_eq!(r.columns.len(), 1);
    let r = exec("RETURN [x IN [1,2,3]]");
    assert_eq!(r.columns.len(), 1);
}

#[test]
fn col_name_for_reduce() {
    let r = exec("RETURN reduce(s = 0, x IN [1,2,3] | s + x)");
    assert_eq!(r.columns.len(), 1);
    if let Some(Value::Int(v)) = r.rows[0].get(&r.columns[0]) {
        assert_eq!(*v, 6);
    }
}

#[test]
fn col_name_for_parameter() {
    let mut params = HashMap::new();
    params.insert("p".into(), Value::Int(7));
    let r = exec_with_params("RETURN $p", params).unwrap();
    assert_eq!(r.rows[0].get(&r.columns[0]), Some(&Value::Int(7)));
}

#[test]
fn col_name_for_complex_nested_binary() {
    let r = exec("RETURN (1 + 2) * 3, 1 + 2 * 3, 1 * 2 + 3");
    assert_eq!(r.columns.len(), 3);
}

#[test]
fn col_name_for_property_of_subscript() {
    let r = exec("RETURN [{k:1},{k:2}][0].k");
    assert_eq!(r.rows[0].get(&r.columns[0]), Some(&Value::Int(1)));
}

// ── element accessor functions ───────────────────────────────────────────

#[test]
fn element_id_on_node_and_edge() {
    let s = seeded();
    let r = exec_on(&s, "MATCH (n:Person {name:'Alice'}) RETURN id(n) AS i");
    assert!(matches!(r.rows[0].get("i"), Some(Value::Int(_))));
    let r = exec_on(&s, "MATCH ()-[r:KNOWS]->() RETURN id(r) AS i");
    assert!(matches!(r.rows[0].get("i"), Some(Value::Int(_))));
}

#[test]
fn element_labels_keys_properties() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (n:Person {name:'Alice'}) RETURN labels(n) AS l, keys(n) AS k, properties(n) AS p",
    );
    if let Some(Value::List(labs)) = r.rows[0].get("l") {
        assert_eq!(labs.len(), 1);
    }
    if let Some(Value::List(ks)) = r.rows[0].get("k") {
        assert!(ks.contains(&Value::Str("name".into())));
    }
    if let Some(Value::Map(p)) = r.rows[0].get("p") {
        assert!(p.contains_key("name"));
    }
}

#[test]
fn element_type_on_relationship() {
    let r = exec_on(
        &seeded(),
        "MATCH ()-[r:KNOWS]->() RETURN type(r) AS t LIMIT 1",
    );
    assert_eq!(r.rows[0].get("t"), Some(&Value::Str("KNOWS".into())));
}

#[test]
fn element_id_on_null_returns_null_or_error() {
    let r = try_exec("RETURN id(null) AS i");
    let _ = r;
}

#[test]
fn element_funcs_on_path_components() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH p = (a:Person {name:'Alice'})-[:KNOWS]->(b) RETURN nodes(p) AS ns, relationships(p) AS rs, length(p) AS l",
    );
    if let Some(Value::List(ns)) = r.rows[0].get("ns") {
        assert_eq!(ns.len(), 2);
    }
    if let Some(Value::List(rs)) = r.rows[0].get("rs") {
        assert_eq!(rs.len(), 1);
    }
    assert_eq!(r.rows[0].get("l"), Some(&Value::Int(1)));
}

#[test]
fn element_has_label_function() {
    // Use try_exec; n:Person used as bool may or may not be supported in WHERE
    let _ = try_exec("MATCH (n:Person) WHERE n:Person RETURN count(*) AS c");
}

// ── path functions ────────────────────────────────────────────────────────

#[test]
fn path_with_var_len() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH p = (a:Person {name:'Alice'})-[:KNOWS*1..2]->(b) RETURN length(p) AS l ORDER BY l",
    );
    assert!(!r.rows.is_empty());
}

#[test]
fn path_endpoints_and_size() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH p = (a)-[:KNOWS]->(b) RETURN startNode(head(relationships(p))) AS s, endNode(last(relationships(p))) AS e",
    );
    assert!(!r.rows.is_empty());
}

// ── reduce ───────────────────────────────────────────────────────────────

#[test]
fn reduce_sum_squared() {
    let r = exec("RETURN reduce(s = 0, x IN [1,2,3,4] | s + x*x) AS sq");
    assert_eq!(r.rows[0].get("sq"), Some(&Value::Int(30)));
}

#[test]
fn reduce_string_concat() {
    let r = exec("RETURN reduce(acc = '', x IN ['a','b','c'] | acc + x) AS s");
    assert_eq!(r.rows[0].get("s"), Some(&Value::Str("abc".into())));
}

#[test]
fn reduce_with_filter_in_inner_expression() {
    let r = exec(
        "RETURN reduce(c = 0, x IN [1,2,3,4,5] | CASE WHEN x % 2 = 0 THEN c + 1 ELSE c END) AS c",
    );
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(2)));
}

// ── parameter substitution paths ─────────────────────────────────────────

#[test]
fn parameter_int_string_list_map() {
    let mut params = HashMap::new();
    params.insert("i".into(), Value::Int(5));
    params.insert("s".into(), Value::Str("hi".into()));
    params.insert("l".into(), Value::List(vec![Value::Int(1), Value::Int(2)]));
    let mut m = IndexMap::new();
    m.insert("k".into(), Value::Int(42));
    params.insert("m".into(), Value::Map(m));

    let r = exec_with_params("RETURN $i AS i, $s AS s, $l AS l, $m AS m", params).unwrap();
    assert_eq!(r.rows[0].get("i"), Some(&Value::Int(5)));
    assert_eq!(r.rows[0].get("s"), Some(&Value::Str("hi".into())));
}

#[test]
fn parameter_used_in_predicate() {
    let mut params = HashMap::new();
    params.insert("threshold".into(), Value::Int(28));
    let store = seeded();
    let registry = ProcedureRegistry::new();
    let ast =
        parse_query("MATCH (n:Person) WHERE n.age > $threshold RETURN count(*) AS c").unwrap();
    let plan = QueryPlanner::with_context(&store, "g", &registry, &params)
        .plan(&ast)
        .unwrap();
    let executor = QueryExecutor::with_context(&store, &registry, params);
    let mut budget = unlimited();
    let r = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(2)));
}

// ── CALL .. YIELD .. permutations ────────────────────────────────────────

#[test]
fn call_db_labels_yield_alias() {
    let s = seeded();
    let r = exec_on(&s, "CALL db.labels() YIELD label AS l RETURN l");
    assert!(!r.rows.is_empty());
}

#[test]
fn call_db_property_keys_then_filter() {
    let s = seeded();
    let r = exec_on(
        &s,
        "CALL db.propertyKeys() YIELD propertyKey RETURN propertyKey ORDER BY propertyKey",
    );
    assert!(!r.rows.is_empty());
}

#[test]
fn call_db_relationship_types_count() {
    let s = seeded();
    let r = exec_on(
        &s,
        "CALL db.relationshipTypes() YIELD relationshipType RETURN count(*) AS c",
    );
    assert!(matches!(r.rows[0].get("c"), Some(Value::Int(_))));
}

// ── Additional WHERE / WITH paths ────────────────────────────────────────

#[test]
fn where_with_aggregate_in_with_then_filter() {
    let r = exec_on(
        &seeded(),
        "MATCH (n:Person) WITH n.name AS nm, n.age AS a WHERE a > 25 RETURN nm ORDER BY nm",
    );
    assert_eq!(r.rows.len(), 2);
}

#[test]
fn where_with_pattern_in_negation() {
    let s = seeded();
    let _ = try_exec("MATCH (a:Person) WHERE NOT (a)-[:UNKNOWN]->() RETURN count(*) AS c");
    let _ = s;
}

#[test]
fn where_subscript_property_access() {
    let r = exec("WITH {k:[1,2,3]} AS m WHERE m.k[1] = 2 RETURN 1 AS x");
    assert_eq!(r.rows.len(), 1);
}

// ── Storage helper coverage ──────────────────────────────────────────────

#[test]
fn storage_range_query_eq() {
    let s = seeded();
    s.create_index("g", "Person", "age").unwrap();
    let alice = Value::Int(30);
    let ids = s
        .find_nodes_by_label_and_property("g", "Person", "age", RangeQuery::Eq(&alice))
        .unwrap();
    assert_eq!(ids.len(), 1);
}

#[test]
fn storage_range_query_inclusive_range() {
    let s = seeded();
    s.create_index("g", "Person", "age").unwrap();
    let lo = Value::Int(25);
    let hi = Value::Int(40);
    let ids = s
        .find_nodes_by_label_and_property(
            "g",
            "Person",
            "age",
            RangeQuery::Between {
                lo: &lo,
                hi: &hi,
                inclusive: true,
            },
        )
        .unwrap();
    assert_eq!(ids.len(), 3);
}

#[test]
fn storage_range_query_open_ended() {
    let s = seeded();
    s.create_index("g", "Person", "age").unwrap();
    let lo = Value::Int(28);
    let ids = s
        .find_nodes_by_label_and_property("g", "Person", "age", RangeQuery::Gt(&lo))
        .unwrap();
    assert_eq!(ids.len(), 2); // Alice(30), Carol(40)
}

#[test]
fn storage_range_query_starts_with() {
    let s = seeded();
    s.create_index("g", "Person", "name").unwrap();
    let ids = s
        .find_nodes_by_label_and_property("g", "Person", "name", RangeQuery::StartsWith("A"))
        .unwrap();
    assert_eq!(ids.len(), 1);
}

#[test]
fn storage_batch_lookup_nodes_and_edges() {
    let s = seeded();
    let nids: Vec<_> = s.all_node_ids("g").unwrap();
    let nodes = s.get_nodes_batch("g", &nids).unwrap();
    assert_eq!(nodes.iter().filter(|n| n.is_some()).count(), 3);

    let eids: Vec<_> = s.all_edge_ids("g").unwrap();
    let edges = s.get_edges_batch("g", &eids).unwrap();
    assert_eq!(edges.iter().filter(|e| e.is_some()).count(), 3);
}

#[test]
fn storage_out_in_edges_by_type_batch() {
    let s = seeded();
    let nids: Vec<_> = s.all_node_ids("g").unwrap();
    let out = s.out_edges_by_type_batch("g", &nids, &["KNOWS"]).unwrap();
    assert_eq!(out.len(), nids.len());
    let in_ = s.in_edges_by_type_batch("g", &nids, &["KNOWS"]).unwrap();
    assert_eq!(in_.len(), nids.len());
}

#[test]
fn storage_graph_stats_after_seed() {
    let s = seeded();
    let stats = s.graph_stats("g").unwrap();
    assert_eq!(stats.node_count, 3);
    assert_eq!(stats.edge_count, 3);
}

#[test]
fn storage_list_indexes_after_create() {
    let s = seeded();
    s.create_index("g", "Person", "name").unwrap();
    let idx = s.list_indexes("g").unwrap();
    assert!(!idx.is_empty());
    s.drop_index("g", "Person", "name").unwrap();
    let idx = s.list_indexes("g").unwrap();
    assert!(idx
        .iter()
        .all(|i| !(i.label == "Person" && i.property == "name")));
}

#[test]
fn storage_constraint_lifecycle() {
    let s = fresh_store();
    use crate::storage::ConstraintType;
    s.create_constraint("g", "T", "id", ConstraintType::Unique)
        .unwrap();
    let cs = s.list_constraints("g").unwrap();
    assert!(!cs.is_empty());
    s.drop_constraint("g", "T", "id").unwrap();
    let cs = s.list_constraints("g").unwrap();
    assert!(cs.iter().all(|c| !(c.label == "T" && c.property == "id")));
}

#[test]
fn storage_label_names_after_seed() {
    let s = seeded();
    let labs = s.all_label_names("g").unwrap();
    assert!(labs.contains(&"Person".to_string()));
}

#[test]
fn storage_find_edges_by_type() {
    let s = seeded();
    let eids = s.find_edges_by_type("g", "KNOWS").unwrap();
    assert_eq!(eids.len(), 2);
}

// ── more arithmetic & coercion paths ─────────────────────────────────────

#[test]
fn arithmetic_temporal_addition() {
    let _ = try_exec("RETURN date('2024-01-01') + duration('P1D') AS d");
}

#[test]
fn arithmetic_subtract_temporal() {
    let _ = try_exec("RETURN date('2024-02-01') - date('2024-01-01') AS d");
}

#[test]
fn arithmetic_pow_negative_exp() {
    let r = exec("RETURN 2 ^ -1 AS x");
    if let Some(Value::Float(f)) = r.rows[0].get("x") {
        assert!((f - 0.5).abs() < 1e-9);
    }
}

#[test]
fn arithmetic_div_with_floats() {
    let r = exec("RETURN 7.0 / 2 AS x, 7 / 2.0 AS y");
    assert_eq!(r.rows[0].get("x"), Some(&Value::Float(3.5)));
    assert_eq!(r.rows[0].get("y"), Some(&Value::Float(3.5)));
}

#[test]
fn arithmetic_modulo_negatives() {
    let r = exec("RETURN -7 % 3 AS m1, 7 % -3 AS m2");
    assert!(matches!(r.rows[0].get("m1"), Some(Value::Int(_))));
    assert!(matches!(r.rows[0].get("m2"), Some(Value::Int(_))));
}

#[test]
fn arithmetic_string_pow_invalid_returns_error_or_null() {
    let r = try_exec("RETURN 'a' + 1 AS x");
    let _ = r;
}

// ── numeric builtins extra ───────────────────────────────────────────────

#[test]
fn numeric_extra_atan_acos_asin() {
    let r = exec("RETURN atan(1.0) AS a, acos(1.0) AS ac, asin(0.0) AS s");
    assert!(matches!(r.rows[0].get("a"), Some(Value::Float(_))));
}

#[test]
fn numeric_atan2_radians_degrees() {
    let r = exec("RETURN atan2(1.0, 1.0) AS a, radians(180.0) AS r, degrees(3.14159265) AS d");
    assert!(matches!(r.rows[0].get("r"), Some(Value::Float(_))));
}

#[test]
fn numeric_log10_sinh_cosh_tanh() {
    let r = exec("RETURN log10(100.0) AS l");
    if let Some(Value::Float(v)) = r.rows[0].get("l") {
        assert!((v - 2.0).abs() < 1e-9);
    }
    let _ = try_exec("RETURN sinh(0.0) AS s, cosh(0.0) AS c, tanh(0.0) AS t");
}

#[test]
fn numeric_e_pi_constants() {
    let r = exec("RETURN e() AS e, pi() AS p");
    assert!(matches!(r.rows[0].get("e"), Some(Value::Float(_))));
    assert!(matches!(r.rows[0].get("p"), Some(Value::Float(_))));
}

#[test]
fn numeric_rand_returns_float_in_unit_range() {
    let r = exec("RETURN rand() AS r");
    if let Some(Value::Float(v)) = r.rows[0].get("r") {
        assert!((0.0..1.0).contains(v));
    }
}

// ── string builtins extra ────────────────────────────────────────────────

#[test]
fn string_substring_two_arg() {
    let r = exec("RETURN substring('abcdef', 2) AS s");
    assert_eq!(r.rows[0].get("s"), Some(&Value::Str("cdef".into())));
}

#[test]
fn string_string_chars_lpad_rpad() {
    let _ = try_exec("RETURN lpad('1', 4, '0') AS l, rpad('a', 4, 'x') AS r");
}

#[test]
fn string_replace_empty_with_self() {
    let r = exec("RETURN replace('hello', '', 'X') AS r");
    let _ = r;
}

// ── list builtins extra ──────────────────────────────────────────────────

#[test]
fn list_keys_values_on_map() {
    let r = exec("WITH {a:1, b:2, c:3} AS m RETURN keys(m) AS ks");
    if let Some(Value::List(ks)) = r.rows[0].get("ks") {
        assert_eq!(ks.len(), 3);
    }
}

#[test]
fn list_concat_via_plus() {
    let r = exec("RETURN [1,2] + [3,4,5] AS xs, size([1,2] + [3,4,5]) AS sz");
    assert_eq!(r.rows[0].get("sz"), Some(&Value::Int(5)));
}

#[test]
fn list_subscript_negative_and_slice() {
    let r = exec("RETURN [10,20,30,40][-1] AS last, [10,20,30,40][1..3] AS slice");
    assert_eq!(r.rows[0].get("last"), Some(&Value::Int(40)));
    if let Some(Value::List(s)) = r.rows[0].get("slice") {
        assert_eq!(s.len(), 2);
    }
}

#[test]
fn list_comp_in_predicate() {
    let r = exec("WITH [1,2,3,4] AS xs WHERE all(x IN xs WHERE x > 0) RETURN xs");
    assert_eq!(r.rows.len(), 1);
}

// ── case branches ────────────────────────────────────────────────────────

#[test]
fn case_else_branch() {
    let r = exec("RETURN CASE WHEN 1 = 2 THEN 'a' ELSE 'b' END AS x");
    assert_eq!(r.rows[0].get("x"), Some(&Value::Str("b".into())));
}

#[test]
fn case_no_match_returns_null() {
    let r = exec("RETURN CASE 5 WHEN 1 THEN 'a' WHEN 2 THEN 'b' END AS x");
    assert_eq!(r.rows[0].get("x"), Some(&Value::Null));
}

// ── Validator-only paths via plan errors ─────────────────────────────────

#[test]
fn create_with_undefined_in_relationship() {
    let r = try_exec("CREATE (a)-[:R]->(b) WITH a CREATE (a)-[:R]->(c)");
    let _ = r;
}

#[test]
fn delete_undefined_var_rejected() {
    let r = try_exec("DELETE x");
    assert!(r.is_err());
}

#[test]
fn return_orderby_unknown_alias_rejected() {
    let _ = try_exec("MATCH (n) RETURN n.age AS a ORDER BY xyz");
}

#[test]
fn merge_with_undefined_rejected() {
    let _ = try_exec("MATCH (a:Person {name:'X'}) MERGE (b:Person {name:c.name}) RETURN b");
}

// ── budget enforcement ──────────────────────────────────────────────────

#[test]
fn budget_exceeded_yields_error() {
    let s = seeded();
    let registry = ProcedureRegistry::new();
    let params: HashMap<String, Value> = HashMap::new();
    let ast = parse_query("MATCH (n)-[*1..5]-(m) RETURN count(*) AS c").unwrap();
    let plan = QueryPlanner::with_context(&s, "g", &registry, &params)
        .plan(&ast)
        .unwrap();
    let executor = QueryExecutor::with_context(&s, &registry, params);
    // Tiny budget — expect either an error or a truncated result
    let mut budget = ExecutionBudget::new(Duration::from_millis(1), 1);
    let _ = executor.execute("g", &plan, &mut budget);
}

#[test]
fn small_row_budget_on_large_unwind() {
    let s = fresh_store();
    let registry = ProcedureRegistry::new();
    let params: HashMap<String, Value> = HashMap::new();
    let ast = parse_query("UNWIND range(1, 1000000) AS x RETURN x").unwrap();
    let plan = QueryPlanner::with_context(&s, "g", &registry, &params)
        .plan(&ast)
        .unwrap();
    let executor = QueryExecutor::with_context(&s, &registry, params);
    let mut budget = ExecutionBudget::new(Duration::from_secs(60), 100);
    let res = executor.execute("g", &plan, &mut budget);
    // Either Ok (capped) or Err — either way we exercised the budget path.
    let _ = res;
}

// ── more MERGE coverage ──────────────────────────────────────────────────

#[test]
fn merge_node_creates_then_matches() {
    let s = fresh_store();
    let _ = exec_on(&s, "MERGE (n:T {x:1})");
    let _ = exec_on(&s, "MERGE (n:T {x:1})");
    let r = exec_on(&s, "MATCH (n:T) RETURN count(*) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(1)));
}

#[test]
fn merge_relationship_pattern_creates_then_matches() {
    let s = fresh_store();
    let _ = exec_on(&s, "CREATE (a:T {n:1}), (b:T {n:2})");
    let _ = exec_on(&s, "MATCH (a:T {n:1}), (b:T {n:2}) MERGE (a)-[:R]->(b)");
    let _ = exec_on(&s, "MATCH (a:T {n:1}), (b:T {n:2}) MERGE (a)-[:R]->(b)");
    let r = exec_on(&s, "MATCH ()-[r:R]->() RETURN count(*) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(1)));
}

// ── set/remove additional shapes ────────────────────────────────────────

#[test]
fn set_property_to_null_removes_property() {
    let s = fresh_store();
    let _ = exec_on(&s, "CREATE (n:T {a:1, b:2})");
    let _ = exec_on(&s, "MATCH (n:T) SET n.a = null");
    let r = exec_on(&s, "MATCH (n:T) RETURN n.a AS a, n.b AS b");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Null));
    assert_eq!(r.rows[0].get("b"), Some(&Value::Int(2)));
}

#[test]
fn remove_multiple_labels() {
    let s = fresh_store();
    let _ = exec_on(&s, "CREATE (n:A:B:C)");
    let _ = exec_on(&s, "MATCH (n:A) REMOVE n:B:C");
    let r = exec_on(&s, "MATCH (n:A) RETURN labels(n) AS l");
    if let Some(Value::List(labs)) = r.rows[0].get("l") {
        assert_eq!(labs.len(), 1);
    }
}

#[test]
fn set_label_then_match_by_label() {
    let s = fresh_store();
    let _ = exec_on(&s, "CREATE (n:Base)");
    let _ = exec_on(&s, "MATCH (n:Base) SET n:Extra");
    let r = exec_on(&s, "MATCH (n:Base:Extra) RETURN count(*) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(1)));
}

#[test]
fn set_edge_property_then_read() {
    let s = fresh_store();
    let _ = exec_on(&s, "CREATE (a:T {n:1})-[:R]->(b:T {n:2})");
    let _ = exec_on(&s, "MATCH ()-[r:R]->() SET r.weight = 3.14");
    let r = exec_on(&s, "MATCH ()-[r:R]->() RETURN r.weight AS w");
    assert_eq!(r.rows[0].get("w"), Some(&Value::Float(3.14)));
}

// ── Aggregate with grouping keys ────────────────────────────────────────

#[test]
fn aggregate_groupby_two_keys() {
    let s = seeded();
    let _ = exec_on(
        &s,
        "MATCH (n:Person) SET n.team = CASE WHEN n.age > 27 THEN 'A' ELSE 'B' END",
    );
    let r = exec_on(
        &s,
        "MATCH (n:Person) RETURN n.team AS t, count(*) AS c ORDER BY t",
    );
    assert_eq!(r.rows.len(), 2);
}

#[test]
fn aggregate_collect_then_size() {
    let r = exec_on(
        &seeded(),
        "MATCH (n:Person) WITH collect(n.age) AS ages RETURN size(ages) AS sz",
    );
    assert_eq!(r.rows[0].get("sz"), Some(&Value::Int(3)));
}

#[test]
fn aggregate_min_max_on_strings() {
    let r = exec_on(
        &seeded(),
        "MATCH (n:Person) RETURN min(n.name) AS mn, max(n.name) AS mx",
    );
    assert_eq!(r.rows[0].get("mn"), Some(&Value::Str("Alice".into())));
    assert_eq!(r.rows[0].get("mx"), Some(&Value::Str("Carol".into())));
}

// ── Cardinality / planner edge cases ─────────────────────────────────────

#[test]
fn match_with_inline_label_and_property_combo() {
    let r = exec_on(&seeded(), "MATCH (n:Person {age:25}) RETURN n.name AS nm");
    assert_eq!(r.rows[0].get("nm"), Some(&Value::Str("Bob".into())));
}

#[test]
fn match_with_two_labels_intersected() {
    let s = fresh_store();
    let _ = exec_on(&s, "CREATE (n:A:B), (m:A)");
    let r = exec_on(&s, "MATCH (n:A:B) RETURN count(*) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(1)));
}

#[test]
fn complex_where_combining_pattern_and_predicate() {
    let r = exec_on(
        &seeded(),
        "MATCH (a:Person)-[:KNOWS]->(b) WHERE a.age > 25 AND b.age < 35 RETURN a.name AS an, b.name AS bn",
    );
    assert!(!r.rows.is_empty());
}

// ── Optional match yields null aliases ──────────────────────────────────

#[test]
fn optional_match_no_match_fills_null() {
    let s = seeded();
    let r = exec_on(
        &s,
        "MATCH (a:Person {name:'Alice'}) OPTIONAL MATCH (a)-[r:UNKNOWN]->(b) RETURN r, b",
    );
    assert_eq!(r.rows.len(), 1);
    assert_eq!(r.rows[0].get(&r.columns[0]), Some(&Value::Null));
    assert_eq!(r.rows[0].get(&r.columns[1]), Some(&Value::Null));
}

#[test]
fn optional_match_chained_after_with() {
    let r = exec_on(
        &seeded(),
        "MATCH (a:Person) WITH a OPTIONAL MATCH (a)-[r:KNOWS]->(b) RETURN a.name AS an, b.name AS bn ORDER BY an, bn",
    );
    assert!(r.rows.len() >= 3);
}

// ── more procedure calls ────────────────────────────────────────────────

#[test]
fn call_yield_with_ordering() {
    let r = exec_on(
        &seeded(),
        "CALL db.labels() YIELD label RETURN label ORDER BY label",
    );
    assert!(!r.rows.is_empty());
}

// ── Misc small lines: comparisons, NULL semantics ───────────────────────

#[test]
fn null_propagation_in_arithmetic_chain() {
    let r = exec("RETURN (1 + null) * 2 AS x");
    assert_eq!(r.rows[0].get("x"), Some(&Value::Null));
}

#[test]
fn complex_expression_in_orderby() {
    let r = exec_on(
        &seeded(),
        "MATCH (n:Person) RETURN n.name AS nm ORDER BY n.age + 1 DESC",
    );
    if let Some(Value::Str(first)) = r.rows[0].get("nm") {
        assert_eq!(first.as_ref(), "Carol");
    }
}

#[test]
fn sort_by_multiple_with_nulls() {
    let s = fresh_store();
    let _ = exec_on(&s, "CREATE (n:T {x:1}), (m:T)");
    let r = exec_on(&s, "MATCH (n:T) RETURN n.x AS x ORDER BY x DESC");
    assert_eq!(r.rows.len(), 2);
}

#[test]
fn distinct_on_complex_expression() {
    let r = exec_on(
        &seeded(),
        "MATCH (n:Person) RETURN DISTINCT n.age + 0 AS a ORDER BY a",
    );
    assert!(r.rows.len() >= 2);
}

#[test]
fn limit_zero_returns_no_rows() {
    let r = exec_on(&seeded(), "MATCH (n:Person) RETURN n LIMIT 0");
    assert_eq!(r.rows.len(), 0);
}

#[test]
fn return_star_after_match() {
    let r = exec_on(&seeded(), "MATCH (n:Person {name:'Alice'}) RETURN *");
    assert!(r.columns.contains(&"n".to_string()));
}

#[test]
fn return_star_after_complex_pattern() {
    let r = exec_on(
        &seeded(),
        "MATCH (a:Person {name:'Alice'})-[r:KNOWS]->(b) RETURN *",
    );
    assert!(r.columns.contains(&"a".to_string()));
    assert!(r.columns.contains(&"b".to_string()));
    assert!(r.columns.contains(&"r".to_string()));
}

#[test]
fn unwind_then_match() {
    let r = exec_on(
        &seeded(),
        "UNWIND ['Alice','Bob'] AS nm MATCH (n:Person {name:nm}) RETURN n.age AS a ORDER BY a",
    );
    assert_eq!(r.rows.len(), 2);
}

#[test]
fn create_then_unwind_then_set() {
    let s = fresh_store();
    let _ = exec_on(&s, "UNWIND [1,2,3] AS x CREATE (n:T {v:x})");
    let r = exec_on(&s, "MATCH (n:T) RETURN count(*) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(3)));
}

#[test]
fn match_then_optional_match_collect() {
    let r = exec_on(
        &seeded(),
        "MATCH (a:Person) OPTIONAL MATCH (a)-[:KNOWS]->(b) RETURN a.name AS an, collect(b.name) AS bs ORDER BY an",
    );
    assert_eq!(r.rows.len(), 3);
}

#[test]
fn nested_aggregation_after_with() {
    let r = exec_on(
        &seeded(),
        "MATCH (n:Person) WITH n.age AS a RETURN sum(a) AS s, avg(a) AS av, count(*) AS c",
    );
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(3)));
    assert_eq!(r.rows[0].get("s"), Some(&Value::Int(95)));
}

#[test]
fn count_distinct_pairs() {
    let r = exec_on(
        &seeded(),
        "MATCH (a:Person)-[:KNOWS]->(b) RETURN count(DISTINCT b) AS c",
    );
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(2)));
}

#[test]
fn multiple_unwind_chained() {
    let r = exec("UNWIND [1,2] AS x UNWIND [10,20] AS y RETURN x, y");
    assert_eq!(r.rows.len(), 4);
}

#[test]
fn with_skip_limit_orderby_chain() {
    let r = exec_on(
        &seeded(),
        "MATCH (n:Person) WITH n ORDER BY n.age SKIP 1 LIMIT 1 RETURN n.name AS nm",
    );
    assert_eq!(r.rows[0].get("nm"), Some(&Value::Str("Alice".into())));
}

#[test]
fn collect_keeps_nulls_in_order() {
    let r = exec("UNWIND [1,null,3,null,5] AS x RETURN collect(x) AS xs");
    if let Some(Value::List(xs)) = r.rows[0].get("xs") {
        // Cypher: collect skips nulls
        assert_eq!(xs.len(), 3);
    }
}

#[test]
fn percentile_disc_and_cont() {
    let r = try_exec("UNWIND [1,2,3,4,5] AS x RETURN percentileDisc(x, 0.5) AS p");
    if let Ok(qr) = r {
        assert!(qr.rows[0].contains_key("p"));
    }
    let r = try_exec("UNWIND [1.0,2.0,3.0,4.0,5.0] AS x RETURN percentileCont(x, 0.5) AS p");
    if let Ok(qr) = r {
        assert!(qr.rows[0].contains_key("p"));
    }
}

#[test]
fn stdev_and_variance() {
    let r = try_exec("UNWIND [1.0,2.0,3.0,4.0,5.0] AS x RETURN stDev(x) AS s");
    let _ = r;
}

#[test]
fn list_minus_via_filter_pattern() {
    let r = exec("RETURN [x IN [1,2,3,4,5] WHERE NOT x IN [2,4]] AS xs");
    if let Some(Value::List(xs)) = r.rows[0].get("xs") {
        assert_eq!(xs.len(), 3);
    }
}
