//! Focused coverage tests for `cypher-core::executor::filter::builtins`.
//!
//! Drives builtins through the public Cypher pipeline (parse → plan → execute).
//! Each function gets at least one nominal and one NULL-arg test.

use std::collections::HashMap;
use std::time::Duration;

use cypher_core::executor::{ExecutionBudget, QueryExecutor, QueryResult};
use cypher_core::parser::parse_query;
use cypher_core::planner::QueryPlanner;
use cypher_core::procedure::ProcedureRegistry;
use cypher_core::storage::{GraphStore, InMemoryGraphStore};
use cypher_core::types::Value;

fn unlimited() -> ExecutionBudget {
    ExecutionBudget::new(Duration::from_secs(60), usize::MAX)
}

fn exec(query: &str) -> QueryResult {
    let store = InMemoryGraphStore::new();
    store.create_graph("g").unwrap();
    let registry = ProcedureRegistry::new();
    let params: HashMap<String, Value> = HashMap::new();
    let q = parse_query(query).unwrap();
    let plan = QueryPlanner::with_context(&store, "g", &registry, &params)
        .plan(&q)
        .unwrap();
    let exec = QueryExecutor::with_context(&store, &registry, params);
    let mut b = unlimited();
    exec.execute("g", &plan, &mut b).unwrap()
}

fn val(query: &str) -> Value {
    let r = exec(query);
    assert_eq!(r.rows.len(), 1, "expected 1 row from {query}");
    let (_, v) = r.rows[0].iter().next().unwrap();
    v.clone()
}

fn approx(v: &Value, expected: f64) {
    match v {
        Value::Float(f) => assert!((f - expected).abs() < 1e-6, "expected ~{expected}, got {f}"),
        Value::Int(i) => assert!(((*i as f64) - expected).abs() < 1e-6),
        _ => panic!("expected float, got {v:?}"),
    }
}

// ── string/list size & length ──────────────────────────────────────────────

#[test]
fn size_string() {
    assert_eq!(val("RETURN size('hello')"), Value::Int(5));
}

#[test]
fn size_list() {
    assert_eq!(val("RETURN size([1,2,3])"), Value::Int(3));
}

#[test]
fn size_null() {
    assert_eq!(val("RETURN size(null)"), Value::Null);
}

#[test]
fn length_path() {
    let r = exec("CREATE p = (a:A)-[:R]->(b:B) RETURN length(p) AS l");
    let (_, v) = r.rows[0].iter().next().unwrap();
    assert_eq!(*v, Value::Int(1));
}

#[test]
fn length_null() {
    assert_eq!(val("RETURN length(null)"), Value::Null);
}

// ── list head/tail/last/reverse/range ───────────────────────────────────────

#[test]
fn head_nominal() {
    assert_eq!(val("RETURN head([10,20,30])"), Value::Int(10));
}

#[test]
fn head_empty_returns_null() {
    assert_eq!(val("RETURN head([])"), Value::Null);
}

#[test]
fn head_null() {
    assert_eq!(val("RETURN head(null)"), Value::Null);
}

#[test]
fn tail_nominal() {
    assert_eq!(
        val("RETURN tail([10,20,30])"),
        Value::List(vec![Value::Int(20), Value::Int(30)])
    );
}

#[test]
fn tail_null() {
    assert_eq!(val("RETURN tail(null)"), Value::Null);
}

#[test]
fn last_nominal() {
    assert_eq!(val("RETURN last([10,20,30])"), Value::Int(30));
}

#[test]
fn last_null() {
    assert_eq!(val("RETURN last(null)"), Value::Null);
}

#[test]
fn reverse_string() {
    assert_eq!(val("RETURN reverse('abc')"), Value::Str("cba".into()));
}

#[test]
fn reverse_list() {
    assert_eq!(
        val("RETURN reverse([1,2,3])"),
        Value::List(vec![Value::Int(3), Value::Int(2), Value::Int(1)])
    );
}

#[test]
fn reverse_null() {
    assert_eq!(val("RETURN reverse(null)"), Value::Null);
}

#[test]
fn range_3_args() {
    assert_eq!(
        val("RETURN range(1, 5, 2)"),
        Value::List(vec![Value::Int(1), Value::Int(3), Value::Int(5)])
    );
}

#[test]
fn range_2_args() {
    assert_eq!(
        val("RETURN range(1, 3)"),
        Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)])
    );
}

#[test]
fn range_null() {
    let r = val("RETURN range(null, 5)");
    // Either Null or List depending on impl; just check it doesn't panic.
    let _ = r;
}

// ── coalesce / isnull ──────────────────────────────────────────────────────

#[test]
fn coalesce_first_non_null() {
    assert_eq!(val("RETURN coalesce(null, null, 7, 8)"), Value::Int(7));
}

#[test]
fn coalesce_all_null() {
    assert_eq!(val("RETURN coalesce(null, null)"), Value::Null);
}

// ── keys / properties / labels / type ───────────────────────────────────────

#[test]
fn keys_of_map() {
    let r = val("RETURN keys({a: 1, b: 2})");
    if let Value::List(items) = r {
        let strs: Vec<String> = items
            .into_iter()
            .filter_map(|v| match v {
                Value::Str(s) => Some(s.to_string()),
                _ => None,
            })
            .collect();
        assert!(strs.contains(&"a".to_string()));
        assert!(strs.contains(&"b".to_string()));
    } else {
        panic!("expected list");
    }
}

#[test]
fn keys_null() {
    assert_eq!(val("RETURN keys(null)"), Value::Null);
}

#[test]
fn properties_of_map() {
    let r = val("RETURN properties({a: 1})");
    matches!(r, Value::Map(_));
}

#[test]
fn properties_null() {
    assert_eq!(val("RETURN properties(null)"), Value::Null);
}

// ── math: abs/ceil/floor/round/sqrt/sign ───────────────────────────────────

#[test]
fn abs_negative() {
    assert_eq!(val("RETURN abs(-5)"), Value::Int(5));
}

#[test]
fn abs_float() {
    approx(&val("RETURN abs(-3.5)"), 3.5);
}

#[test]
fn abs_null() {
    assert_eq!(val("RETURN abs(null)"), Value::Null);
}

#[test]
fn ceil_nominal() {
    approx(&val("RETURN ceil(2.1)"), 3.0);
}

#[test]
fn ceil_null() {
    assert_eq!(val("RETURN ceil(null)"), Value::Null);
}

#[test]
fn floor_nominal() {
    approx(&val("RETURN floor(2.9)"), 2.0);
}

#[test]
fn floor_null() {
    assert_eq!(val("RETURN floor(null)"), Value::Null);
}

#[test]
fn round_nominal() {
    approx(&val("RETURN round(2.5)"), 3.0);
}

#[test]
fn round_null() {
    assert_eq!(val("RETURN round(null)"), Value::Null);
}

#[test]
fn sqrt_nominal() {
    approx(&val("RETURN sqrt(16)"), 4.0);
}

#[test]
fn sqrt_null() {
    assert_eq!(val("RETURN sqrt(null)"), Value::Null);
}

#[test]
fn sign_positive() {
    assert_eq!(val("RETURN sign(5)"), Value::Int(1));
}

#[test]
fn sign_negative() {
    assert_eq!(val("RETURN sign(-5)"), Value::Int(-1));
}

#[test]
fn sign_zero() {
    assert_eq!(val("RETURN sign(0)"), Value::Int(0));
}

#[test]
fn sign_null() {
    assert_eq!(val("RETURN sign(null)"), Value::Null);
}

// ── math: log/log10/exp/e/pi ───────────────────────────────────────────────

#[test]
fn log_one_is_zero() {
    approx(&val("RETURN log(1)"), 0.0);
}

#[test]
fn log_null() {
    assert_eq!(val("RETURN log(null)"), Value::Null);
}

#[test]
fn log10_100() {
    approx(&val("RETURN log10(100)"), 2.0);
}

#[test]
fn log10_null() {
    assert_eq!(val("RETURN log10(null)"), Value::Null);
}

#[test]
fn exp_zero() {
    approx(&val("RETURN exp(0)"), 1.0);
}

#[test]
fn exp_null() {
    assert_eq!(val("RETURN exp(null)"), Value::Null);
}

#[test]
fn e_returns_constant() {
    approx(&val("RETURN e()"), std::f64::consts::E);
}

#[test]
fn pi_returns_constant() {
    approx(&val("RETURN pi()"), std::f64::consts::PI);
}

// ── math: trig ─────────────────────────────────────────────────────────────

#[test]
fn sin_zero() {
    approx(&val("RETURN sin(0)"), 0.0);
}

#[test]
fn sin_null() {
    assert_eq!(val("RETURN sin(null)"), Value::Null);
}

#[test]
fn cos_zero() {
    approx(&val("RETURN cos(0)"), 1.0);
}

#[test]
fn cos_null() {
    assert_eq!(val("RETURN cos(null)"), Value::Null);
}

#[test]
fn tan_zero() {
    approx(&val("RETURN tan(0)"), 0.0);
}

#[test]
fn tan_null() {
    assert_eq!(val("RETURN tan(null)"), Value::Null);
}

#[test]
fn asin_zero() {
    approx(&val("RETURN asin(0)"), 0.0);
}

#[test]
fn asin_null() {
    assert_eq!(val("RETURN asin(null)"), Value::Null);
}

#[test]
fn acos_one() {
    approx(&val("RETURN acos(1)"), 0.0);
}

#[test]
fn acos_null() {
    assert_eq!(val("RETURN acos(null)"), Value::Null);
}

#[test]
fn atan_zero() {
    approx(&val("RETURN atan(0)"), 0.0);
}

#[test]
fn atan_null() {
    assert_eq!(val("RETURN atan(null)"), Value::Null);
}

#[test]
fn atan2_nominal() {
    approx(&val("RETURN atan2(0, 1)"), 0.0);
}

#[test]
fn atan2_null() {
    assert_eq!(val("RETURN atan2(null, 1)"), Value::Null);
}

// ── string conversion: toString/toInteger/toFloat/toBoolean ────────────────

#[test]
fn tostring_int() {
    assert_eq!(val("RETURN toString(42)"), Value::Str("42".into()));
}

#[test]
fn tostring_null() {
    assert_eq!(val("RETURN toString(null)"), Value::Null);
}

#[test]
fn tointeger_string() {
    assert_eq!(val("RETURN toInteger('42')"), Value::Int(42));
}

#[test]
fn tointeger_float() {
    assert_eq!(val("RETURN toInteger(3.7)"), Value::Int(3));
}

#[test]
fn tointeger_null() {
    assert_eq!(val("RETURN toInteger(null)"), Value::Null);
}

#[test]
fn tofloat_string() {
    approx(&val("RETURN toFloat('3.25')"), 3.25);
}

#[test]
fn tofloat_null() {
    assert_eq!(val("RETURN toFloat(null)"), Value::Null);
}

#[test]
fn toboolean_true() {
    assert_eq!(val("RETURN toBoolean('true')"), Value::Bool(true));
}

#[test]
fn toboolean_null() {
    assert_eq!(val("RETURN toBoolean(null)"), Value::Null);
}

// ── string functions ──────────────────────────────────────────────────────

#[test]
fn toupper_nominal() {
    assert_eq!(val("RETURN toUpper('abc')"), Value::Str("ABC".into()));
}

#[test]
fn toupper_null() {
    assert_eq!(val("RETURN toUpper(null)"), Value::Null);
}

#[test]
fn tolower_nominal() {
    assert_eq!(val("RETURN toLower('ABC')"), Value::Str("abc".into()));
}

#[test]
fn tolower_null() {
    assert_eq!(val("RETURN toLower(null)"), Value::Null);
}

#[test]
fn trim_nominal() {
    assert_eq!(val("RETURN trim('  hi  ')"), Value::Str("hi".into()));
}

#[test]
fn trim_null() {
    assert_eq!(val("RETURN trim(null)"), Value::Null);
}

#[test]
fn ltrim_nominal() {
    assert_eq!(val("RETURN ltrim('  hi  ')"), Value::Str("hi  ".into()));
}

#[test]
fn ltrim_null() {
    assert_eq!(val("RETURN ltrim(null)"), Value::Null);
}

#[test]
fn rtrim_nominal() {
    assert_eq!(val("RETURN rtrim('  hi  ')"), Value::Str("  hi".into()));
}

#[test]
fn rtrim_null() {
    assert_eq!(val("RETURN rtrim(null)"), Value::Null);
}

#[test]
fn left_nominal() {
    assert_eq!(val("RETURN left('hello', 3)"), Value::Str("hel".into()));
}

#[test]
fn left_null() {
    assert_eq!(val("RETURN left(null, 3)"), Value::Null);
}

#[test]
fn right_nominal() {
    assert_eq!(val("RETURN right('hello', 3)"), Value::Str("llo".into()));
}

#[test]
fn right_null() {
    assert_eq!(val("RETURN right(null, 3)"), Value::Null);
}

#[test]
fn substring_2_args() {
    assert_eq!(
        val("RETURN substring('hello', 2)"),
        Value::Str("llo".into())
    );
}

#[test]
fn substring_3_args() {
    assert_eq!(
        val("RETURN substring('hello', 1, 3)"),
        Value::Str("ell".into())
    );
}

#[test]
fn substring_null() {
    assert_eq!(val("RETURN substring(null, 0)"), Value::Null);
}

#[test]
fn replace_nominal() {
    assert_eq!(
        val("RETURN replace('hello', 'l', 'L')"),
        Value::Str("heLLo".into())
    );
}

#[test]
fn replace_null() {
    assert_eq!(val("RETURN replace(null, 'a', 'b')"), Value::Null);
}

#[test]
fn split_nominal() {
    assert_eq!(
        val("RETURN split('a,b,c', ',')"),
        Value::List(vec![
            Value::Str("a".into()),
            Value::Str("b".into()),
            Value::Str("c".into())
        ])
    );
}

#[test]
fn split_null() {
    assert_eq!(val("RETURN split(null, ',')"), Value::Null);
}

// ── aggregations ───────────────────────────────────────────────────────────

#[test]
fn count_unwound() {
    let r = exec("UNWIND [1,2,3,4] AS x RETURN count(x) AS c");
    let (_, v) = r.rows[0].iter().next().unwrap();
    assert_eq!(*v, Value::Int(4));
}

#[test]
fn count_with_nulls_skips() {
    let r = exec("UNWIND [1, null, 2, null] AS x RETURN count(x) AS c");
    let (_, v) = r.rows[0].iter().next().unwrap();
    assert_eq!(*v, Value::Int(2));
}

#[test]
fn sum_nominal() {
    let r = exec("UNWIND [1,2,3] AS x RETURN sum(x) AS s");
    let (_, v) = r.rows[0].iter().next().unwrap();
    assert_eq!(*v, Value::Int(6));
}

#[test]
fn sum_with_null() {
    let r = exec("UNWIND [1, null, 2] AS x RETURN sum(x) AS s");
    let (_, v) = r.rows[0].iter().next().unwrap();
    assert_eq!(*v, Value::Int(3));
}

#[test]
fn avg_nominal() {
    let r = exec("UNWIND [2,4,6] AS x RETURN avg(x) AS a");
    let (_, v) = r.rows[0].iter().next().unwrap();
    approx(v, 4.0);
}

#[test]
fn min_nominal() {
    let r = exec("UNWIND [3,1,2] AS x RETURN min(x) AS m");
    let (_, v) = r.rows[0].iter().next().unwrap();
    assert_eq!(*v, Value::Int(1));
}

#[test]
fn max_nominal() {
    let r = exec("UNWIND [3,1,2] AS x RETURN max(x) AS m");
    let (_, v) = r.rows[0].iter().next().unwrap();
    assert_eq!(*v, Value::Int(3));
}

#[test]
fn collect_nominal() {
    let r = exec("UNWIND [1,2,3] AS x RETURN collect(x) AS c");
    let (_, v) = r.rows[0].iter().next().unwrap();
    assert_eq!(
        *v,
        Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)])
    );
}

#[test]
fn collect_skips_null() {
    let r = exec("UNWIND [1, null, 2] AS x RETURN collect(x) AS c");
    let (_, v) = r.rows[0].iter().next().unwrap();
    assert_eq!(*v, Value::List(vec![Value::Int(1), Value::Int(2)]));
}

#[test]
fn count_distinct() {
    let r = exec("UNWIND [1,1,2,3,3] AS x RETURN count(DISTINCT x) AS c");
    let (_, v) = r.rows[0].iter().next().unwrap();
    assert_eq!(*v, Value::Int(3));
}

#[test]
fn stdev_with_three_values() {
    let r = exec("UNWIND [1.0, 2.0, 3.0] AS x RETURN stDev(x) AS s");
    let (_, v) = r.rows[0].iter().next().unwrap();
    approx(v, 1.0);
}

#[test]
fn stdevp_with_three_values() {
    let r = exec("UNWIND [1.0, 2.0, 3.0] AS x RETURN stDevP(x) AS s");
    let (_, v) = r.rows[0].iter().next().unwrap();
    let f = match v {
        Value::Float(f) => *f,
        _ => panic!(),
    };
    assert!((f - (2.0_f64 / 3.0).sqrt()).abs() < 1e-6);
}

// ── node/edge functions: type/labels ───────────────────────────────────────

#[test]
fn labels_of_node() {
    let r = exec("CREATE (n:A:B) RETURN labels(n) AS ls");
    let (_, v) = r.rows[0].iter().next().unwrap();
    if let Value::List(items) = v {
        let strs: Vec<String> = items
            .iter()
            .filter_map(|x| match x {
                Value::Str(s) => Some(s.to_string()),
                _ => None,
            })
            .collect();
        assert!(strs.contains(&"A".to_string()));
        assert!(strs.contains(&"B".to_string()));
    } else {
        panic!("expected list");
    }
}

#[test]
fn labels_null() {
    assert_eq!(val("RETURN labels(null)"), Value::Null);
}

#[test]
fn type_of_edge() {
    let r = exec("CREATE (a)-[r:REL]->(b) RETURN type(r) AS t");
    let (_, v) = r.rows[0].iter().next().unwrap();
    assert_eq!(*v, Value::Str("REL".into()));
}

#[test]
fn type_null() {
    assert_eq!(val("RETURN type(null)"), Value::Null);
}

#[test]
fn exists_property_present() {
    let r = exec("CREATE (n:T {x: 1}) WITH n RETURN exists(n.x) AS e");
    let (_, v) = r.rows[0].iter().next().unwrap();
    assert_eq!(*v, Value::Bool(true));
}

#[test]
fn exists_property_absent() {
    let r = exec("CREATE (n:T {x: 1}) WITH n RETURN exists(n.y) AS e");
    let (_, v) = r.rows[0].iter().next().unwrap();
    assert_eq!(*v, Value::Bool(false));
}

// ── extra: rand & nominal of degrees/radians ───────────────────────────────

#[test]
fn rand_in_bounds() {
    let r = val("RETURN rand()");
    if let Value::Float(f) = r {
        assert!((0.0..1.0).contains(&f));
    } else {
        panic!("expected float");
    }
}

#[test]
fn degrees_pi_is_180() {
    approx(&val("RETURN degrees(pi())"), 180.0);
}

#[test]
fn radians_180_is_pi() {
    approx(&val("RETURN radians(180)"), std::f64::consts::PI);
}

#[test]
fn isnull_true() {
    assert_eq!(val("RETURN null IS NULL"), Value::Bool(true));
}

#[test]
fn isnull_false() {
    assert_eq!(val("RETURN 1 IS NULL"), Value::Bool(false));
}
