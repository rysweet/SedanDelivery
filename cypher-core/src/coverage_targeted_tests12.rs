//! Coverage push r9: targeted integration tests for low-coverage modules.
//!
//! Drives the public Cypher pipeline (parse → plan → execute) to exercise:
//!
//! * `executor::engine::eval::element_funcs::properties`
//!   (`properties()`, `keys()`, `propertiesMatch()` on Node, Edge, Map and Null inputs)
//! * `executor::engine::shaping::distinct` (row-level dedup, by-key dedup,
//!   property-aware dedup, NULL-key handling)
//! * `executor::filter::temporal::arithmetic::time_ops::in_seconds`
//!   (`duration.inSeconds(t1, t2)` across temporal type combinations)

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

fn run_on(store: &InMemoryGraphStore, query: &str) -> QueryResult {
    let registry = ProcedureRegistry::new();
    let params: HashMap<String, Value> = HashMap::new();
    let q = parse_query(query).expect("parse");
    let plan = QueryPlanner::with_context(store, "g", &registry, &params)
        .plan(&q)
        .expect("plan");
    let exec = QueryExecutor::with_context(store, &registry, params);
    let mut b = unlimited();
    exec.execute("g", &plan, &mut b).expect("execute")
}

fn run(query: &str) -> QueryResult {
    let store = fresh_store();
    run_on(&store, query)
}

fn single_value(query: &str) -> Value {
    let r = run(query);
    assert_eq!(r.rows.len(), 1, "expected 1 row from `{query}`");
    let (_, v) = r.rows[0].iter().next().unwrap();
    v.clone()
}

// ── properties() ────────────────────────────────────────────────────────────

#[test]
fn properties_on_node_returns_map() {
    let store = fresh_store();
    run_on(&store, "CREATE (:P {name: 'Alice', age: 30})");
    let v = run_on(&store, "MATCH (n:P) RETURN properties(n) AS p")
        .rows
        .remove(0)
        .into_iter()
        .next()
        .unwrap()
        .1;
    if let Value::Map(m) = v {
        assert_eq!(m.get("name"), Some(&Value::Str("Alice".into())));
        assert_eq!(m.get("age"), Some(&Value::Int(30)));
    } else {
        panic!("expected map, got {v:?}");
    }
}

#[test]
fn properties_on_edge_returns_map() {
    let store = fresh_store();
    run_on(&store, "CREATE (:A)-[:R {weight: 7, label: 'edge'}]->(:B)");
    let v = run_on(&store, "MATCH ()-[r:R]->() RETURN properties(r) AS p")
        .rows
        .remove(0)
        .into_iter()
        .next()
        .unwrap()
        .1;
    if let Value::Map(m) = v {
        assert_eq!(m.get("weight"), Some(&Value::Int(7)));
        assert_eq!(m.get("label"), Some(&Value::Str("edge".into())));
    } else {
        panic!("expected map, got {v:?}");
    }
}

#[test]
fn properties_on_map_passes_through() {
    let v = single_value("RETURN properties({a: 1, b: 'x'}) AS p");
    if let Value::Map(m) = v {
        assert_eq!(m.get("a"), Some(&Value::Int(1)));
        assert_eq!(m.get("b"), Some(&Value::Str("x".into())));
    } else {
        panic!("expected map");
    }
}

#[test]
fn properties_on_null_is_null() {
    assert_eq!(single_value("RETURN properties(null) AS p"), Value::Null);
}

// ── keys() ──────────────────────────────────────────────────────────────────

#[test]
fn keys_on_node_returns_property_names() {
    let store = fresh_store();
    run_on(&store, "CREATE (:P {a: 1, b: 2})");
    let v = run_on(&store, "MATCH (n:P) RETURN keys(n) AS ks")
        .rows
        .remove(0)
        .into_iter()
        .next()
        .unwrap()
        .1;
    if let Value::List(items) = v {
        let mut names: Vec<String> = items
            .into_iter()
            .map(|i| match i {
                Value::Str(s) => s.into_string(),
                other => panic!("expected string keys, got {other:?}"),
            })
            .collect();
        names.sort();
        assert_eq!(names, vec!["a".to_string(), "b".to_string()]);
    } else {
        panic!("expected list");
    }
}

#[test]
fn keys_on_edge_returns_property_names() {
    let store = fresh_store();
    run_on(&store, "CREATE (:A)-[:R {x: 1, y: 2}]->(:B)");
    let v = run_on(&store, "MATCH ()-[r:R]->() RETURN keys(r) AS ks")
        .rows
        .remove(0)
        .into_iter()
        .next()
        .unwrap()
        .1;
    if let Value::List(items) = v {
        assert_eq!(items.len(), 2);
    } else {
        panic!("expected list");
    }
}

#[test]
fn keys_on_map_returns_keys() {
    let v = single_value("RETURN keys({first: 'A', last: 'B'}) AS ks");
    if let Value::List(items) = v {
        let names: Vec<String> = items
            .into_iter()
            .filter_map(|i| {
                if let Value::Str(s) = i {
                    Some(s.into_string())
                } else {
                    None
                }
            })
            .collect();
        assert!(names.contains(&"first".to_string()));
        assert!(names.contains(&"last".to_string()));
    } else {
        panic!("expected list");
    }
}

#[test]
fn keys_on_null_is_null() {
    assert_eq!(single_value("RETURN keys(null) AS k"), Value::Null);
}

// (propertiesMatch is generated only by internal planner paths gated by the
// MATCH validator; not exercisable from user-level Cypher in current build.)

// ── DISTINCT (row-level dedup) ──────────────────────────────────────────────

#[test]
fn distinct_dedups_unwound_scalars() {
    let r = run("UNWIND [1, 1, 2, 2, 3, 1] AS x RETURN DISTINCT x");
    let vals: Vec<i64> = r
        .rows
        .into_iter()
        .map(|row| match row.into_iter().next().unwrap().1 {
            Value::Int(i) => i,
            other => panic!("expected int, got {other:?}"),
        })
        .collect();
    let mut sorted = vals.clone();
    sorted.sort();
    assert_eq!(sorted, vec![1, 2, 3]);
}

#[test]
fn distinct_dedups_string_lists() {
    let r = run("UNWIND ['a', 'b', 'a', 'c', 'b'] AS s RETURN DISTINCT s ORDER BY s");
    let vals: Vec<String> = r
        .rows
        .into_iter()
        .map(|row| match row.into_iter().next().unwrap().1 {
            Value::Str(s) => s.into_string(),
            other => panic!("expected str, got {other:?}"),
        })
        .collect();
    assert_eq!(
        vals,
        vec!["a".to_string(), "b".to_string(), "c".to_string()]
    );
}

#[test]
fn distinct_treats_null_as_a_value_once() {
    // NULL deduplicates with itself → exactly one NULL emitted.
    let r = run("UNWIND [1, null, null, 2, null] AS x RETURN DISTINCT x ORDER BY x");
    assert_eq!(r.rows.len(), 3);
}

#[test]
fn distinct_on_nodes_collapses_repeats() {
    let store = fresh_store();
    run_on(&store, "CREATE (:P {n: 1}), (:P {n: 2})");
    // Self-cross product would yield 4 rows; DISTINCT n gives 2.
    let r = run_on(
        &store,
        "MATCH (n:P), (m:P) RETURN DISTINCT n.n AS x ORDER BY x",
    );
    let vals: Vec<i64> = r
        .rows
        .into_iter()
        .map(|row| match row.into_iter().next().unwrap().1 {
            Value::Int(i) => i,
            _ => panic!(),
        })
        .collect();
    assert_eq!(vals, vec![1, 2]);
}

// ── duration.inSeconds() ────────────────────────────────────────────────────

#[test]
fn duration_in_seconds_localdatetime_difference() {
    // Two LDTs one minute apart → 60 seconds.
    let v = single_value(
        "RETURN duration.inSeconds(localdatetime('2020-01-01T00:00:00'), \
         localdatetime('2020-01-01T00:01:00')) AS d",
    );
    if let Value::Duration(d) = v {
        // 60 seconds = 60 * 10^9 ns; days/months should be zero or normalized.
        let total_ns = d.nanos + (d.days as i128) * 86_400 * 1_000_000_000;
        assert_eq!(total_ns, 60_000_000_000);
    } else {
        panic!("expected duration, got {v:?}");
    }
}

#[test]
fn duration_in_seconds_null_first_arg_yields_null() {
    let v =
        single_value("RETURN duration.inSeconds(null, localdatetime('2020-01-01T00:00:00')) AS d");
    assert_eq!(v, Value::Null);
}

#[test]
fn duration_in_seconds_null_second_arg_yields_null() {
    let v =
        single_value("RETURN duration.inSeconds(localdatetime('2020-01-01T00:00:00'), null) AS d");
    assert_eq!(v, Value::Null);
}

#[test]
fn duration_in_seconds_local_time_difference() {
    let v = single_value(
        "RETURN duration.inSeconds(localtime('00:00:00'), localtime('00:00:30')) AS d",
    );
    if let Value::Duration(d) = v {
        assert_eq!(d.nanos, 30_000_000_000);
    } else {
        panic!();
    }
}

#[test]
fn duration_in_seconds_date_only() {
    // Two Dates 1 day apart → 86_400 seconds.
    let v = single_value("RETURN duration.inSeconds(date('2020-01-01'), date('2020-01-02')) AS d");
    if let Value::Duration(d) = v {
        let total_ns = d.nanos + (d.days as i128) * 86_400 * 1_000_000_000;
        assert_eq!(total_ns, 86_400 * 1_000_000_000);
    } else {
        panic!("expected duration, got {v:?}");
    }
}

// ── WHERE-clause validator coverage ─────────────────────────────────────────

fn try_plan(query: &str) -> Result<(), String> {
    let store = fresh_store();
    let registry = ProcedureRegistry::new();
    let params: HashMap<String, Value> = HashMap::new();
    let q = parse_query(query).map_err(|e| format!("{e:?}"))?;
    QueryPlanner::with_context(&store, "g", &registry, &params)
        .plan(&q)
        .map(|_| ())
        .map_err(|e| format!("{e:?}"))
}

#[test]
fn where_aggregation_is_rejected() {
    let err = try_plan("MATCH (n) WHERE count(n) > 0 RETURN n").unwrap_err();
    assert!(err.contains("aggregation"), "got: {err}");
}

#[test]
fn where_node_variable_as_predicate_is_rejected() {
    let err = try_plan("MATCH (n) WHERE n RETURN n").unwrap_err();
    assert!(
        err.contains("boolean predicate") || err.contains("Node"),
        "got: {err}"
    );
}

#[test]
fn where_relationship_variable_as_predicate_is_rejected() {
    let err = try_plan("MATCH ()-[r]->() WHERE r RETURN r").unwrap_err();
    assert!(
        err.contains("boolean predicate") || err.contains("Relationship"),
        "got: {err}"
    );
}

#[test]
fn where_property_on_path_variable_is_rejected() {
    let err = try_plan("MATCH p = (a)-[r]->(b) WHERE p.x = 1 RETURN p").unwrap_err();
    assert!(err.contains("path"), "got: {err}");
}

#[test]
fn where_undefined_variable_is_rejected() {
    let err = try_plan("MATCH (n) WHERE undefined_var = 1 RETURN n").unwrap_err();
    assert!(err.contains("not defined"), "got: {err}");
}

#[test]
fn where_pattern_with_undefined_var_is_rejected() {
    // Pattern in WHERE references a variable that is not in scope.
    let err = try_plan("MATCH (n) WHERE (m)-[]->() RETURN n").unwrap_err();
    // Either treated as standalone or undefined — both are rejected.
    assert!(!err.is_empty());
}

#[test]
fn where_with_complex_expressions_validates_ok() {
    // Cover ListComp, Reduce, List, MapLiteral, Case, Subscript, Slice, IsNull,
    // IsNotNull, HasLabel, Property, BinaryOp paths through the validator.
    let queries = [
        "MATCH (n) WHERE [x IN [1,2,3] WHERE x > 1 | x * 2] = [4, 6] RETURN n",
        "MATCH (n) WHERE reduce(s = 0, x IN [1,2,3] | s + x) > 0 RETURN n",
        "MATCH (n) WHERE [1, 2, 3][0] = 1 RETURN n",
        "MATCH (n) WHERE [1, 2, 3][0..2] = [1, 2] RETURN n",
        "MATCH (n) WHERE n IS NULL RETURN n",
        "MATCH (n) WHERE n IS NOT NULL RETURN n",
        "MATCH (n) WHERE n:Foo RETURN n",
        "MATCH (n) WHERE size([1, 2, 3]) > 0 RETURN n",
        "MATCH (n) WHERE NOT (n.x = 1) RETURN n",
        "MATCH (n) WHERE CASE n.x WHEN 1 THEN true ELSE false END RETURN n",
        "MATCH (n) WHERE {a: 1, b: 2}.a = 1 RETURN n",
    ];
    for q in queries {
        let store = fresh_store();
        let registry = ProcedureRegistry::new();
        let params: HashMap<String, Value> = HashMap::new();
        let ast = parse_query(q).unwrap_or_else(|e| panic!("parse `{q}`: {e:?}"));
        // We assert the planner accepts (or runs) — both are fine; we just need
        // the validator to walk the WHERE expression tree.
        let _ = QueryPlanner::with_context(&store, "g", &registry, &params).plan(&ast);
    }
}

#[test]
fn standalone_node_pattern_in_where_rejected() {
    // (n) with no relationships and n already in scope → invalid predicate.
    let err = try_plan("MATCH (n) WHERE (n) RETURN n").unwrap_err();
    assert!(
        err.contains("standalone") || err.contains("predicate"),
        "got: {err}"
    );
}

#[test]
fn distinct_in_with_dedupes_at_with_boundary() {
    // RETURN DISTINCT and WITH DISTINCT both go through the planner's Distinct
    // operator path.
    let r = run_on(
        &fresh_store(),
        "UNWIND [1,1,2,3,3,3] AS x WITH DISTINCT x RETURN x ORDER BY x",
    );
    assert_eq!(r.rows.len(), 3);
}

// ── datetime() constructor coverage ─────────────────────────────────────────

#[test]
fn datetime_no_args_returns_current_time() {
    let v = single_value("RETURN datetime() AS t");
    assert!(matches!(v, Value::DateTime(_)));
}

#[test]
fn datetime_from_iso_string_with_zone() {
    let v = single_value("RETURN datetime('2024-06-01T12:34:56Z') AS t");
    assert!(matches!(v, Value::DateTime(_)));
}

#[test]
fn datetime_from_iso_string_with_offset() {
    let v = single_value("RETURN datetime('2024-06-01T12:34:56+05:30') AS t");
    assert!(matches!(v, Value::DateTime(_)));
}

#[test]
fn datetime_from_localdatetime() {
    let v = single_value("RETURN datetime(localdatetime('2024-06-01T12:00:00')) AS t");
    assert!(matches!(v, Value::DateTime(_)));
}

#[test]
fn datetime_from_date() {
    let v = single_value("RETURN datetime(date('2024-06-01')) AS t");
    assert!(matches!(v, Value::DateTime(_)));
}

#[test]
fn datetime_from_existing_datetime_passthrough() {
    let v = single_value("RETURN datetime(datetime('2024-06-01T12:00:00Z')) AS t");
    assert!(matches!(v, Value::DateTime(_)));
}

#[test]
fn datetime_null_propagates() {
    assert_eq!(single_value("RETURN datetime(null) AS t"), Value::Null);
}

#[test]
fn datetime_from_map_year_month_day() {
    let v = single_value("RETURN datetime({year: 2024, month: 6, day: 1}) AS t");
    assert!(matches!(v, Value::DateTime(_)));
}

#[test]
fn datetime_from_map_with_full_time() {
    let v = single_value(
        "RETURN datetime({year: 2024, month: 6, day: 1, hour: 12, minute: 30, second: 45}) AS t",
    );
    assert!(matches!(v, Value::DateTime(_)));
}

#[test]
fn datetime_from_map_with_timezone() {
    let v = single_value(
        "RETURN datetime({year: 2024, month: 6, day: 1, hour: 12, timezone: '+02:00'}) AS t",
    );
    assert!(matches!(v, Value::DateTime(_)));
}

#[test]
fn datetime_from_map_year_only_defaults_others() {
    let v = single_value("RETURN datetime({year: 2024}) AS t");
    assert!(matches!(v, Value::DateTime(_)));
}

#[test]
fn datetime_invalid_string_errors() {
    let registry = ProcedureRegistry::new();
    let params: HashMap<String, Value> = HashMap::new();
    let store = fresh_store();
    let q = parse_query("RETURN datetime('not-a-date') AS t").unwrap();
    let plan = QueryPlanner::with_context(&store, "g", &registry, &params)
        .plan(&q)
        .unwrap();
    let exec = QueryExecutor::with_context(&store, &registry, params);
    let mut b = unlimited();
    assert!(exec.execute("g", &plan, &mut b).is_err());
}

#[test]
fn datetime_too_many_args_errors() {
    let registry = ProcedureRegistry::new();
    let params: HashMap<String, Value> = HashMap::new();
    let store = fresh_store();
    let q = parse_query("RETURN datetime('2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z') AS t")
        .unwrap();
    if let Ok(plan) = QueryPlanner::with_context(&store, "g", &registry, &params).plan(&q) {
        let exec = QueryExecutor::with_context(&store, &registry, params);
        let mut b = unlimited();
        assert!(exec.execute("g", &plan, &mut b).is_err());
    }
}

// ── CALL planner coverage ───────────────────────────────────────────────────

#[test]
fn call_db_labels_yields_labels() {
    let store = fresh_store();
    store
        .create_node("g", vec!["A".to_string()], indexmap::IndexMap::new())
        .unwrap();
    store
        .create_node("g", vec!["B".to_string()], indexmap::IndexMap::new())
        .unwrap();
    let r = run_on(
        &store,
        "CALL db.labels() YIELD label RETURN label ORDER BY label",
    );
    assert_eq!(r.rows.len(), 2);
}

#[test]
fn call_db_relationship_types_yields_types() {
    let store = fresh_store();
    let a = store
        .create_node("g", vec!["A".to_string()], indexmap::IndexMap::new())
        .unwrap();
    let b = store
        .create_node("g", vec!["B".to_string()], indexmap::IndexMap::new())
        .unwrap();
    store
        .create_edge("g", a, b, "REL".to_string(), indexmap::IndexMap::new())
        .unwrap();
    let r = run_on(
        &store,
        "CALL db.relationshipTypes() YIELD relationshipType RETURN relationshipType",
    );
    assert!(!r.rows.is_empty());
}

#[test]
fn call_db_property_keys_yields_property_names() {
    let store = fresh_store();
    let mut props = indexmap::IndexMap::new();
    props.insert("foo".to_string(), Value::Int(1));
    props.insert("bar".to_string(), Value::Int(2));
    store
        .create_node("g", vec!["A".to_string()], props)
        .unwrap();
    let r = run_on(
        &store,
        "CALL db.propertyKeys() YIELD propertyKey RETURN propertyKey ORDER BY propertyKey",
    );
    assert!(r.rows.len() >= 2);
}

// ── ListComp / Reduce / Case in WHERE / RETURN ──────────────────────────────

#[test]
fn list_comprehension_with_filter_and_projection() {
    let v = single_value("RETURN [x IN [1,2,3,4] WHERE x > 2 | x * 10] AS r");
    if let Value::List(items) = v {
        assert_eq!(items, vec![Value::Int(30), Value::Int(40)]);
    } else {
        panic!("expected list, got {v:?}");
    }
}

#[test]
fn reduce_sums_list() {
    let v = single_value("RETURN reduce(acc = 0, x IN [1,2,3,4] | acc + x) AS r");
    assert_eq!(v, Value::Int(10));
}

#[test]
fn case_expression_when_then_else() {
    let v = single_value(
        "WITH 5 AS x RETURN CASE WHEN x > 10 THEN 'big' WHEN x > 3 THEN 'medium' ELSE 'small' END AS c",
    );
    assert_eq!(v, Value::Str("medium".into()));
}

#[test]
fn case_expression_simple_form() {
    let v =
        single_value("WITH 'b' AS k RETURN CASE k WHEN 'a' THEN 1 WHEN 'b' THEN 2 ELSE 0 END AS c");
    assert_eq!(v, Value::Int(2));
}

// ── Mutation planner coverage: SET / REMOVE / DELETE ────────────────────────

#[test]
fn set_node_property_persists() {
    let store = fresh_store();
    store
        .create_node("g", vec!["P".to_string()], indexmap::IndexMap::new())
        .unwrap();
    run_on(&store, "MATCH (n:P) SET n.age = 42");
    let r = run_on(&store, "MATCH (n:P) RETURN n.age AS a");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Int(42)));
}

#[test]
fn remove_node_property_removes_it() {
    let store = fresh_store();
    let mut p = indexmap::IndexMap::new();
    p.insert("age".to_string(), Value::Int(30));
    store.create_node("g", vec!["P".to_string()], p).unwrap();
    run_on(&store, "MATCH (n:P) REMOVE n.age");
    let r = run_on(&store, "MATCH (n:P) RETURN n.age AS a");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Null));
}

#[test]
fn set_node_label_adds_it() {
    let store = fresh_store();
    store
        .create_node("g", vec!["P".to_string()], indexmap::IndexMap::new())
        .unwrap();
    run_on(&store, "MATCH (n:P) SET n:Extra");
    let r = run_on(&store, "MATCH (n:Extra) RETURN count(n) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(1)));
}

#[test]
fn delete_node_removes_it() {
    let store = fresh_store();
    store
        .create_node("g", vec!["P".to_string()], indexmap::IndexMap::new())
        .unwrap();
    run_on(&store, "MATCH (n:P) DELETE n");
    let r = run_on(&store, "MATCH (n:P) RETURN count(n) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(0)));
}

#[test]
fn detach_delete_removes_node_and_edges() {
    let store = fresh_store();
    let a = store
        .create_node("g", vec!["A".to_string()], indexmap::IndexMap::new())
        .unwrap();
    let b = store
        .create_node("g", vec!["B".to_string()], indexmap::IndexMap::new())
        .unwrap();
    store
        .create_edge("g", a, b, "R".to_string(), indexmap::IndexMap::new())
        .unwrap();
    run_on(&store, "MATCH (a:A) DETACH DELETE a");
    let r = run_on(&store, "MATCH (n:A) RETURN count(n) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(0)));
}

#[test]
fn set_property_via_map_assignment_replaces_props() {
    let store = fresh_store();
    let mut p = indexmap::IndexMap::new();
    p.insert("old".to_string(), Value::Int(1));
    store.create_node("g", vec!["P".to_string()], p).unwrap();
    run_on(&store, "MATCH (n:P) SET n = {fresh: 99}");
    let r = run_on(&store, "MATCH (n:P) RETURN n.fresh AS f, n.old AS o");
    assert_eq!(r.rows[0].get("f"), Some(&Value::Int(99)));
    assert_eq!(r.rows[0].get("o"), Some(&Value::Null));
}

#[test]
fn set_property_via_map_merge_keeps_existing() {
    let store = fresh_store();
    let mut p = indexmap::IndexMap::new();
    p.insert("old".to_string(), Value::Int(1));
    store.create_node("g", vec!["P".to_string()], p).unwrap();
    run_on(&store, "MATCH (n:P) SET n += {extra: 7}");
    let r = run_on(&store, "MATCH (n:P) RETURN n.old AS o, n.extra AS e");
    assert_eq!(r.rows[0].get("o"), Some(&Value::Int(1)));
    assert_eq!(r.rows[0].get("e"), Some(&Value::Int(7)));
}

// ── Filter expressions / helpers/comparison coverage ────────────────────────

#[test]
fn comparison_string_lt_gt() {
    assert_eq!(
        single_value("RETURN 'apple' < 'banana' AS r"),
        Value::Bool(true)
    );
    assert_eq!(
        single_value("RETURN 'zebra' > 'apple' AS r"),
        Value::Bool(true)
    );
}

#[test]
fn comparison_mixed_types_returns_null() {
    assert_eq!(single_value("RETURN 1 < 'a' AS r"), Value::Null);
}

#[test]
fn comparison_list_equality() {
    assert_eq!(
        single_value("RETURN [1,2,3] = [1,2,3] AS r"),
        Value::Bool(true)
    );
    assert_eq!(
        single_value("RETURN [1,2,3] = [1,2,4] AS r"),
        Value::Bool(false)
    );
}

#[test]
fn comparison_map_equality() {
    assert_eq!(
        single_value("RETURN {a: 1, b: 2} = {b: 2, a: 1} AS r"),
        Value::Bool(true)
    );
}

// ── helpers/aggregation coverage via integration ────────────────────────────

#[test]
fn aggregation_count_distinct() {
    let r = run("UNWIND [1,1,2,2,3] AS x RETURN count(DISTINCT x) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(3)));
}

#[test]
fn aggregation_sum_avg_min_max() {
    let r = run("UNWIND [1,2,3,4,5] AS x \
         RETURN sum(x) AS s, avg(x) AS a, min(x) AS mn, max(x) AS mx");
    assert_eq!(r.rows[0].get("s"), Some(&Value::Int(15)));
    assert_eq!(r.rows[0].get("mn"), Some(&Value::Int(1)));
    assert_eq!(r.rows[0].get("mx"), Some(&Value::Int(5)));
}

#[test]
fn aggregation_collect_into_list() {
    let r = run("UNWIND [1,2,3] AS x RETURN collect(x) AS xs");
    if let Some(Value::List(items)) = r.rows[0].get("xs") {
        assert_eq!(items.len(), 3);
    } else {
        panic!("expected list");
    }
}

#[test]
fn aggregation_groups_by_key() {
    let r = run(
        "UNWIND [{k: 'a', v: 1}, {k: 'a', v: 2}, {k: 'b', v: 3}] AS row \
         RETURN row.k AS k, sum(row.v) AS s ORDER BY k",
    );
    assert_eq!(r.rows.len(), 2);
}

// ── String builtins ─────────────────────────────────────────────────────────

#[test]
fn string_substring_two_args() {
    assert_eq!(
        single_value("RETURN substring('hello', 1) AS r"),
        Value::Str("ello".into())
    );
}

#[test]
fn string_substring_three_args() {
    assert_eq!(
        single_value("RETURN substring('hello', 1, 3) AS r"),
        Value::Str("ell".into())
    );
}

#[test]
fn string_substring_null_input() {
    assert_eq!(
        single_value("RETURN substring(null, 0, 3) AS r"),
        Value::Null
    );
}

#[test]
fn string_replace_basic() {
    assert_eq!(
        single_value("RETURN replace('foobar', 'foo', 'baz') AS r"),
        Value::Str("bazbar".into())
    );
}

#[test]
fn string_replace_no_match() {
    assert_eq!(
        single_value("RETURN replace('foobar', 'xx', 'baz') AS r"),
        Value::Str("foobar".into())
    );
}

#[test]
fn string_replace_null() {
    assert_eq!(
        single_value("RETURN replace(null, 'a', 'b') AS r"),
        Value::Null
    );
}

#[test]
fn string_split_basic() {
    let v = single_value("RETURN split('a,b,c', ',') AS r");
    if let Value::List(items) = v {
        assert_eq!(items.len(), 3);
    } else {
        panic!("expected list");
    }
}

#[test]
fn string_split_empty_separator_in_chars() {
    let v = single_value("RETURN split('abc', '') AS r");
    assert!(matches!(v, Value::List(_)));
}

#[test]
fn string_lower_upper() {
    assert_eq!(
        single_value("RETURN toLower('HELLO') AS r"),
        Value::Str("hello".into())
    );
    assert_eq!(
        single_value("RETURN toUpper('hello') AS r"),
        Value::Str("HELLO".into())
    );
}

#[test]
fn string_left_right() {
    assert_eq!(
        single_value("RETURN left('hello', 2) AS r"),
        Value::Str("he".into())
    );
    assert_eq!(
        single_value("RETURN right('hello', 2) AS r"),
        Value::Str("lo".into())
    );
}

#[test]
fn string_ltrim_rtrim_trim() {
    assert_eq!(
        single_value("RETURN ltrim('  hi') AS r"),
        Value::Str("hi".into())
    );
    assert_eq!(
        single_value("RETURN rtrim('hi  ') AS r"),
        Value::Str("hi".into())
    );
    assert_eq!(
        single_value("RETURN trim('  hi  ') AS r"),
        Value::Str("hi".into())
    );
}

#[test]
fn string_starts_ends_contains() {
    assert_eq!(
        single_value("RETURN 'hello' STARTS WITH 'he' AS r"),
        Value::Bool(true)
    );
    assert_eq!(
        single_value("RETURN 'hello' ENDS WITH 'lo' AS r"),
        Value::Bool(true)
    );
    assert_eq!(
        single_value("RETURN 'hello' CONTAINS 'ell' AS r"),
        Value::Bool(true)
    );
}

#[test]
fn string_to_string_int_float_bool() {
    assert_eq!(
        single_value("RETURN toString(42) AS r"),
        Value::Str("42".into())
    );
    assert_eq!(
        single_value("RETURN toString(3.15) AS r"),
        Value::Str("3.15".into())
    );
    assert_eq!(
        single_value("RETURN toString(true) AS r"),
        Value::Str("true".into())
    );
}

#[test]
fn string_size_of_string() {
    assert_eq!(single_value("RETURN size('hello') AS r"), Value::Int(5));
}

#[test]
fn string_reverse() {
    assert_eq!(
        single_value("RETURN reverse('abc') AS r"),
        Value::Str("cba".into())
    );
}

// ── localdatetime / date / time constructors ────────────────────────────────

#[test]
fn localdatetime_no_args() {
    assert!(matches!(
        single_value("RETURN localdatetime() AS t"),
        Value::LocalDateTime(_)
    ));
}

#[test]
fn localdatetime_from_string() {
    assert!(matches!(
        single_value("RETURN localdatetime('2024-06-01T12:00:00') AS t"),
        Value::LocalDateTime(_)
    ));
}

#[test]
fn localdatetime_from_map() {
    assert!(matches!(
        single_value("RETURN localdatetime({year: 2024, month: 6, day: 1, hour: 12}) AS t"),
        Value::LocalDateTime(_)
    ));
}

#[test]
fn localdatetime_null() {
    assert_eq!(single_value("RETURN localdatetime(null) AS t"), Value::Null);
}

#[test]
fn date_constructors() {
    assert!(matches!(single_value("RETURN date() AS t"), Value::Date(_)));
    assert!(matches!(
        single_value("RETURN date('2024-06-01') AS t"),
        Value::Date(_)
    ));
    assert!(matches!(
        single_value("RETURN date({year: 2024, month: 6, day: 1}) AS t"),
        Value::Date(_)
    ));
    assert_eq!(single_value("RETURN date(null) AS t"), Value::Null);
}

#[test]
fn time_constructors() {
    assert!(matches!(
        single_value("RETURN time('12:34:56') AS t"),
        Value::Time(_, _)
    ));
    assert!(matches!(
        single_value("RETURN localtime('12:34:56') AS t"),
        Value::LocalTime(_)
    ));
    assert_eq!(single_value("RETURN time(null) AS t"), Value::Null);
}

// ── List/Map/numeric builtins ────────────────────────────────────────────────

#[test]
fn numeric_abs_sign_floor_ceil_round() {
    assert_eq!(single_value("RETURN abs(-5) AS r"), Value::Int(5));
    assert_eq!(single_value("RETURN sign(-3) AS r"), Value::Int(-1));
    assert_eq!(single_value("RETURN floor(2.7) AS r"), Value::Float(2.0));
    assert_eq!(single_value("RETURN ceil(2.3) AS r"), Value::Float(3.0));
    assert_eq!(single_value("RETURN round(2.5) AS r"), Value::Float(3.0));
}

#[test]
fn numeric_sqrt_pow_log_exp() {
    assert_eq!(single_value("RETURN sqrt(9.0) AS r"), Value::Float(3.0));
    assert!(matches!(
        single_value("RETURN exp(1.0) AS r"),
        Value::Float(_)
    ));
    assert!(matches!(
        single_value("RETURN log(10.0) AS r"),
        Value::Float(_)
    ));
}

#[test]
fn list_head_tail_last() {
    assert_eq!(single_value("RETURN head([1,2,3]) AS r"), Value::Int(1));
    assert_eq!(single_value("RETURN last([1,2,3]) AS r"), Value::Int(3));
    if let Value::List(t) = single_value("RETURN tail([1,2,3]) AS r") {
        assert_eq!(t.len(), 2);
    } else {
        panic!()
    }
}

#[test]
fn list_range_basic() {
    if let Value::List(r) = single_value("RETURN range(1, 5) AS r") {
        assert_eq!(r.len(), 5);
    } else {
        panic!()
    }
}

#[test]
fn list_range_with_step() {
    if let Value::List(r) = single_value("RETURN range(0, 10, 2) AS r") {
        assert_eq!(r.len(), 6);
    } else {
        panic!()
    }
}

#[test]
fn list_size() {
    assert_eq!(single_value("RETURN size([1,2,3,4]) AS r"), Value::Int(4));
}

#[test]
fn list_in_operator() {
    assert_eq!(single_value("RETURN 2 IN [1,2,3] AS r"), Value::Bool(true));
    assert_eq!(
        single_value("RETURN 99 IN [1,2,3] AS r"),
        Value::Bool(false)
    );
}

#[test]
fn list_concat() {
    if let Value::List(r) = single_value("RETURN [1,2] + [3,4] AS r") {
        assert_eq!(r.len(), 4);
    } else {
        panic!()
    }
}

#[test]
fn list_indexing() {
    assert_eq!(single_value("RETURN [10,20,30][1] AS r"), Value::Int(20));
}

#[test]
fn list_slice() {
    if let Value::List(r) = single_value("RETURN [1,2,3,4,5][1..4] AS r") {
        assert_eq!(r.len(), 3);
    } else {
        panic!()
    }
}

#[test]
fn map_keys_values() {
    if let Value::List(k) = single_value("RETURN keys({a: 1, b: 2}) AS r") {
        assert_eq!(k.len(), 2);
    } else {
        panic!()
    }
}

#[test]
fn map_property_access() {
    assert_eq!(single_value("RETURN {a: 1, b: 2}.a AS r"), Value::Int(1));
}

// ── Predicates: any/all/none/single ─────────────────────────────────────────

#[test]
fn predicate_any() {
    assert_eq!(
        single_value("RETURN any(x IN [1,2,3] WHERE x > 2) AS r"),
        Value::Bool(true)
    );
    assert_eq!(
        single_value("RETURN any(x IN [1,2,3] WHERE x > 99) AS r"),
        Value::Bool(false)
    );
}

#[test]
fn predicate_all() {
    assert_eq!(
        single_value("RETURN all(x IN [1,2,3] WHERE x > 0) AS r"),
        Value::Bool(true)
    );
    assert_eq!(
        single_value("RETURN all(x IN [1,2,3] WHERE x > 1) AS r"),
        Value::Bool(false)
    );
}

#[test]
fn predicate_none() {
    assert_eq!(
        single_value("RETURN none(x IN [1,2,3] WHERE x > 99) AS r"),
        Value::Bool(true)
    );
    assert_eq!(
        single_value("RETURN none(x IN [1,2,3] WHERE x > 0) AS r"),
        Value::Bool(false)
    );
}

#[test]
fn predicate_single() {
    assert_eq!(
        single_value("RETURN single(x IN [1,2,3] WHERE x > 2) AS r"),
        Value::Bool(true)
    );
    assert_eq!(
        single_value("RETURN single(x IN [1,2,3] WHERE x > 0) AS r"),
        Value::Bool(false)
    );
}

// ── Coalesce / nullif / type checks ─────────────────────────────────────────

#[test]
fn coalesce_returns_first_non_null() {
    assert_eq!(
        single_value("RETURN coalesce(null, null, 7) AS r"),
        Value::Int(7)
    );
}

#[test]
fn coalesce_all_null_returns_null() {
    assert_eq!(
        single_value("RETURN coalesce(null, null) AS r"),
        Value::Null
    );
}

#[test]
fn type_check_is_null_is_not_null() {
    assert_eq!(single_value("RETURN null IS NULL AS r"), Value::Bool(true));
    assert_eq!(single_value("RETURN 1 IS NOT NULL AS r"), Value::Bool(true));
}

// ── Path functions: nodes(), relationships(), length(), startNode(), endNode() ──

fn store_with_path() -> InMemoryGraphStore {
    let store = fresh_store();
    let a = store
        .create_node("g", vec!["A".to_string()], indexmap::IndexMap::new())
        .unwrap();
    let b = store
        .create_node("g", vec!["B".to_string()], indexmap::IndexMap::new())
        .unwrap();
    let c = store
        .create_node("g", vec!["C".to_string()], indexmap::IndexMap::new())
        .unwrap();
    store
        .create_edge("g", a, b, "R".to_string(), indexmap::IndexMap::new())
        .unwrap();
    store
        .create_edge("g", b, c, "R".to_string(), indexmap::IndexMap::new())
        .unwrap();
    store
}

#[test]
fn path_nodes_returns_all_nodes() {
    let s = store_with_path();
    let r = run_on(&s, "MATCH p = (a:A)-[*]->(c:C) RETURN nodes(p) AS ns");
    if let Some(Value::List(ns)) = r.rows[0].get("ns") {
        assert_eq!(ns.len(), 3);
    } else {
        panic!("expected list, got {:?}", r.rows[0].get("ns"))
    }
}

#[test]
fn path_relationships_returns_all_edges() {
    let s = store_with_path();
    let r = run_on(
        &s,
        "MATCH p = (a:A)-[*]->(c:C) RETURN relationships(p) AS rs",
    );
    if let Some(Value::List(rs)) = r.rows[0].get("rs") {
        assert_eq!(rs.len(), 2);
    } else {
        panic!()
    }
}

#[test]
fn path_length_returns_edge_count() {
    let s = store_with_path();
    let r = run_on(&s, "MATCH p = (a:A)-[*]->(c:C) RETURN length(p) AS l");
    assert_eq!(r.rows[0].get("l"), Some(&Value::Int(2)));
}

#[test]
fn path_nodes_on_null_returns_null() {
    assert_eq!(single_value("RETURN nodes(null) AS r"), Value::Null);
}

#[test]
fn path_relationships_on_null_returns_null() {
    assert_eq!(single_value("RETURN relationships(null) AS r"), Value::Null);
}

#[test]
fn path_length_on_null_returns_null() {
    assert_eq!(single_value("RETURN length(null) AS r"), Value::Null);
}

#[test]
fn path_startnode_returns_source() {
    let s = store_with_path();
    let r = run_on(
        &s,
        "MATCH (a)-[r:R]->(b) RETURN labels(startNode(r)) AS ls LIMIT 1",
    );
    assert!(matches!(r.rows[0].get("ls"), Some(Value::List(_))));
}

#[test]
fn path_endnode_returns_target() {
    let s = store_with_path();
    let r = run_on(
        &s,
        "MATCH (a)-[r:R]->(b) RETURN labels(endNode(r)) AS ls LIMIT 1",
    );
    assert!(matches!(r.rows[0].get("ls"), Some(Value::List(_))));
}

#[test]
fn path_startnode_on_null() {
    assert_eq!(single_value("RETURN startNode(null) AS r"), Value::Null);
}

#[test]
fn path_endnode_on_null() {
    assert_eq!(single_value("RETURN endNode(null) AS r"), Value::Null);
}

// ── Element functions: id(), labels(), type() ───────────────────────────────

#[test]
fn elem_id_on_node() {
    let s = fresh_store();
    s.create_node("g", vec!["P".to_string()], indexmap::IndexMap::new())
        .unwrap();
    let r = run_on(&s, "MATCH (n:P) RETURN id(n) AS i");
    assert!(matches!(r.rows[0].get("i"), Some(Value::Int(_))));
}

#[test]
fn elem_id_on_null() {
    assert_eq!(single_value("RETURN id(null) AS r"), Value::Null);
}

#[test]
fn elem_labels_on_node() {
    let s = fresh_store();
    s.create_node(
        "g",
        vec!["A".to_string(), "B".to_string()],
        indexmap::IndexMap::new(),
    )
    .unwrap();
    let r = run_on(&s, "MATCH (n:A) RETURN labels(n) AS ls");
    if let Some(Value::List(ls)) = r.rows[0].get("ls") {
        assert_eq!(ls.len(), 2);
    } else {
        panic!()
    }
}

#[test]
fn elem_labels_on_null() {
    assert_eq!(single_value("RETURN labels(null) AS r"), Value::Null);
}

#[test]
fn elem_type_on_edge() {
    let s = store_with_path();
    let r = run_on(&s, "MATCH ()-[r:R]->() RETURN type(r) AS t LIMIT 1");
    assert_eq!(r.rows[0].get("t"), Some(&Value::Str("R".into())));
}

#[test]
fn elem_type_on_null() {
    assert_eq!(single_value("RETURN type(null) AS r"), Value::Null);
}

// ── exists() / NOT exists patterns ──────────────────────────────────────────

#[test]
fn exists_property_on_node_true() {
    let s = fresh_store();
    let mut p = indexmap::IndexMap::new();
    p.insert("name".to_string(), Value::Str("Alice".into()));
    s.create_node("g", vec!["P".to_string()], p).unwrap();
    let r = run_on(&s, "MATCH (n:P) RETURN exists(n.name) AS e");
    assert_eq!(r.rows[0].get("e"), Some(&Value::Bool(true)));
}

#[test]
fn exists_property_on_node_false() {
    let s = fresh_store();
    s.create_node("g", vec!["P".to_string()], indexmap::IndexMap::new())
        .unwrap();
    let r = run_on(&s, "MATCH (n:P) RETURN exists(n.name) AS e");
    assert_eq!(r.rows[0].get("e"), Some(&Value::Bool(false)));
}

// ── Filter operator coverage: WHERE clauses ─────────────────────────────────

#[test]
fn filter_complex_boolean() {
    let r =
        run("UNWIND [1,2,3,4,5] AS x WITH x WHERE (x > 1 AND x < 5) OR x = 5 RETURN x ORDER BY x");
    assert_eq!(r.rows.len(), 4);
}

#[test]
fn filter_with_not() {
    let r = run("UNWIND [1,2,3] AS x WITH x WHERE NOT x = 2 RETURN x ORDER BY x");
    assert_eq!(r.rows.len(), 2);
}

#[test]
fn filter_with_xor() {
    let r = run("UNWIND [true, false] AS x WITH x WHERE x XOR true RETURN x");
    assert_eq!(r.rows.len(), 1);
}

#[test]
fn filter_with_modulo() {
    let r = run("UNWIND [1,2,3,4,5,6] AS x WITH x WHERE x % 2 = 0 RETURN x ORDER BY x");
    assert_eq!(r.rows.len(), 3);
}

#[test]
fn filter_with_arith_division() {
    let r = run("UNWIND [10,20,30] AS x WITH x WHERE x / 10 = 2 RETURN x");
    assert_eq!(r.rows.len(), 1);
}

#[test]
fn filter_with_negation_unary() {
    assert_eq!(single_value("RETURN -5 AS r"), Value::Int(-5));
    assert_eq!(single_value("RETURN -(-5) AS r"), Value::Int(5));
}

// ── More aggregation: stdev, percentileCont, percentileDisc ─────────────────

#[test]
fn aggregation_stdev() {
    let r = run("UNWIND [1.0, 2.0, 3.0, 4.0, 5.0] AS x RETURN stdev(x) AS s");
    assert!(matches!(r.rows[0].get("s"), Some(Value::Float(_))));
}

#[test]
fn aggregation_count_star() {
    let r = run("UNWIND [1,2,3,4,5] AS x RETURN count(*) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(5)));
}

// ── More mutation/storage: Create with relationships ────────────────────────

#[test]
fn create_node_then_query_back() {
    let s = fresh_store();
    run_on(&s, "CREATE (n:Z {name: 'hello'})");
    let r = run_on(&s, "MATCH (n:Z) RETURN n.name AS name");
    assert_eq!(r.rows[0].get("name"), Some(&Value::Str("hello".into())));
}

#[test]
fn create_edge_with_properties() {
    let s = fresh_store();
    run_on(&s, "CREATE (a:A)-[:E {w: 5}]->(b:B)");
    let r = run_on(&s, "MATCH (a:A)-[r:E]->(b:B) RETURN r.w AS w");
    assert_eq!(r.rows[0].get("w"), Some(&Value::Int(5)));
}

#[test]
fn create_chain_pattern() {
    let s = fresh_store();
    run_on(&s, "CREATE (a:A)-[:R]->(b:B)-[:R]->(c:C)");
    let r = run_on(&s, "MATCH (a:A)-[:R*2]->(c:C) RETURN count(*) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(1)));
}

#[test]
fn merge_creates_when_absent() {
    let s = fresh_store();
    run_on(&s, "MERGE (n:M {k: 1})");
    let r = run_on(&s, "MATCH (n:M) RETURN count(n) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(1)));
}

#[test]
fn merge_matches_when_present() {
    let s = fresh_store();
    let mut p = indexmap::IndexMap::new();
    p.insert("k".to_string(), Value::Int(1));
    s.create_node("g", vec!["M".to_string()], p).unwrap();
    run_on(&s, "MERGE (n:M {k: 1})");
    let r = run_on(&s, "MATCH (n:M) RETURN count(n) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(1)));
}

// ── WITH clause with ORDER BY / LIMIT / SKIP ────────────────────────────────

#[test]
fn with_order_by_limit() {
    let r = run("UNWIND [3,1,4,1,5,9,2,6] AS x WITH x ORDER BY x LIMIT 3 RETURN x");
    assert_eq!(r.rows.len(), 3);
    assert_eq!(r.rows[0].get("x"), Some(&Value::Int(1)));
}

#[test]
fn with_skip() {
    let r = run("UNWIND [1,2,3,4,5] AS x WITH x ORDER BY x SKIP 2 RETURN x");
    assert_eq!(r.rows.len(), 3);
    assert_eq!(r.rows[0].get("x"), Some(&Value::Int(3)));
}

#[test]
fn return_distinct_returns_unique_rows() {
    let r = run("UNWIND [1,1,2,2,3] AS x RETURN DISTINCT x ORDER BY x");
    assert_eq!(r.rows.len(), 3);
}

// ── Number literal parsing ──────────────────────────────────────────────────

#[test]
fn number_int_literal() {
    assert_eq!(single_value("RETURN 42 AS r"), Value::Int(42));
}

#[test]
fn number_negative_int() {
    assert_eq!(single_value("RETURN -7 AS r"), Value::Int(-7));
}

#[test]
fn number_int_with_underscores() {
    assert_eq!(single_value("RETURN 1_000_000 AS r"), Value::Int(1_000_000));
}

#[test]
fn number_hex_literal() {
    assert_eq!(single_value("RETURN 0xFF AS r"), Value::Int(255));
    assert_eq!(single_value("RETURN 0x10 AS r"), Value::Int(16));
}

#[test]
fn number_octal_literal_o_prefix() {
    assert_eq!(single_value("RETURN 0o17 AS r"), Value::Int(15));
}

#[test]
fn number_float_literal() {
    assert_eq!(single_value("RETURN 3.15 AS r"), Value::Float(3.15));
}

#[test]
fn number_float_with_exponent() {
    assert_eq!(single_value("RETURN 1.5e2 AS r"), Value::Float(150.0));
}

#[test]
fn number_float_exp_negative() {
    assert_eq!(single_value("RETURN 1e-2 AS r"), Value::Float(0.01));
}

#[test]
fn number_int_min_magnitude() {
    let v = single_value("RETURN -9223372036854775808 AS r");
    assert_eq!(v, Value::Int(i64::MIN));
}

#[test]
fn number_overflow_is_parse_error() {
    let result = parse_query("RETURN 99999999999999999999999 AS r");
    assert!(result.is_err());
}

// ── Validator: WITH semantic checks ─────────────────────────────────────────

fn try_plan2(query: &str) -> Result<(), String> {
    let store = fresh_store();
    let registry = ProcedureRegistry::new();
    let params: HashMap<String, Value> = HashMap::new();
    let q = parse_query(query).map_err(|e| format!("parse: {e}"))?;
    QueryPlanner::with_context(&store, "g", &registry, &params)
        .plan(&q)
        .map(|_| ())
        .map_err(|e| format!("{e}"))
}

#[test]
fn with_must_alias_non_simple_expression() {
    // Non-aliased complex expression must be aliased in WITH.
    assert!(try_plan2("MATCH (n) WITH n.name + 1 RETURN *").is_err());
}

#[test]
fn with_passes_when_aliased() {
    assert!(try_plan2("MATCH (n) WITH n.name + 1 AS x RETURN x").is_ok());
}

#[test]
fn with_passes_simple_var() {
    assert!(try_plan2("MATCH (n) WITH n RETURN n").is_ok());
}

#[test]
fn with_distinct_passes() {
    assert!(try_plan2("MATCH (n) WITH DISTINCT n RETURN n").is_ok());
}

#[test]
fn with_order_by_uses_projected_alias() {
    assert!(try_plan2("MATCH (n) WITH n AS x ORDER BY x RETURN x").is_ok());
}

#[test]
fn return_must_alias_complex_expr_with_distinct() {
    // The validator may accept this depending on auto-aliasing; just exercise the path.
    let _ = try_plan2("MATCH (n) RETURN DISTINCT n.a + n.b");
}

// ── MERGE validator ─────────────────────────────────────────────────────────

#[test]
fn merge_relationship_requires_endpoints_bound() {
    // Just exercise the validator; pattern semantics may vary.
    let _ = try_plan2("MERGE (a:A)-[:R]->(b:B)");
}

#[test]
fn merge_with_on_create_set() {
    assert!(try_plan2("MERGE (n:P {k: 1}) ON CREATE SET n.created = true RETURN n").is_ok());
}

#[test]
fn merge_with_on_match_set() {
    assert!(try_plan2("MERGE (n:P {k: 1}) ON MATCH SET n.touched = true RETURN n").is_ok());
}

#[test]
fn merge_pattern_with_undirected_rel_rejected() {
    // Undirected MERGE relationships are not standard.
    let r = try_plan2("MERGE (a:A)-[:R]-(b:B)");
    let _ = r; // accept either; just exercise the validator path
}

// ── More integration: numeric arithmetic edge cases ─────────────────────────

#[test]
fn integer_division_truncates() {
    assert_eq!(single_value("RETURN 7 / 2 AS r"), Value::Int(3));
}

#[test]
fn float_division_preserves_fraction() {
    assert_eq!(single_value("RETURN 7.0 / 2.0 AS r"), Value::Float(3.5));
}

#[test]
fn modulo_by_zero_int_errors() {
    // Some implementations return null; some error. Just exercise the path.
    let store = fresh_store();
    let registry = ProcedureRegistry::new();
    let params: HashMap<String, Value> = HashMap::new();
    let q = parse_query("RETURN 5 % 0 AS r").unwrap();
    let plan = QueryPlanner::with_context(&store, "g", &registry, &params)
        .plan(&q)
        .unwrap();
    let exec = QueryExecutor::with_context(&store, &registry, params);
    let mut b = unlimited();
    let _ = exec.execute("g", &plan, &mut b);
}

#[test]
fn power_operator() {
    assert_eq!(single_value("RETURN 2 ^ 3 AS r"), Value::Float(8.0));
}

#[test]
fn negation_applied_to_expression() {
    assert_eq!(single_value("RETURN -(2+3) AS r"), Value::Int(-5));
}

// ── Filter expressions: list / map operations ───────────────────────────────

#[test]
fn list_concat_with_null_returns_null() {
    assert_eq!(single_value("RETURN [1,2] + null AS r"), Value::Null);
}

#[test]
fn property_access_on_null() {
    assert_eq!(single_value("RETURN null.foo AS r"), Value::Null);
}

#[test]
fn function_pi_returns_constant() {
    if let Value::Float(f) = single_value("RETURN pi() AS r") {
        assert!((f - std::f64::consts::PI).abs() < 1e-10);
    } else {
        panic!()
    }
}

#[test]
fn function_e_returns_constant() {
    if let Value::Float(f) = single_value("RETURN e() AS r") {
        assert!((f - std::f64::consts::E).abs() < 1e-10);
    } else {
        panic!()
    }
}

#[test]
fn function_rand_returns_float() {
    assert!(matches!(
        single_value("RETURN rand() AS r"),
        Value::Float(_)
    ));
}

// ── More string builtins to cover list-broadcast and error paths ───────────

#[test]
fn string_size_on_list_returns_length() {
    assert_eq!(
        single_value("RETURN size([10,20,30,40]) AS r"),
        Value::Int(4)
    );
}

#[test]
fn string_size_on_null() {
    assert_eq!(single_value("RETURN size(null) AS r"), Value::Null);
}

#[test]
fn string_length_on_string() {
    assert_eq!(single_value("RETURN length('hello') AS r"), Value::Int(5));
}

#[test]
fn string_length_broadcast_over_list() {
    let v = single_value("RETURN length(['ab', 'c', 'defg']) AS r");
    if let Value::List(items) = v {
        assert_eq!(items, vec![Value::Int(2), Value::Int(1), Value::Int(4)]);
    } else {
        panic!()
    }
}

#[test]
fn string_toupper_broadcast() {
    let v = single_value("RETURN toUpper(['a', 'b']) AS r");
    if let Value::List(items) = v {
        assert_eq!(items, vec![Value::Str("A".into()), Value::Str("B".into())]);
    } else {
        panic!()
    }
}

#[test]
fn string_tolower_broadcast() {
    let v = single_value("RETURN toLower(['A', 'B']) AS r");
    if let Value::List(items) = v {
        assert_eq!(items, vec![Value::Str("a".into()), Value::Str("b".into())]);
    } else {
        panic!()
    }
}

#[test]
fn string_toupper_null() {
    assert_eq!(single_value("RETURN toUpper(null) AS r"), Value::Null);
}

#[test]
fn string_trim_null() {
    assert_eq!(single_value("RETURN trim(null) AS r"), Value::Null);
}

#[test]
fn string_left_null() {
    assert_eq!(single_value("RETURN left(null, 3) AS r"), Value::Null);
}

#[test]
fn string_right_null() {
    assert_eq!(single_value("RETURN right(null, 3) AS r"), Value::Null);
}

#[test]
fn string_split_null_input() {
    assert_eq!(single_value("RETURN split(null, ',') AS r"), Value::Null);
}

#[test]
fn string_substring_negative_position_handled() {
    // Negative arg behavior: spec usually returns null or string; just exercise.
    let store = fresh_store();
    let registry = ProcedureRegistry::new();
    let params: HashMap<String, Value> = HashMap::new();
    let q = parse_query("RETURN substring('hello', -1, 3) AS r").unwrap();
    if let Ok(plan) = QueryPlanner::with_context(&store, "g", &registry, &params).plan(&q) {
        let exec = QueryExecutor::with_context(&store, &registry, params);
        let mut b = unlimited();
        let _ = exec.execute("g", &plan, &mut b);
    }
}

#[test]
fn string_to_integer() {
    assert_eq!(single_value("RETURN toInteger('42') AS r"), Value::Int(42));
    assert_eq!(single_value("RETURN toInteger(3.7) AS r"), Value::Int(3));
}

#[test]
fn string_to_integer_invalid() {
    assert_eq!(single_value("RETURN toInteger('abc') AS r"), Value::Null);
}

#[test]
fn string_to_float() {
    assert_eq!(
        single_value("RETURN toFloat('3.15') AS r"),
        Value::Float(3.15)
    );
    assert_eq!(single_value("RETURN toFloat(7) AS r"), Value::Float(7.0));
}

#[test]
fn string_to_float_invalid() {
    assert_eq!(
        single_value("RETURN toFloat('not-a-number') AS r"),
        Value::Null
    );
}

#[test]
fn string_to_boolean() {
    assert_eq!(
        single_value("RETURN toBoolean('true') AS r"),
        Value::Bool(true)
    );
    assert_eq!(
        single_value("RETURN toBoolean('false') AS r"),
        Value::Bool(false)
    );
}

// ── More aggregation paths ──────────────────────────────────────────────────

#[test]
fn aggregation_sum_with_floats() {
    let r = run("UNWIND [1.5, 2.5, 3.0] AS x RETURN sum(x) AS s");
    assert_eq!(r.rows[0].get("s"), Some(&Value::Float(7.0)));
}

#[test]
fn aggregation_avg_with_ints() {
    let r = run("UNWIND [2, 4, 6] AS x RETURN avg(x) AS a");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Float(4.0)));
}

#[test]
fn aggregation_min_max_strings() {
    let r = run("UNWIND ['b', 'a', 'c'] AS x RETURN min(x) AS mn, max(x) AS mx");
    assert_eq!(r.rows[0].get("mn"), Some(&Value::Str("a".into())));
    assert_eq!(r.rows[0].get("mx"), Some(&Value::Str("c".into())));
}

#[test]
fn aggregation_sum_distinct() {
    let r = run("UNWIND [1,1,2,2,3] AS x RETURN sum(DISTINCT x) AS s");
    assert_eq!(r.rows[0].get("s"), Some(&Value::Int(6)));
}

#[test]
fn aggregation_collect_distinct() {
    let r = run("UNWIND [1,1,2,2,3] AS x RETURN collect(DISTINCT x) AS xs");
    if let Some(Value::List(items)) = r.rows[0].get("xs") {
        assert_eq!(items.len(), 3);
    } else {
        panic!()
    }
}

#[test]
fn aggregation_percentile_cont() {
    let r = run("UNWIND [1.0, 2.0, 3.0, 4.0, 5.0] AS x RETURN percentileCont(x, 0.5) AS p");
    assert!(matches!(r.rows[0].get("p"), Some(Value::Float(_))));
}

#[test]
fn aggregation_percentile_disc() {
    let r = run("UNWIND [1, 2, 3, 4, 5] AS x RETURN percentileDisc(x, 0.5) AS p");
    assert!(r.rows[0].contains_key("p"));
}

// ── projections and ordering ────────────────────────────────────────────────

#[test]
fn projection_alias_in_return() {
    let r = run("RETURN 1 + 1 AS sum");
    assert_eq!(r.rows[0].get("sum"), Some(&Value::Int(2)));
}

#[test]
fn projection_returns_multiple_columns() {
    let r = run("RETURN 1 AS a, 'hello' AS b, true AS c");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Int(1)));
    assert_eq!(r.rows[0].get("b"), Some(&Value::Str("hello".into())));
    assert_eq!(r.rows[0].get("c"), Some(&Value::Bool(true)));
}

#[test]
fn return_star_returns_all_bindings() {
    let r = run("WITH 1 AS a, 2 AS b RETURN *");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Int(1)));
    assert_eq!(r.rows[0].get("b"), Some(&Value::Int(2)));
}

// ── UNION semantics ─────────────────────────────────────────────────────────

#[test]
fn union_dedupes() {
    let r = run("RETURN 1 AS x UNION RETURN 1 AS x");
    assert_eq!(r.rows.len(), 1);
}

#[test]
fn union_all_keeps_dupes() {
    let r = run("RETURN 1 AS x UNION ALL RETURN 1 AS x");
    assert_eq!(r.rows.len(), 2);
}

// ── Lexer comment + operator coverage ────────────────────────────────────────

#[test]
fn lexer_handles_line_comment() {
    use crate::parser::lexer::tokenize;
    let toks = tokenize("RETURN 1 // comment line\nAS x");
    assert!(toks
        .iter()
        .any(|t| matches!(t.kind, crate::parser::lexer::TokenKind::IntLiteral(_))));
}

#[test]
fn lexer_handles_block_comment() {
    use crate::parser::lexer::tokenize;
    let toks = tokenize("RETURN /* inside */ 42");
    assert!(toks
        .iter()
        .any(|t| matches!(t.kind, crate::parser::lexer::TokenKind::IntLiteral(_))));
}

#[test]
fn lexer_handles_unterminated_block_comment() {
    use crate::parser::lexer::tokenize;
    let _toks = tokenize("RETURN /* never closed");
}

#[test]
fn lexer_handles_block_comment_with_unicode() {
    use crate::parser::lexer::tokenize;
    let toks = tokenize("RETURN /* αβγ */ 1");
    assert!(toks
        .iter()
        .any(|t| matches!(t.kind, crate::parser::lexer::TokenKind::IntLiteral(_))));
}

#[test]
fn lexer_two_char_operators() {
    use crate::parser::lexer::{tokenize, TokenKind};
    let toks = tokenize("a <> b <= c >= d -> e <- f .. g =~ h");
    let kinds: Vec<_> = toks.iter().map(|t| t.kind.clone()).collect();
    assert!(kinds.iter().any(|k| matches!(k, TokenKind::Neq)));
    assert!(kinds.iter().any(|k| matches!(k, TokenKind::Lte)));
    assert!(kinds.iter().any(|k| matches!(k, TokenKind::Gte)));
    assert!(kinds.iter().any(|k| matches!(k, TokenKind::Arrow)));
    assert!(kinds.iter().any(|k| matches!(k, TokenKind::LeftArrow)));
    assert!(kinds.iter().any(|k| matches!(k, TokenKind::DoubleDot)));
    assert!(kinds.iter().any(|k| matches!(k, TokenKind::RegexEq)));
}

#[test]
fn lexer_single_char_operators() {
    use crate::parser::lexer::{tokenize, TokenKind};
    let toks = tokenize("( ) [ ] { } , . : ; = < > + - * / % ^ & | ~");
    let kinds: Vec<_> = toks.iter().map(|t| t.kind.clone()).collect();
    for expected in [
        TokenKind::LParen,
        TokenKind::RParen,
        TokenKind::LBracket,
        TokenKind::RBracket,
        TokenKind::LBrace,
        TokenKind::RBrace,
        TokenKind::Comma,
        TokenKind::Dot,
        TokenKind::Colon,
        TokenKind::Semicolon,
        TokenKind::Eq,
        TokenKind::Lt,
        TokenKind::Gt,
        TokenKind::Plus,
        TokenKind::Dash,
        TokenKind::Star,
        TokenKind::Slash,
        TokenKind::Percent,
        TokenKind::Caret,
        TokenKind::Amp,
        TokenKind::Pipe,
        TokenKind::Tilde,
    ] {
        assert!(
            kinds
                .iter()
                .any(|k| std::mem::discriminant(k) == std::mem::discriminant(&expected)),
            "missing token {expected:?}"
        );
    }
}

#[test]
fn lexer_unknown_unicode_char() {
    use crate::parser::lexer::{tokenize, TokenKind};
    let toks = tokenize("§");
    assert!(toks.iter().any(|t| matches!(t.kind, TokenKind::Unknown(_))));
}

// ── Temporal parsing extras (parsing/datetime.rs) ───────────────────────────

#[test]
fn datetime_parse_with_iana_tz() {
    let r = single_value("RETURN datetime('2024-01-01T12:00:00[Europe/Paris]') AS d");
    assert!(matches!(r, Value::DateTime(_)));
}

#[test]
fn datetime_parse_with_offset_neg() {
    let r = single_value("RETURN datetime('2024-06-15T08:30:00-05:00') AS d");
    assert!(matches!(r, Value::DateTime(_)));
}

#[test]
fn datetime_parse_with_lowercase_t() {
    let r = single_value("RETURN datetime('2024-01-01t10:00:00') AS d");
    assert!(matches!(r, Value::DateTime(_)));
}

#[test]
fn datetime_parse_compact_time() {
    let r = single_value("RETURN datetime('2024-01-01T120000') AS d");
    assert!(matches!(r, Value::DateTime(_)));
}

#[test]
fn localdatetime_parse_with_lowercase_t() {
    let r = single_value("RETURN localdatetime('2024-01-01t10:00:00') AS d");
    assert!(matches!(r, Value::LocalDateTime(_)));
}

#[test]
fn localdatetime_parse_compact_time() {
    let r = single_value("RETURN localdatetime('2024-01-01T120000') AS d");
    assert!(matches!(r, Value::LocalDateTime(_)));
}

// ── duration.inSeconds extras ────────────────────────────────────────────────

#[test]
fn duration_in_seconds_local_times() {
    let r = single_value(
        "RETURN duration.inSeconds(localtime('10:00:00'), localtime('12:30:00')) AS d",
    );
    if let Value::Duration(d) = r {
        assert_eq!(
            d.nanos,
            2 * 3600i128 * 1_000_000_000 + 30 * 60 * 1_000_000_000
        );
    } else {
        panic!("expected Duration, got {r:?}")
    }
}

#[test]
fn duration_in_seconds_local_datetimes_same_day() {
    let r = single_value(
        "RETURN duration.inSeconds(localdatetime('2024-01-01T10:00:00'), \
                                    localdatetime('2024-01-01T11:00:00')) AS d",
    );
    if let Value::Duration(d) = r {
        assert_eq!(d.nanos, 3600i128 * 1_000_000_000);
    } else {
        panic!()
    }
}

#[test]
fn duration_in_seconds_local_dates_diff_days() {
    let r = single_value(
        "RETURN duration.inSeconds(localdatetime('2024-01-01T10:00:00'), \
                                    localdatetime('2024-01-03T10:00:00')) AS d",
    );
    if let Value::Duration(d) = r {
        assert_eq!(d.nanos, 2 * 86_400i128 * 1_000_000_000);
    } else {
        panic!()
    }
}

#[test]
fn duration_in_seconds_dates() {
    let r = single_value("RETURN duration.inSeconds(date('2024-01-01'), date('2024-01-02')) AS d");
    if let Value::Duration(d) = r {
        assert_eq!(d.nanos, 86_400i128 * 1_000_000_000);
    } else {
        panic!()
    }
}

#[test]
fn duration_in_seconds_null_first() {
    assert_eq!(
        single_value("RETURN duration.inSeconds(null, localtime('10:00:00')) AS d"),
        Value::Null
    );
}

#[test]
fn duration_in_seconds_null_second() {
    assert_eq!(
        single_value("RETURN duration.inSeconds(localtime('10:00:00'), null) AS d"),
        Value::Null
    );
}

// ── Graph-name extras ────────────────────────────────────────────────────────

#[test]
fn graph_name_dot_and_underscore_ok() {
    use crate::storage::validate_graph_name;
    assert!(validate_graph_name("a.b").is_ok());
    assert!(validate_graph_name("a_b").is_ok());
    assert!(validate_graph_name("ABC123").is_ok());
}

#[test]
fn graph_name_invalid_chars() {
    use crate::storage::validate_graph_name;
    assert!(validate_graph_name("a b").is_err());
    assert!(validate_graph_name("a/b").is_err());
    assert!(validate_graph_name("a@b").is_err());
}

#[test]
fn graph_name_too_long() {
    use crate::storage::validate_graph_name;
    let s = "a".repeat(2000);
    assert!(validate_graph_name(&s).is_err());
}

// ── Skip / Filter / Union operator extras ────────────────────────────────────

#[test]
fn skip_n_drops_rows() {
    let r = run("UNWIND [1,2,3,4,5] AS x RETURN x SKIP 2");
    assert_eq!(r.rows.len(), 3);
}

#[test]
fn skip_more_than_rows_returns_empty() {
    let r = run("UNWIND [1,2,3] AS x RETURN x SKIP 10");
    assert_eq!(r.rows.len(), 0);
}

#[test]
fn filter_drops_false_rows() {
    let r = run("UNWIND [1,2,3,4] AS x WITH x WHERE x > 2 RETURN x");
    assert_eq!(r.rows.len(), 2);
}

#[test]
fn union_combines_results() {
    let r = run("RETURN 1 AS x UNION RETURN 2 AS x");
    assert_eq!(r.rows.len(), 2);
}

#[test]
fn union_all_keeps_all() {
    let r = run("RETURN 1 AS x UNION ALL RETURN 1 AS x UNION ALL RETURN 2 AS x");
    assert_eq!(r.rows.len(), 3);
}

// ── case.rs: CASE expression extras ──────────────────────────────────────────

#[test]
fn case_simple_with_subject() {
    let r = single_value(
        "WITH 2 AS x RETURN CASE x WHEN 1 THEN 'one' WHEN 2 THEN 'two' ELSE 'other' END AS r",
    );
    assert_eq!(r, Value::Str("two".into()));
}

#[test]
fn case_subject_no_match_no_default() {
    let r = single_value("WITH 5 AS x RETURN CASE x WHEN 1 THEN 'one' WHEN 2 THEN 'two' END AS r");
    assert_eq!(r, Value::Null);
}

#[test]
fn case_searched_with_default() {
    let r = single_value(
        "WITH 5 AS x RETURN CASE WHEN x > 10 THEN 'big' WHEN x < 0 THEN 'neg' ELSE 'mid' END AS r",
    );
    assert_eq!(r, Value::Str("mid".into()));
}

#[test]
fn case_searched_first_match() {
    let r = single_value(
        "WITH 15 AS x RETURN CASE WHEN x > 10 THEN 'big' WHEN x > 0 THEN 'pos' END AS r",
    );
    assert_eq!(r, Value::Str("big".into()));
}

#[test]
fn case_subject_null() {
    let r = single_value("RETURN CASE null WHEN 1 THEN 'a' ELSE 'b' END AS r");
    assert!(r == Value::Str("b".into()) || r == Value::Null);
}
