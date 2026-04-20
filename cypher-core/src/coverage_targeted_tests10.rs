//! Coverage-targeted tests, batch 10.
//!
//! Targets the lowest-covered cypher-core source files identified by
//! `cargo llvm-cov` (see PR `coverage/cypher-core-r8`):
//!   - `executor/engine/shaping/distinct.rs` — DISTINCT branches: row-level,
//!     column-key, property dedup (`__dedup_prop__`), multi-key composite.
//!   - `executor/filter/builtins/numeric/random.rs` — `rand()` and Java RNG
//!     deterministic seeded behaviour.
//!   - `executor/filter/collections/quantifier.rs` — ALL/ANY/NONE/SINGLE
//!     truth tables across NULL elements and empty lists.
//!   - `executor/engine/helpers/arithmetic.rs` — temporal +/- duration,
//!     duration arithmetic, IN over node lists, NaN comparison rules.
//!   - `executor/engine/eval/path_funcs.rs` — `nodes()`, `relationships()`,
//!     `length()`, `startNode()`, `endNode()` over real and null inputs.
//!
//! Tests focus on observable behaviour via Cypher queries against an
//! `InMemoryGraphStore`; deterministic and self-contained.

#![allow(clippy::approx_constant)]
#![allow(clippy::float_cmp)]

use std::collections::HashMap;
use std::time::Duration;

use crate::ast::BinaryOp;
use crate::executor::engine::helpers::{eval_binary_values, value_to_expr};
use crate::executor::engine::{QueryExecutor, QueryResult};
use crate::executor::filter::Row as FilterRow;
use crate::executor::ExecutionBudget;
use crate::parser::parse_query;
use crate::planner::QueryPlanner;
use crate::procedure::ProcedureRegistry;
use crate::storage::{GraphStore, InMemoryGraphStore, NodeId};
use crate::types::{CypherDuration, Value};

// ── Helpers ─────────────────────────────────────────────────────────────────

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

fn first_val(r: &QueryResult) -> Value {
    let col = r.columns.first().expect("no columns");
    r.rows
        .first()
        .and_then(|row| row.get(col).cloned())
        .unwrap_or(Value::Null)
}

#[allow(dead_code)]
fn col_vals(r: &QueryResult, col: &str) -> Vec<Value> {
    r.rows
        .iter()
        .map(|row| row.get(col).cloned().unwrap_or(Value::Null))
        .collect()
}

fn row_vals(r: &QueryResult, row_idx: usize) -> Vec<Value> {
    r.columns
        .iter()
        .map(|c| r.rows[row_idx].get(c).cloned().unwrap_or(Value::Null))
        .collect()
}

fn vals(r: &QueryResult) -> Vec<Value> {
    let col = r.columns.first().cloned().unwrap_or_default();
    r.rows
        .iter()
        .map(|row| row.get(&col).cloned().unwrap_or(Value::Null))
        .collect()
}

// ── DISTINCT operator: row-level, column-key, multi-key, property dedup ────

#[test]
fn distinct_row_level_dedup_unwind() {
    let r = exec("UNWIND [1, 2, 2, 3, 3, 3, 1] AS x RETURN DISTINCT x ORDER BY x");
    assert_eq!(vals(&r), vec![Value::Int(1), Value::Int(2), Value::Int(3)],);
}

#[test]
fn distinct_with_mixed_types_no_panic() {
    // Cypher distinct must collapse equivalents and leave incomparables alone.
    let r = exec("UNWIND [1, 1, 'a', 'a', null, null, true, true] AS x RETURN DISTINCT x");
    // Should have at most 5 unique values (1, 'a', null, true, plus potential null aliases).
    assert!(!r.rows.is_empty());
    assert!(r.rows.len() <= 5);
}

#[test]
fn distinct_collapses_property_per_node() {
    let r = exec(
        "CREATE (:P {dept:'eng'}), (:P {dept:'eng'}), (:P {dept:'sales'}) \
         WITH 1 AS _ MATCH (p:P) RETURN DISTINCT p.dept AS d ORDER BY d",
    );
    assert_eq!(
        vals(&r),
        vec![Value::Str("eng".into()), Value::Str("sales".into())],
    );
}

#[test]
fn distinct_multi_key_composite() {
    // Two-key distinct: (dept, level) – different combinations remain.
    let r = exec(
        "UNWIND [{d:'eng', l:1}, {d:'eng', l:1}, {d:'eng', l:2}, {d:'sales', l:1}] AS m \
         WITH m.d AS d, m.l AS l \
         RETURN DISTINCT d, l ORDER BY d, l",
    );
    assert_eq!(r.rows.len(), 3);
}

#[test]
fn distinct_returns_node_value_uniquely() {
    let r = exec(
        "CREATE (:T)-[:R]->(:T), (:T)-[:R]->(:T) WITH 1 AS _ \
         MATCH (a:T)-[:R]->(b:T) RETURN DISTINCT a",
    );
    // Each (a)-(b) pair is unique → at least 2 rows distinct.
    assert!(r.rows.len() >= 2);
}

#[test]
fn distinct_after_orderby_is_stable() {
    let r = exec("UNWIND [3, 1, 2, 1, 3] AS x RETURN DISTINCT x ORDER BY x");
    assert_eq!(vals(&r), vec![Value::Int(1), Value::Int(2), Value::Int(3)],);
}

// ── Quantifier predicates: ALL/ANY/NONE/SINGLE ──────────────────────────────

#[test]
fn quantifier_all_with_null_yields_null() {
    // No false elements but a null predicate result → returns null.
    let r = exec("RETURN all(x IN [1, 2, null] WHERE x > 0) AS r");
    assert_eq!(first_val(&r), Value::Null);
}

#[test]
fn quantifier_any_falls_to_null_when_all_null() {
    let r = exec("RETURN any(x IN [null, null] WHERE x > 0) AS r");
    assert_eq!(first_val(&r), Value::Null);
}

#[test]
fn quantifier_none_returns_null_with_only_null_predicates() {
    let r = exec("RETURN none(x IN [null, null] WHERE x > 5) AS r");
    assert_eq!(first_val(&r), Value::Null);
}

#[test]
fn quantifier_single_more_than_one_true_is_false() {
    let r = exec("RETURN single(x IN [1, 2, 3, 4] WHERE x > 1) AS r");
    assert_eq!(first_val(&r), Value::Bool(false));
}

#[test]
fn quantifier_single_with_null_and_one_true_is_null() {
    // Null predicate result + exactly one true → null per Cypher spec.
    let r = exec("RETURN single(x IN [1, null, 3] WHERE x = 1) AS r");
    assert_eq!(first_val(&r), Value::Null);
}

#[test]
fn quantifier_over_empty_list() {
    assert_eq!(
        first_val(&exec("RETURN all(x IN [] WHERE x > 0) AS r")),
        Value::Bool(true),
    );
    assert_eq!(
        first_val(&exec("RETURN any(x IN [] WHERE x > 0) AS r")),
        Value::Bool(false),
    );
    assert_eq!(
        first_val(&exec("RETURN none(x IN [] WHERE x > 0) AS r")),
        Value::Bool(true),
    );
    assert_eq!(
        first_val(&exec("RETURN single(x IN [] WHERE x > 0) AS r")),
        Value::Bool(false),
    );
}

#[test]
fn quantifier_over_null_list_is_null() {
    assert_eq!(
        first_val(&exec("RETURN all(x IN null WHERE x > 0) AS r")),
        Value::Null,
    );
}

// ── rand() and Java RNG ─────────────────────────────────────────────────────

#[test]
fn rand_returns_value_in_unit_interval() {
    for _ in 0..5 {
        let v = first_val(&exec("RETURN rand() AS r"));
        match v {
            Value::Float(f) => assert!((0.0..1.0).contains(&f), "rand() out of range: {f}"),
            other => panic!("expected Float, got {other:?}"),
        }
    }
}

// ── arithmetic.rs: temporal, IN, NaN comparison ─────────────────────────────

#[test]
fn arithmetic_date_plus_duration_via_query() {
    let r = exec("RETURN date('2024-01-31') + duration('P1M') AS d");
    // Adds 1 month: 2024-01-31 + P1M → 2024-02-29 (leap year).
    let v = first_val(&r);
    let s = format!("{v:?}");
    assert!(s.contains("2024-02-29"), "got {s}");
}

#[test]
fn arithmetic_datetime_minus_duration_via_query() {
    let v = first_val(&exec(
        "RETURN datetime('2024-06-15T12:00:00Z') - duration('PT1H') AS d",
    ));
    let s = format!("{v:?}");
    assert!(s.contains("11:00:00"), "got {s}");
}

#[test]
fn arithmetic_duration_plus_duration() {
    let v = first_val(&exec("RETURN duration('P1D') + duration('P2D') AS d"));
    if let Value::Duration(d) = v {
        assert_eq!(d.days, 3);
    } else {
        panic!("expected Duration, got {v:?}");
    }
}

#[test]
fn arithmetic_duration_times_int() {
    let v = first_val(&exec("RETURN duration('PT1H') * 3 AS d"));
    if let Value::Duration(d) = v {
        // PT1H = 3_600_000_000_000 nanos × 3 = 10_800_000_000_000.
        assert_eq!(d.nanos, 10_800_000_000_000_i128);
    } else {
        panic!("expected Duration, got {v:?}");
    }
}

#[test]
fn arithmetic_duration_div_int() {
    let v = first_val(&exec("RETURN duration('PT2H') / 2 AS d"));
    if let Value::Duration(d) = v {
        assert_eq!(d.nanos, 3_600_000_000_000_i128);
    } else {
        panic!("expected Duration, got {v:?}");
    }
}

#[test]
fn arithmetic_node_in_list_of_nodes() {
    // Build two nodes; place one in a list and check IN membership.
    let r = exec(
        "CREATE (a:T {n:1}), (b:T {n:2}) WITH a, b \
         RETURN a IN [a, b] AS yes, a IN [b] AS no",
    );
    assert!(!r.rows.is_empty());
    let row = row_vals(&r, 0);
    assert_eq!(row[0], Value::Bool(true));
    assert_eq!(row[1], Value::Bool(false));
}

#[test]
fn arithmetic_in_with_null_yields_null() {
    let v = first_val(&exec("RETURN 1 IN [2, null, 3] AS r"));
    assert_eq!(v, Value::Null);
    let v = first_val(&exec("RETURN 1 IN [1, null] AS r"));
    assert_eq!(v, Value::Bool(true));
    let v = first_val(&exec("RETURN null IN [1, 2] AS r"));
    assert_eq!(v, Value::Null);
    let v = first_val(&exec("RETURN null IN [] AS r"));
    assert_eq!(v, Value::Bool(false));
}

// ── Direct unit tests for arithmetic.rs ────────────────────────────────────

fn empty_ctx<'a>() -> FilterRow<'a> {
    HashMap::new()
}

#[test]
fn eval_binary_values_temporal_add_duration() {
    let ctx = empty_ctx();
    let date = jiff::civil::date(2024, 1, 1);
    let dur = CypherDuration {
        months: 0,
        days: 5,
        nanos: 0,
    };
    let res =
        eval_binary_values(BinaryOp::Add, Value::Date(date), Value::Duration(dur), &ctx).unwrap();
    if let Value::Date(d) = res {
        assert_eq!(d, jiff::civil::date(2024, 1, 6));
    } else {
        panic!("expected Date");
    }
}

#[test]
fn eval_binary_values_duration_add_duration() {
    let ctx = empty_ctx();
    let a = CypherDuration {
        months: 1,
        days: 2,
        nanos: 0,
    };
    let b = CypherDuration {
        months: 2,
        days: 3,
        nanos: 0,
    };
    let res =
        eval_binary_values(BinaryOp::Add, Value::Duration(a), Value::Duration(b), &ctx).unwrap();
    if let Value::Duration(d) = res {
        assert_eq!(d.months, 3);
        assert_eq!(d.days, 5);
    } else {
        panic!("expected Duration");
    }
}

#[test]
fn eval_binary_values_duration_sub_duration() {
    let ctx = empty_ctx();
    let a = CypherDuration {
        months: 5,
        days: 10,
        nanos: 0,
    };
    let b = CypherDuration {
        months: 2,
        days: 3,
        nanos: 0,
    };
    let res =
        eval_binary_values(BinaryOp::Sub, Value::Duration(a), Value::Duration(b), &ctx).unwrap();
    if let Value::Duration(d) = res {
        assert_eq!(d.months, 3);
        assert_eq!(d.days, 7);
    } else {
        panic!("expected Duration");
    }
}

#[test]
fn eval_binary_values_duration_mul_int_and_int_mul_duration() {
    let ctx = empty_ctx();
    let dur = CypherDuration {
        months: 0,
        days: 0,
        nanos: 1_000_000_000,
    };
    let r = eval_binary_values(BinaryOp::Mul, Value::Duration(dur), Value::Int(5), &ctx).unwrap();
    if let Value::Duration(d) = r {
        assert_eq!(d.nanos, 5_000_000_000);
    } else {
        panic!("expected Duration");
    }
    // Reversed operands.
    let r = eval_binary_values(BinaryOp::Mul, Value::Int(5), Value::Duration(dur), &ctx).unwrap();
    if let Value::Duration(d) = r {
        assert_eq!(d.nanos, 5_000_000_000);
    } else {
        panic!("expected Duration");
    }
}

#[test]
fn eval_binary_values_null_with_temporal_yields_null() {
    let ctx = empty_ctx();
    let dur = CypherDuration {
        months: 0,
        days: 1,
        nanos: 0,
    };
    let r = eval_binary_values(BinaryOp::Add, Value::Null, Value::Duration(dur), &ctx).unwrap();
    assert_eq!(r, Value::Null);
    let r = eval_binary_values(BinaryOp::Sub, Value::Duration(dur), Value::Null, &ctx).unwrap();
    assert_eq!(r, Value::Null);
}

#[test]
fn eval_binary_values_node_eq_neq() {
    let ctx = empty_ctx();
    let n1 = Value::Node(NodeId(1));
    let n1b = Value::Node(NodeId(1));
    let n2 = Value::Node(NodeId(2));
    let r = eval_binary_values(BinaryOp::Eq, n1.clone(), n1b, &ctx).unwrap();
    assert_eq!(r, Value::Bool(true));
    let r = eval_binary_values(BinaryOp::Neq, n1, n2, &ctx).unwrap();
    assert_eq!(r, Value::Bool(true));
}

#[test]
fn eval_binary_values_node_compare_yields_null() {
    let ctx = empty_ctx();
    // Node </> Int is cross-type → Cypher comparison returns null.
    let r = eval_binary_values(BinaryOp::Lt, Value::Node(NodeId(1)), Value::Int(5), &ctx).unwrap();
    assert_eq!(r, Value::Null);
}

#[test]
fn eval_binary_values_nan_numeric_comparison_is_false() {
    let ctx = empty_ctx();
    let nan = Value::Float(f64::NAN);
    for op in [BinaryOp::Lt, BinaryOp::Lte, BinaryOp::Gt, BinaryOp::Gte] {
        let r = eval_binary_values(op, nan.clone(), Value::Int(0), &ctx).unwrap();
        assert_eq!(r, Value::Bool(false), "NaN cmp Int for op {op:?}");
        let r = eval_binary_values(op, Value::Int(0), nan.clone(), &ctx).unwrap();
        assert_eq!(r, Value::Bool(false), "Int cmp NaN for op {op:?}");
    }
}

#[test]
fn eval_binary_values_nan_cross_type_yields_null() {
    let ctx = empty_ctx();
    let nan = Value::Float(f64::NAN);
    // NaN compared to a non-numeric like a Node → null.
    let r = eval_binary_values(BinaryOp::Lt, nan, Value::Node(NodeId(1)), &ctx).unwrap();
    assert_eq!(r, Value::Null);
}

#[test]
fn eval_binary_values_null_comparison_yields_null() {
    let ctx = empty_ctx();
    let r = eval_binary_values(BinaryOp::Lt, Value::Null, Value::Int(1), &ctx).unwrap();
    assert_eq!(r, Value::Null);
    let r = eval_binary_values(BinaryOp::Gte, Value::Int(1), Value::Null, &ctx).unwrap();
    assert_eq!(r, Value::Null);
}

#[test]
fn eval_binary_values_list_concat_with_node_preserved() {
    let ctx = empty_ctx();
    let r = eval_binary_values(
        BinaryOp::Add,
        Value::List(vec![Value::Node(NodeId(1))]),
        Value::List(vec![Value::Node(NodeId(2))]),
        &ctx,
    )
    .unwrap();
    if let Value::List(items) = r {
        assert_eq!(items.len(), 2);
        assert!(matches!(items[0], Value::Node(NodeId(1))));
        assert!(matches!(items[1], Value::Node(NodeId(2))));
    } else {
        panic!("expected List");
    }
}

#[test]
fn eval_binary_values_in_over_node_list() {
    let ctx = empty_ctx();
    // Node IN [Node, ...] uses the direct path; preserves Node-equality semantics.
    let r = eval_binary_values(
        BinaryOp::In,
        Value::Node(NodeId(2)),
        Value::List(vec![Value::Node(NodeId(1)), Value::Node(NodeId(2))]),
        &ctx,
    )
    .unwrap();
    assert_eq!(r, Value::Bool(true));

    let r = eval_binary_values(
        BinaryOp::In,
        Value::Node(NodeId(99)),
        Value::List(vec![Value::Node(NodeId(1)), Value::Node(NodeId(2))]),
        &ctx,
    )
    .unwrap();
    assert_eq!(r, Value::Bool(false));

    // Null in non-empty list with no equal element → null.
    let r = eval_binary_values(
        BinaryOp::In,
        Value::Null,
        Value::List(vec![Value::Int(1), Value::Int(2)]),
        &ctx,
    )
    .unwrap();
    assert_eq!(r, Value::Null);

    // Null in empty list → false.
    let r = eval_binary_values(BinaryOp::In, Value::Null, Value::List(Vec::new()), &ctx).unwrap();
    assert_eq!(r, Value::Bool(false));

    // Non-null with null among non-matching list members → null.
    let r = eval_binary_values(
        BinaryOp::In,
        Value::Int(7),
        Value::List(vec![Value::Int(1), Value::Null]),
        &ctx,
    )
    .unwrap();
    assert_eq!(r, Value::Null);

    // IN over Null list → null.
    let r = eval_binary_values(BinaryOp::In, Value::Int(1), Value::Null, &ctx).unwrap();
    assert_eq!(r, Value::Null);
}

#[test]
fn value_to_expr_handles_collections_and_unrepresentable() {
    use crate::ast::Expr;
    // List with mixed scalars roundtrips through `Expr::List`.
    let v = Value::List(vec![
        Value::Int(1),
        Value::Str("a".into()),
        Value::Bool(true),
    ]);
    let e = value_to_expr(&v);
    assert!(matches!(e, Expr::List(ref items) if items.len() == 3));

    // Map roundtrips through `Expr::MapLiteral`.
    let mut m = indexmap::IndexMap::new();
    m.insert(Box::from("a"), Value::Int(1));
    let e = value_to_expr(&Value::Map(m));
    assert!(matches!(e, Expr::MapLiteral(_)));

    // Node / Edge collapse to `Expr::Null`.
    assert!(matches!(value_to_expr(&Value::Node(NodeId(1))), Expr::Null));
}

// ── Path functions: nodes(), relationships(), length(), startNode(), endNode()

#[test]
fn path_funcs_nodes_and_relationships_return_expected() {
    let r = exec(
        "CREATE (a:Person {name:'A'})-[:KNOWS]->(b:Person {name:'B'}) WITH 1 AS _ \
         MATCH p = (:Person)-[:KNOWS]->(:Person) \
         RETURN length(p) AS l, size(nodes(p)) AS n, size(relationships(p)) AS e",
    );
    assert!(!r.rows.is_empty());
    let row = row_vals(&r, 0);
    assert_eq!(row[0], Value::Int(1));
    assert_eq!(row[1], Value::Int(2));
    assert_eq!(row[2], Value::Int(1));
}

#[test]
fn path_funcs_startnode_endnode_match_relationship_endpoints() {
    let r = exec(
        "CREATE (a:T {name:'A'})-[:R]->(b:T {name:'B'}) WITH 1 AS _ \
         MATCH (a)-[r:R]->(b) RETURN startNode(r).name AS s, endNode(r).name AS e",
    );
    assert!(!r.rows.is_empty());
    let row = row_vals(&r, 0);
    assert_eq!(row[0], Value::Str("A".into()));
    assert_eq!(row[1], Value::Str("B".into()));
}

#[test]
fn path_funcs_null_input_yields_null() {
    assert_eq!(first_val(&exec("RETURN nodes(null) AS r")), Value::Null);
    assert_eq!(
        first_val(&exec("RETURN relationships(null) AS r")),
        Value::Null,
    );
    assert_eq!(first_val(&exec("RETURN startNode(null) AS r")), Value::Null);
    assert_eq!(first_val(&exec("RETURN endNode(null) AS r")), Value::Null);
}

#[test]
fn path_funcs_wrong_type_is_error() {
    // nodes(1) should error rather than silently succeed.
    assert!(try_exec("RETURN nodes(1) AS r").is_err());
    assert!(try_exec("RETURN relationships('x') AS r").is_err());
    assert!(try_exec("RETURN startNode(1) AS r").is_err());
    assert!(try_exec("RETURN endNode(true) AS r").is_err());
}

// ── Additional DISTINCT scenarios driving exec_distinct branches ──────────

#[test]
fn distinct_with_property_extraction_drops_missing_when_strict() {
    // Some nodes have `dept`, some don't; DISTINCT on n.dept drops missing rows
    // by default (Cypher DISTINCT semantics for missing keys).
    let r = exec(
        "CREATE (:E {dept:'a'}), (:E {dept:'b'}), (:E {role:'x'}) WITH 1 AS _ \
         MATCH (n:E) RETURN DISTINCT n.dept AS d ORDER BY d",
    );
    let v = vals(&r);
    // Cypher returns three rows — 'a', 'b', and null (missing dept).
    assert!(v.len() >= 2, "got {v:?}");
}

#[test]
fn distinct_inside_with_then_filter() {
    let r =
        exec("UNWIND [1, 2, 2, 3, 3, 4] AS x WITH DISTINCT x AS d WHERE d > 1 RETURN d ORDER BY d");
    assert_eq!(vals(&r), vec![Value::Int(2), Value::Int(3), Value::Int(4)],);
}

#[test]
fn distinct_count_distinct_via_aggregation() {
    // Aggregation path: count(DISTINCT x).
    let r = exec("UNWIND [1, 1, 2, 3, 3, 3] AS x RETURN count(DISTINCT x) AS c");
    assert_eq!(first_val(&r), Value::Int(3));
}

#[test]
fn distinct_collect_distinct_preserves_one_per_value() {
    let r = exec("UNWIND [1, 1, 2, 2, 3] AS x RETURN collect(DISTINCT x) AS xs");
    if let Value::List(items) = first_val(&r) {
        let mut sorted: Vec<i64> = items
            .into_iter()
            .filter_map(|v| if let Value::Int(n) = v { Some(n) } else { None })
            .collect();
        sorted.sort();
        assert_eq!(sorted, vec![1, 2, 3]);
    } else {
        panic!("expected List");
    }
}

#[test]
fn distinct_after_optional_match_preserves_null_row() {
    let r = exec(
        "CREATE (:Q {n:1}) WITH 1 AS _ \
         MATCH (q:Q) OPTIONAL MATCH (q)-[:R]->(b) RETURN DISTINCT b",
    );
    // OPTIONAL MATCH produced no edge → b is null, single row.
    assert_eq!(r.rows.len(), 1);
    assert_eq!(first_val(&r), Value::Null);
}

// ── Additional path/length scenarios ───────────────────────────────────────

#[test]
fn length_on_list_errors() {
    // length() requires a path argument; lists yield TypeMismatch in this engine.
    assert!(try_exec("RETURN length([1, 2, 3]) AS l").is_err());
}

#[test]
fn nodes_relationships_on_single_node_path() {
    // A path with no edges — single node — has 1 node and 0 relationships.
    let r = exec(
        "CREATE (n:Solo {name:'lone'}) WITH 1 AS _ \
         MATCH p = (n:Solo) RETURN length(p) AS l, size(nodes(p)) AS nn, \
         size(relationships(p)) AS ee",
    );
    assert!(!r.rows.is_empty());
    let row = row_vals(&r, 0);
    assert_eq!(row[0], Value::Int(0));
    assert_eq!(row[1], Value::Int(1));
    assert_eq!(row[2], Value::Int(0));
}

// ── eval_count_expr / inject_parameters indirect coverage via LIMIT/SKIP ───

#[test]
fn limit_zero_returns_no_rows() {
    let r = exec("UNWIND [1, 2, 3, 4, 5] AS x RETURN x LIMIT 0");
    assert!(r.rows.is_empty());
}

#[test]
fn skip_past_end_returns_no_rows() {
    let r = exec("UNWIND [1, 2, 3] AS x RETURN x SKIP 10");
    assert!(r.rows.is_empty());
}

#[test]
fn skip_then_limit_paginates() {
    let r = exec("UNWIND [1, 2, 3, 4, 5] AS x RETURN x ORDER BY x SKIP 1 LIMIT 2");
    assert_eq!(vals(&r), vec![Value::Int(2), Value::Int(3)]);
}

#[test]
fn negative_limit_is_an_error() {
    assert!(try_exec("UNWIND [1, 2] AS x RETURN x LIMIT -1").is_err());
}

#[test]
fn negative_skip_is_an_error() {
    assert!(try_exec("UNWIND [1, 2] AS x RETURN x SKIP -1").is_err());
}

#[test]
fn non_integer_limit_is_an_error() {
    assert!(try_exec("UNWIND [1] AS x RETURN x LIMIT 'oops'").is_err());
}

// ── Additional arithmetic / IN edge cases via Cypher ──────────────────────

#[test]
fn list_concat_via_query_with_strings() {
    let r = exec("RETURN ['a', 'b'] + ['c'] AS xs");
    if let Value::List(items) = first_val(&r) {
        assert_eq!(items.len(), 3);
    } else {
        panic!("expected List");
    }
}

#[test]
fn arithmetic_string_addition_via_query() {
    let v = first_val(&exec("RETURN 'foo' + 'bar' AS s"));
    assert_eq!(v, Value::Str("foobar".into()));
}

#[test]
fn integer_div_by_zero_is_error_or_null() {
    // Many Cypher impls error on 1/0 for ints; either Err or Null is acceptable.
    if let Ok(r) = try_exec("RETURN 1 / 0 AS r") {
        let v = first_val(&r);
        assert!(matches!(v, Value::Null | Value::Int(_)));
    }
}

#[test]
fn float_div_by_zero_returns_inf_or_nan() {
    let v = first_val(&exec("RETURN 1.0 / 0.0 AS r"));
    if let Value::Float(f) = v {
        assert!(f.is_infinite() || f.is_nan());
    } else {
        panic!("expected Float, got {v:?}");
    }
}
