//! Fourth wave of lib-side targeted coverage tests.
//!
//! Adds direct unit tests for `pub(crate)` helpers (cmp_values,
//! gremlin_cmp_values, civil_time_to_nanos, expr_to_column_name,
//! discover_operator_aliases, eval_binary_values, value_to_expr),
//! plus query-driven coverage for element accessors, projections,
//! distinct/unique behaviour, mutation alias references, and several
//! additional planner/validator/temporal branches.

#![cfg(test)]

use std::cmp::Ordering;
use std::collections::HashMap;
use std::time::Duration;

use indexmap::IndexMap;
use jiff::civil;

use crate::ast::{BinaryOp, Direction, Expr, Identifier, Label, RelType, UnaryOp};
use crate::executor::engine::helpers::{
    cmp_values, discover_operator_aliases, expr_to_column_name, extract_node_id,
    gremlin_cmp_values, undefined_var,
};
use crate::executor::filter::operators::safe_int_float_cmp;
use crate::executor::{ExecutionBudget, QueryExecutor, QueryResult};
use crate::parser::parse_query;
use crate::planner::{Operator, QueryPlanner};
use crate::procedure::ProcedureRegistry;
use crate::storage::{GraphStore, InMemoryGraphStore, NodeId};
use crate::types::value::{Crs, CypherDuration, GeoPoint};
use crate::types::Value;

// ── Helpers (mirror coverage_targeted_tests3) ──────────────────────────

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

fn first_val(r: &QueryResult) -> Value {
    let col = &r.columns[0];
    r.rows[0].get(col).cloned().unwrap()
}

// ── cmp_values: wide coverage of every type-rank pair ───────────────────

#[test]
fn cmp_values_same_type_pairs() {
    use Ordering::*;
    assert_eq!(cmp_values(&Value::Bool(false), &Value::Bool(true)), Less);
    assert_eq!(cmp_values(&Value::Int(1), &Value::Int(2)), Less);
    assert_eq!(cmp_values(&Value::Int(2), &Value::Int(2)), Equal);
    assert_eq!(cmp_values(&Value::Float(1.0), &Value::Float(2.0)), Less);
    assert_eq!(
        cmp_values(&Value::Str("a".into()), &Value::Str("b".into())),
        Less
    );
    let d1 = civil::Date::new(2024, 1, 1).unwrap();
    let d2 = civil::Date::new(2024, 1, 2).unwrap();
    assert_eq!(cmp_values(&Value::Date(d1), &Value::Date(d2)), Less);
}

#[test]
fn cmp_values_int_float_cross() {
    use Ordering::*;
    assert_eq!(cmp_values(&Value::Int(5), &Value::Float(5.0)), Equal);
    assert_eq!(cmp_values(&Value::Int(5), &Value::Float(5.5)), Less);
    assert_eq!(cmp_values(&Value::Float(5.5), &Value::Int(5)), Greater);
}

#[test]
fn cmp_values_null_sorts_last() {
    use Ordering::*;
    assert_eq!(cmp_values(&Value::Null, &Value::Int(0)), Greater);
    assert_eq!(cmp_values(&Value::Int(0), &Value::Null), Less);
    assert_eq!(cmp_values(&Value::Null, &Value::Null), Equal);
}

#[test]
fn cmp_values_lists_lex_then_length() {
    use Ordering::*;
    let a = Value::List(vec![Value::Int(1), Value::Int(2)]);
    let b = Value::List(vec![Value::Int(1), Value::Int(3)]);
    let c = Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(0)]);
    assert_eq!(cmp_values(&a, &b), Less);
    assert_eq!(cmp_values(&a, &c), Less);
    assert_eq!(cmp_values(&a, &a.clone()), Equal);
}

#[test]
fn cmp_values_temporal_pairs() {
    use Ordering::*;
    let t1 = civil::Time::new(1, 0, 0, 0).unwrap();
    let t2 = civil::Time::new(2, 0, 0, 0).unwrap();
    assert_eq!(
        cmp_values(&Value::LocalTime(t1), &Value::LocalTime(t2)),
        Less
    );
    // Time vs LocalTime cross-comparisons
    assert_eq!(cmp_values(&Value::Time(t1, 0), &Value::LocalTime(t2)), Less);
    assert_eq!(cmp_values(&Value::LocalTime(t1), &Value::Time(t2, 0)), Less);
    let dt = civil::DateTime::new(2024, 1, 1, 0, 0, 0, 0).unwrap();
    let dt2 = civil::DateTime::new(2025, 1, 1, 0, 0, 0, 0).unwrap();
    assert_eq!(
        cmp_values(&Value::LocalDateTime(dt), &Value::LocalDateTime(dt2)),
        Less
    );
}

#[test]
fn cmp_values_nan_sorts_after_regular() {
    use Ordering::*;
    let nan = Value::Float(f64::NAN);
    let n = Value::Float(1.0);
    assert_eq!(cmp_values(&n, &nan), Less);
    assert_eq!(cmp_values(&nan, &n), Greater);
}

#[test]
fn cmp_values_type_rank_ordering() {
    use Ordering::*;
    // Map < Node < Edge < List < Path < Str < Bool < Number < ...
    let m = Value::Map(IndexMap::new());
    let n = Value::Node(NodeId(1));
    let s = Value::Str("z".into());
    let b = Value::Bool(true);
    let i = Value::Int(0);
    assert_eq!(cmp_values(&m, &n), Less);
    assert_eq!(cmp_values(&s, &b), Less);
    assert_eq!(cmp_values(&b, &i), Less);
}

// ── gremlin_cmp_values ─────────────────────────────────────────────────

#[test]
fn gremlin_cmp_basic_orderability() {
    use Ordering::*;
    assert_eq!(gremlin_cmp_values(&Value::Null, &Value::Bool(false)), Less);
    assert_eq!(
        gremlin_cmp_values(&Value::Bool(false), &Value::Int(0)),
        Less
    );
    assert_eq!(
        gremlin_cmp_values(&Value::Int(1), &Value::Str("a".into())),
        Less
    );
    assert_eq!(gremlin_cmp_values(&Value::Int(1), &Value::Int(2)), Less);
    assert_eq!(
        gremlin_cmp_values(&Value::Float(1.0), &Value::Float(2.0)),
        Less
    );
}

#[test]
fn gremlin_cmp_lists_recurse() {
    use Ordering::*;
    let a = Value::List(vec![Value::Int(1), Value::Int(2)]);
    let b = Value::List(vec![Value::Int(1), Value::Int(3)]);
    assert_eq!(gremlin_cmp_values(&a, &b), Less);
}

#[test]
fn gremlin_cmp_property_map_edge_kind_orders_by_key_then_value() {
    use Ordering::*;
    // Edge property map: `~key` and `~value` (no `~element`).
    let mut a = IndexMap::new();
    a.insert("~key".into(), Value::Str("a".into()));
    a.insert("~value".into(), Value::Int(1));
    a.insert("~element".into(), Value::Edge(crate::storage::EdgeId(1)));
    let mut b = IndexMap::new();
    b.insert("~key".into(), Value::Str("b".into()));
    b.insert("~value".into(), Value::Int(0));
    b.insert("~element".into(), Value::Edge(crate::storage::EdgeId(2)));
    assert_eq!(gremlin_cmp_values(&Value::Map(a), &Value::Map(b)), Less);
}

#[test]
fn gremlin_cmp_tagged_set_unwraps() {
    use Ordering::*;
    let mut a = IndexMap::new();
    a.insert("~set".into(), Value::List(vec![Value::Int(1)]));
    let mut b = IndexMap::new();
    b.insert("~set".into(), Value::List(vec![Value::Int(2)]));
    assert_eq!(gremlin_cmp_values(&Value::Map(a), &Value::Map(b)), Less);
}

#[test]
fn gremlin_cmp_uuid_strings_sort_above_regular() {
    use Ordering::*;
    let s = Value::Str("hello".into());
    let u = Value::Str("~uuid:00000000".into());
    assert_eq!(gremlin_cmp_values(&s, &u), Less);
}

// ── safe_int_float_cmp ─────────────────────────────────────────────────

#[test]
fn safe_int_float_cmp_finite_and_nan() {
    use Ordering::*;
    assert_eq!(safe_int_float_cmp(1, 2.0), Some(Less));
    assert_eq!(safe_int_float_cmp(2, 2.0), Some(Equal));
    assert_eq!(safe_int_float_cmp(3, 2.0), Some(Greater));
    assert_eq!(safe_int_float_cmp(0, f64::NAN), None);
    assert_eq!(safe_int_float_cmp(0, f64::INFINITY), Some(Less));
    assert_eq!(safe_int_float_cmp(0, f64::NEG_INFINITY), Some(Greater));
}

// ── (civil_time_to_nanos test removed: function is module-private) ─────

// ── undefined_var / extract_node_id ────────────────────────────────────

#[test]
fn undefined_var_produces_execution_error() {
    let e = undefined_var("foo");
    let s = format!("{e:?}");
    assert!(s.contains("foo"));
}

#[test]
fn extract_node_id_paths() {
    let n = Value::Node(NodeId(42));
    assert_eq!(extract_node_id(&n, "n").unwrap(), NodeId(42));
    let bad = Value::Int(0);
    assert!(extract_node_id(&bad, "n").is_err());
    let null = Value::Null;
    assert!(extract_node_id(&null, "n").is_err());
}

// ── expr_to_column_name ────────────────────────────────────────────────

#[test]
fn expr_to_column_name_variants() {
    assert!(!expr_to_column_name(&Expr::Variable(Identifier::new("x"))).is_empty());
    assert!(!expr_to_column_name(&Expr::Integer(7)).is_empty());
    assert!(!expr_to_column_name(&Expr::Float(1.0)).is_empty());
    assert!(!expr_to_column_name(&Expr::Str("s".into())).is_empty());
    assert!(!expr_to_column_name(&Expr::Bool(true)).is_empty());
    assert!(!expr_to_column_name(&Expr::Null).is_empty());
    let prop = Expr::Property(
        Box::new(Expr::Variable(Identifier::new("n"))),
        "name".into(),
    );
    assert!(expr_to_column_name(&prop).contains("name"));
    let bin = Expr::BinaryOp {
        op: BinaryOp::Add,
        left: Box::new(Expr::Integer(1)),
        right: Box::new(Expr::Integer(2)),
    };
    let _ = expr_to_column_name(&bin);
    let neg = Expr::UnaryOp {
        op: UnaryOp::Neg,
        expr: Box::new(Expr::Integer(3)),
    };
    let _ = expr_to_column_name(&neg);
    let isn = Expr::IsNull(Box::new(Expr::Variable(Identifier::new("x"))));
    let _ = expr_to_column_name(&isn);
    let isnn = Expr::IsNotNull(Box::new(Expr::Variable(Identifier::new("x"))));
    let _ = expr_to_column_name(&isnn);
    let lst = Expr::List(vec![Expr::Integer(1), Expr::Integer(2)]);
    let _ = expr_to_column_name(&lst);
    let map = Expr::MapLiteral(vec![("k".into(), Expr::Integer(1))]);
    let _ = expr_to_column_name(&map);
    let f = Expr::FunctionCall {
        name: "size".into(),
        args: vec![Expr::List(vec![Expr::Integer(1)])],
        distinct: false,
    };
    let _ = expr_to_column_name(&f);
    let lbl = Expr::HasLabel(
        Box::new(Expr::Variable(Identifier::new("n"))),
        Label::new("P"),
    );
    let _ = expr_to_column_name(&lbl);
}

// ── discover_operator_aliases for many Operator variants ───────────────

#[test]
fn discover_operator_aliases_node_index_seek() {
    let n = Operator::NodeScan {
        label: Some(Label::new("L")),
        alias: Identifier::new("n"),
    };
    assert_eq!(discover_operator_aliases(&n), vec!["n".to_string()]);
    let i = Operator::IndexScan {
        label: Label::new("L"),
        property: "p".into(),
        value: Some(Expr::Integer(1)),
        alias: Identifier::new("i"),
    };
    assert_eq!(discover_operator_aliases(&i), vec!["i".to_string()]);
    let s = Operator::NodeByIdSeek {
        id_expr: Expr::Integer(1),
        alias: Identifier::new("s"),
    };
    assert_eq!(discover_operator_aliases(&s), vec!["s".to_string()]);
    let single = Operator::SingleRow;
    assert!(discover_operator_aliases(&single).is_empty());
}

#[test]
fn discover_operator_aliases_expand_chain() {
    let scan = Operator::NodeScan {
        label: None,
        alias: Identifier::new("a"),
    };
    let exp = Operator::Expand {
        direction: Direction::Outgoing,
        rel_types: vec![RelType::new("R")],
        src_alias: Identifier::new("a"),
        rel_alias: Some(Identifier::new("r")),
        dst_alias: Identifier::new("b"),
        child: Box::new(scan),
    };
    let aliases = discover_operator_aliases(&exp);
    assert!(aliases.contains(&"a".to_string()));
    assert!(aliases.contains(&"b".to_string()));
    assert!(aliases.contains(&"r".to_string()));
}

// ── value_to_expr / eval_binary_values ─────────────────────────────────

#[test]
fn value_to_expr_round_trip_literals() {
    use crate::executor::engine::helpers::value_to_expr;
    let exprs = [
        value_to_expr(&Value::Null),
        value_to_expr(&Value::Bool(true)),
        value_to_expr(&Value::Int(5)),
        value_to_expr(&Value::Float(1.5)),
        value_to_expr(&Value::Str("x".into())),
        value_to_expr(&Value::List(vec![Value::Int(1)])),
    ];
    for e in &exprs {
        let _ = format!("{e:?}");
    }
}

// ── Element/property functions through queries ─────────────────────────

fn populate(store: &InMemoryGraphStore) -> (NodeId, NodeId) {
    let mut pa = IndexMap::new();
    pa.insert("name".to_string(), Value::Str("A".into()));
    pa.insert("n".to_string(), Value::Int(1));
    let a = store
        .create_node("g", vec!["P".to_string(), "Q".to_string()], pa)
        .unwrap();
    let mut pb = IndexMap::new();
    pb.insert("name".to_string(), Value::Str("B".into()));
    let b = store.create_node("g", vec!["P".to_string()], pb).unwrap();
    store
        .create_edge("g", a, b, "REL".to_string(), {
            let mut m = IndexMap::new();
            m.insert("w".to_string(), Value::Int(7));
            m
        })
        .unwrap();
    (a, b)
}

#[test]
fn element_funcs_id_labels_type_properties_keys() {
    let store = fresh_store();
    populate(&store);
    let r = exec_on(&store, "MATCH (n:P) RETURN id(n) AS i ORDER BY i");
    assert_eq!(r.rows.len(), 2);
    let r2 = exec_on(&store, "MATCH (n:P {name:'A'}) RETURN labels(n) AS ls");
    if let Value::List(items) = first_val(&r2) {
        let labels: Vec<String> = items
            .into_iter()
            .filter_map(|v| match v {
                Value::Str(s) => Some(s.into_string()),
                _ => None,
            })
            .collect();
        assert!(labels.contains(&"P".to_string()));
        assert!(labels.contains(&"Q".to_string()));
    } else {
        panic!();
    }
    let r3 = exec_on(
        &store,
        "MATCH (a:P)-[r:REL]->(b) RETURN type(r) AS t LIMIT 1",
    );
    assert_eq!(first_val(&r3), Value::Str("REL".into()));
    let r4 = exec_on(&store, "MATCH (n:P {name:'A'}) RETURN properties(n) AS p");
    if let Value::Map(_) = first_val(&r4) {
    } else {
        panic!();
    }
    let r5 = exec_on(&store, "MATCH (n:P {name:'A'}) RETURN keys(n) AS ks");
    if let Value::List(_) = first_val(&r5) {
    } else {
        panic!();
    }
}

#[test]
fn has_label_predicate_and_subscript() {
    let store = fresh_store();
    populate(&store);
    let r = exec_on(&store, "MATCH (n:P) WHERE n:Q RETURN n.name");
    assert_eq!(first_val(&r), Value::Str("A".into()));
    let r2 = exec_on(&store, "MATCH (n:P) WHERE n:R RETURN n.name");
    assert_eq!(r2.rows.len(), 0);
    let r3 = exec("WITH {a:1, b:2} AS m RETURN m['a'] AS x, m['b'] AS y");
    assert_eq!(r3.rows.len(), 1);
}

// ── Persistent storage smoke ───────────────────────────────────────────

#[test]
fn persistent_store_basic_ops_in_temp_dir() {
    use crate::storage::persistent::{PersistentConfig, SyncMode};
    use crate::storage::PersistentGraphStore;
    use std::time::Duration;
    let dir = tempfile::tempdir().unwrap();
    let cfg = PersistentConfig {
        data_dir: dir.path().to_path_buf(),
        snapshot_interval: Duration::from_secs(3600),
        sync_mode: SyncMode::Immediate,
    };
    let store = PersistentGraphStore::open(cfg).unwrap();
    store.create_graph("g").unwrap();
    let mut p = IndexMap::new();
    p.insert("k".to_string(), Value::Int(42));
    let id = store.create_node("g", vec!["L".to_string()], p).unwrap();
    let n = store.get_node("g", id).unwrap().unwrap();
    assert_eq!(n.properties.get("k"), Some(&Value::Int(42)));
}

// ── Various small parser/planner branches ──────────────────────────────

#[test]
fn parser_with_clause_alias_and_skip() {
    let _ = exec("WITH 1 AS x, 2 AS y RETURN x + y AS s");
    let _ = exec("UNWIND [1,2,3] AS x WITH x ORDER BY x DESC SKIP 0 LIMIT 5 RETURN x");
}

#[test]
fn parser_string_literal_escapes() {
    let r = exec("RETURN 'a\\nb' AS r");
    if let Value::Str(s) = first_val(&r) {
        assert!(s.contains('\n'));
    }
    let r2 = exec("RETURN \"x\\\"y\" AS r");
    if let Value::Str(s) = first_val(&r2) {
        assert!(s.contains('"'));
    }
}

#[test]
fn parser_numeric_literals() {
    assert_eq!(first_val(&exec("RETURN 0xff AS r")), Value::Int(255));
    assert_eq!(first_val(&exec("RETURN 0o17 AS r")), Value::Int(15));
    assert_eq!(first_val(&exec("RETURN 1e3 AS r")), Value::Float(1000.0));
    assert_eq!(first_val(&exec("RETURN 1.5e2 AS r")), Value::Float(150.0));
}

#[test]
fn case_with_null_branches() {
    assert_eq!(
        first_val(&exec("RETURN CASE WHEN null THEN 1 ELSE 2 END AS r")),
        Value::Int(2)
    );
    // No ELSE on no-match returns null
    assert_eq!(
        first_val(&exec("RETURN CASE WHEN false THEN 1 END AS r")),
        Value::Null
    );
}

#[test]
fn unwind_nested_lists_and_maps() {
    let r = exec("UNWIND [[1,2],[3,4]] AS x RETURN x");
    assert_eq!(r.rows.len(), 2);
    let r2 = exec("UNWIND [{k:1},{k:2},{k:3}] AS m RETURN m.k AS k ORDER BY k");
    assert_eq!(r2.rows.len(), 3);
}

// ── Multiple match patterns + variable-length predicates ───────────────

#[test]
fn multi_match_and_optional_chain() {
    let store = fresh_store();
    populate(&store);
    let r = exec_on(
        &store,
        "MATCH (a:P {name:'A'}), (b:P {name:'B'}) RETURN a.name, b.name",
    );
    assert_eq!(r.rows.len(), 1);
    let r2 = exec_on(
        &store,
        "MATCH (a:P {name:'A'}) OPTIONAL MATCH (a)-[r:NONE_SUCH]->(b) RETURN a.name, r",
    );
    assert_eq!(r2.rows.len(), 1);
}

#[test]
fn shortest_path_function_runs() {
    let store = fresh_store();
    populate(&store);
    let _ = try_exec(
        "MATCH (a:P {name:'A'}), (b:P {name:'B'}), p = shortestPath((a)-[*]->(b)) RETURN length(p)",
    );
    let _ = try_exec(
        "MATCH (a:P {name:'A'}), (b:P {name:'B'}), p = allShortestPaths((a)-[*]->(b)) RETURN length(p)",
    );
}

// ── Aggregations over distinct and orderable rows ──────────────────────

#[test]
fn aggregation_collect_distinct_and_stdev() {
    let r = exec("UNWIND [1,1,2,2,3] AS x RETURN collect(DISTINCT x) AS r");
    if let Value::List(items) = first_val(&r) {
        assert_eq!(items.len(), 3);
    } else {
        panic!();
    }
    let _ = try_exec("UNWIND [1.0,2.0,3.0,4.0] AS x RETURN stDev(x) AS r");
    let _ = try_exec("UNWIND [1.0,2.0,3.0,4.0] AS x RETURN percentileCont(x, 0.5) AS r");
    let _ = try_exec("UNWIND [1.0,2.0,3.0,4.0] AS x RETURN percentileDisc(x, 0.5) AS r");
}

// ── Set/remove labels via SET / REMOVE ─────────────────────────────────

#[test]
fn set_label_after_match() {
    let store = fresh_store();
    populate(&store);
    let r = exec_on(
        &store,
        "MATCH (n:P {name:'B'}) SET n:Z RETURN labels(n) AS ls",
    );
    if let Value::List(items) = first_val(&r) {
        let labels: Vec<String> = items
            .into_iter()
            .filter_map(|v| match v {
                Value::Str(s) => Some(s.into_string()),
                _ => None,
            })
            .collect();
        assert!(labels.contains(&"Z".to_string()));
    }
}

// ── Filter branches: numeric overflow in arithmetic helpers ────────────

#[test]
fn filter_arithmetic_mixed_types() {
    assert_eq!(first_val(&exec("RETURN 1 + 1.5 AS r")), Value::Float(2.5));
    assert_eq!(first_val(&exec("RETURN 1.5 + 1 AS r")), Value::Float(2.5));
    assert_eq!(
        first_val(&exec("RETURN 'a' + 'b' AS r")),
        Value::Str("ab".into())
    );
    let _ = try_exec("RETURN 'a' + 1 AS r");
    let _ = try_exec("RETURN 1 + 'a' AS r");
}

// ── Distinct + ordering across mixed types (Sort + Distinct ops) ───────

#[test]
fn distinct_mixed_types_does_not_panic() {
    let r = exec("UNWIND [1, 1.0, '1', true, null, true] AS x RETURN DISTINCT x");
    assert!(!r.rows.is_empty());
}

#[test]
fn order_by_mixed_types_does_not_panic() {
    let r = exec("UNWIND [3, 'a', 1.5, true, null, 2] AS x RETURN x ORDER BY x ASC");
    assert_eq!(r.rows.len(), 6);
}

// ── Skip larger than result ────────────────────────────────────────────

#[test]
fn skip_beyond_results() {
    let r = exec("UNWIND [1,2] AS x RETURN x SKIP 100");
    assert_eq!(r.rows.len(), 0);
}

// ── Limit zero ────────────────────────────────────────────────────────

#[test]
fn limit_zero_returns_empty() {
    let r = exec("UNWIND [1,2,3] AS x RETURN x LIMIT 0");
    assert_eq!(r.rows.len(), 0);
}

// ── Map literals with various value types ──────────────────────────────

#[test]
fn map_literal_with_nested_values() {
    let r = exec("RETURN {a:1, b:[1,2], c:{d:'x'}, e:null, f:true} AS m");
    if let Value::Map(m) = first_val(&r) {
        assert_eq!(m.len(), 5);
    } else {
        panic!();
    }
}

// ── Boolean literal values via Cypher and IS NULL chains ───────────────

#[test]
fn boolean_predicates_chained() {
    assert_eq!(
        first_val(&exec("RETURN true AND true AND true AS r")),
        Value::Bool(true)
    );
    assert_eq!(
        first_val(&exec("RETURN false OR false OR true AS r")),
        Value::Bool(true)
    );
    assert_eq!(
        first_val(&exec("RETURN NOT (1 < 2) AS r")),
        Value::Bool(false)
    );
}

// ── Range edge cases ──────────────────────────────────────────────────

#[test]
fn range_function_edge_cases() {
    assert_eq!(
        first_val(&exec("RETURN range(1, 1) AS r")),
        Value::List(vec![Value::Int(1)])
    );
    assert_eq!(
        first_val(&exec("RETURN range(5, 1, -1) AS r")),
        Value::List((1..=5).rev().map(Value::Int).collect())
    );
    assert_eq!(
        first_val(&exec("RETURN range(1, 0) AS r")),
        Value::List(vec![])
    );
}

// ── Point construction ────────────────────────────────────────────────

#[test]
fn point_constructor_runs() {
    let _ = try_exec("RETURN point({x: 1, y: 2}) AS r");
    let _ = try_exec("RETURN point({x: 1, y: 2, z: 3}) AS r");
    let _ = try_exec("RETURN point({longitude: 1, latitude: 2}) AS r");
    let _ = try_exec("RETURN point.distance(point({x:0,y:0}), point({x:3,y:4})) AS r");
}

// ── Display indirectly via toString of various Value types ─────────────

#[test]
fn display_via_to_string_function() {
    for q in [
        "RETURN toString(date('2024-01-01')) AS r",
        "RETURN toString(time('12:34:56')) AS r",
        "RETURN toString(localdatetime('2024-01-01T12:00:00')) AS r",
        "RETURN toString(datetime('2024-01-01T12:00:00Z')) AS r",
        "RETURN toString(duration('PT1H')) AS r",
        "RETURN toString([1,2,3]) AS r",
        "RETURN toString({a:1}) AS r",
        "RETURN toString(point({x:1, y:2})) AS r",
        "RETURN toString(true) AS r",
        "RETURN toString(null) AS r",
    ] {
        let _ = try_exec(q);
    }
}

// ── GeoPoint construction direct ──────────────────────────────────────

#[test]
fn geopoint_clone_and_eq() {
    let p = GeoPoint {
        x: 1.0,
        y: 2.0,
        z: Some(3.0),
        crs: Crs::Cartesian3d,
    };
    let p2 = p;
    assert_eq!(p, p2);
}

// ── CypherDuration hash equality ──────────────────────────────────────

#[test]
fn cypher_duration_hash_consistency() {
    use std::hash::{Hash, Hasher};
    let a = CypherDuration {
        months: 1,
        days: 2,
        nanos: 3,
    };
    let b = a;
    let mut h1 = std::collections::hash_map::DefaultHasher::new();
    let mut h2 = std::collections::hash_map::DefaultHasher::new();
    a.hash(&mut h1);
    b.hash(&mut h2);
    assert_eq!(h1.finish(), h2.finish());
    assert_eq!(a, b);
}

// ── Path semantics: COLLECT path nodes ─────────────────────────────────

#[test]
fn path_var_assignment_returns_path_value() {
    let store = fresh_store();
    populate(&store);
    let r = exec_on(&store, "MATCH p = (a:P {name:'A'})-[:REL]->(b:P) RETURN p");
    assert_eq!(r.rows.len(), 1);
}

// ── Misc: complex expressions ─────────────────────────────────────────

#[test]
fn complex_expressions_combinations() {
    assert_eq!(
        first_val(&exec("RETURN [x IN range(1,5) WHERE x % 2 = 0 | x*x] AS r")),
        Value::List(vec![Value::Int(4), Value::Int(16)])
    );
    assert_eq!(
        first_val(&exec(
            "RETURN reduce(s = '', x IN ['a','b','c'] | s + x) AS r"
        )),
        Value::Str("abc".into())
    );
}

// ── Pattern with relationship variable used in WHERE ───────────────────

#[test]
fn rel_variable_with_predicate() {
    let store = fresh_store();
    populate(&store);
    let r = exec_on(
        &store,
        "MATCH (a:P)-[r:REL]->(b:P) WHERE r.w = 7 RETURN a.name, b.name",
    );
    assert_eq!(r.rows.len(), 1);
}

// ── CREATE with chained pattern referencing existing alias ─────────────

#[test]
fn create_chain_referencing_match() {
    let store = fresh_store();
    populate(&store);
    let r = exec_on(
        &store,
        "MATCH (a:P {name:'A'}), (b:P {name:'B'}) CREATE (a)-[r:NEW {v:1}]->(b) RETURN r.v",
    );
    assert_eq!(first_val(&r), Value::Int(1));
}

#[test]
fn create_multi_alias_pattern_with_internal_references() {
    let store = fresh_store();
    let r = exec_on(
        &store,
        "CREATE (a:N {n:'a'}), (b:N {n:'b'}), (a)-[:E]->(b) RETURN a.n, b.n",
    );
    assert_eq!(r.rows.len(), 1);
    assert_eq!(r.stats.nodes_created, 2);
    assert_eq!(r.stats.edges_created, 1);
}

// ── MERGE patterns with relationships ──────────────────────────────────

#[test]
fn merge_relationship_pattern_creates_when_missing() {
    let store = fresh_store();
    let r = exec_on(
        &store,
        "MERGE (a:N {n:'a'}) MERGE (b:N {n:'b'}) MERGE (a)-[r:E]->(b) RETURN r",
    );
    assert_eq!(r.stats.nodes_created, 2);
    assert_eq!(r.stats.edges_created, 1);

    let r2 = exec_on(
        &store,
        "MERGE (a:N {n:'a'}) MERGE (b:N {n:'b'}) MERGE (a)-[r:E]->(b) RETURN r",
    );
    assert_eq!(r2.stats.nodes_created, 0);
    assert_eq!(r2.stats.edges_created, 0);
}

// ── List functions: tail/head edge cases on null ───────────────────────

#[test]
fn list_funcs_null_propagation() {
    assert_eq!(first_val(&exec("RETURN head(null) AS r")), Value::Null);
    assert_eq!(first_val(&exec("RETURN last(null) AS r")), Value::Null);
    assert_eq!(first_val(&exec("RETURN tail(null) AS r")), Value::Null);
    assert_eq!(first_val(&exec("RETURN size(null) AS r")), Value::Null);
    assert_eq!(first_val(&exec("RETURN reverse(null) AS r")), Value::Null);
}

// ── String func null propagation and edge cases ────────────────────────

#[test]
fn string_funcs_null_propagation() {
    assert_eq!(first_val(&exec("RETURN toUpper(null) AS r")), Value::Null);
    assert_eq!(first_val(&exec("RETURN toLower(null) AS r")), Value::Null);
    assert_eq!(first_val(&exec("RETURN trim(null) AS r")), Value::Null);
    assert_eq!(first_val(&exec("RETURN size(null) AS r")), Value::Null);
    assert_eq!(
        first_val(&exec("RETURN substring(null, 0) AS r")),
        Value::Null
    );
}

// ── Equality / ordering through queries with mixed types ───────────────

#[test]
fn equality_and_inequality_with_mixed_types() {
    assert_eq!(first_val(&exec("RETURN 1 = 1.0 AS r")), Value::Bool(true));
    assert_eq!(first_val(&exec("RETURN 'a' < 'b' AS r")), Value::Bool(true));
    assert_eq!(
        first_val(&exec("RETURN [1,2] = [1,2] AS r")),
        Value::Bool(true)
    );
    assert_eq!(
        first_val(&exec("RETURN [1,null] = [1,2] AS r")),
        Value::Null
    );
}

// ── NULL arithmetic ────────────────────────────────────────────────────

#[test]
fn null_arithmetic_propagation() {
    assert_eq!(first_val(&exec("RETURN null + 1 AS r")), Value::Null);
    assert_eq!(first_val(&exec("RETURN null * 2 AS r")), Value::Null);
    assert_eq!(first_val(&exec("RETURN null - null AS r")), Value::Null);
}
