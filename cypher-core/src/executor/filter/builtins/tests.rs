//! Unit tests for the builtins module.
//!
//! Tests use `super::*` to reach the re-exports declared in `mod.rs`.

use super::helpers::{require_arity, type_error};
use super::*;
use crate::types::Value;

#[test]
fn tointeger_nan_returns_null() {
    let result = builtin_tointeger(vec![Value::Float(f64::NAN)]).unwrap();
    assert_eq!(result, Value::Null);
}

#[test]
fn tointeger_positive_infinity_returns_null() {
    let result = builtin_tointeger(vec![Value::Float(f64::INFINITY)]).unwrap();
    assert_eq!(result, Value::Null);
}

#[test]
fn tointeger_negative_infinity_returns_null() {
    let result = builtin_tointeger(vec![Value::Float(f64::NEG_INFINITY)]).unwrap();
    assert_eq!(result, Value::Null);
}

#[test]
fn tointeger_finite_float_returns_int() {
    let result = builtin_tointeger(vec![Value::Float(3.7)]).unwrap();
    assert_eq!(result, Value::Int(3));
}

#[test]
fn tointeger_string_nan_returns_null() {
    let result = builtin_tointeger(vec![Value::Str("NaN".into())]).unwrap();
    assert_eq!(result, Value::Null);
}

#[test]
fn tointeger_string_infinity_returns_null() {
    let result = builtin_tointeger(vec![Value::Str("Infinity".into())]).unwrap();
    assert_eq!(result, Value::Null);
}

#[test]
fn tointeger_string_neg_infinity_returns_null() {
    let result = builtin_tointeger(vec![Value::Str("-Infinity".into())]).unwrap();
    assert_eq!(result, Value::Null);
}

#[test]
fn tointeger_null_returns_null() {
    let result = builtin_tointeger(vec![Value::Null]).unwrap();
    assert_eq!(result, Value::Null);
}

#[test]
fn tointeger_int_returns_same() {
    let result = builtin_tointeger(vec![Value::Int(42)]).unwrap();
    assert_eq!(result, Value::Int(42));
}

// ── Math function tests ─────────────────────────────────────────────────

#[test]
fn log_of_1_returns_0() {
    let result = builtin_log(vec![Value::Int(1)]).unwrap();
    assert_eq!(result, Value::Float(0.0));
}

#[test]
fn log_of_e_returns_1() {
    let result = builtin_log(vec![Value::Float(std::f64::consts::E)]).unwrap();
    match result {
        Value::Float(f) => assert!((f - 1.0).abs() < 1e-10),
        _ => panic!("expected Float"),
    }
}

#[test]
fn log_null_returns_null() {
    let result = builtin_log(vec![Value::Null]).unwrap();
    assert_eq!(result, Value::Null);
}

#[test]
fn log10_of_100_returns_2() {
    let result = builtin_log10(vec![Value::Int(100)]).unwrap();
    match result {
        Value::Float(f) => assert!((f - 2.0).abs() < 1e-10),
        _ => panic!("expected Float"),
    }
}

#[test]
fn exp_of_0_returns_1() {
    let result = builtin_exp(vec![Value::Int(0)]).unwrap();
    assert_eq!(result, Value::Float(1.0));
}

#[test]
fn exp_of_1_returns_e() {
    let result = builtin_exp(vec![Value::Int(1)]).unwrap();
    match result {
        Value::Float(f) => assert!((f - std::f64::consts::E).abs() < 1e-10),
        _ => panic!("expected Float"),
    }
}

#[test]
fn sin_of_0_returns_0() {
    let result = builtin_sin(vec![Value::Int(0)]).unwrap();
    assert_eq!(result, Value::Float(0.0));
}

#[test]
fn cos_of_0_returns_1() {
    let result = builtin_cos(vec![Value::Int(0)]).unwrap();
    assert_eq!(result, Value::Float(1.0));
}

#[test]
fn tan_of_0_returns_0() {
    let result = builtin_tan(vec![Value::Int(0)]).unwrap();
    assert_eq!(result, Value::Float(0.0));
}

#[test]
fn asin_of_0_returns_0() {
    let result = builtin_asin(vec![Value::Int(0)]).unwrap();
    assert_eq!(result, Value::Float(0.0));
}

#[test]
fn acos_of_1_returns_0() {
    let result = builtin_acos(vec![Value::Int(1)]).unwrap();
    assert_eq!(result, Value::Float(0.0));
}

#[test]
fn atan_of_0_returns_0() {
    let result = builtin_atan(vec![Value::Int(0)]).unwrap();
    assert_eq!(result, Value::Float(0.0));
}

#[test]
fn atan2_of_0_1_returns_0() {
    let result = builtin_atan2(vec![Value::Int(0), Value::Int(1)]).unwrap();
    assert_eq!(result, Value::Float(0.0));
}

#[test]
fn atan2_of_1_0_returns_pi_over_2() {
    let result = builtin_atan2(vec![Value::Float(1.0), Value::Float(0.0)]).unwrap();
    match result {
        Value::Float(f) => assert!((f - std::f64::consts::FRAC_PI_2).abs() < 1e-10),
        _ => panic!("expected Float"),
    }
}

#[test]
fn atan2_mixed_types() {
    let result = builtin_atan2(vec![Value::Int(1), Value::Float(0.0)]).unwrap();
    match result {
        Value::Float(f) => assert!((f - std::f64::consts::FRAC_PI_2).abs() < 1e-10),
        _ => panic!("expected Float"),
    }
}

#[test]
fn atan2_null_returns_null() {
    let result = builtin_atan2(vec![Value::Null, Value::Int(1)]).unwrap();
    assert_eq!(result, Value::Null);
}

#[test]
fn pi_returns_pi() {
    let result = builtin_pi(vec![]).unwrap();
    assert_eq!(result, Value::Float(std::f64::consts::PI));
}

#[test]
fn e_returns_e() {
    let result = builtin_e(vec![]).unwrap();
    assert_eq!(result, Value::Float(std::f64::consts::E));
}

#[test]
fn degrees_of_pi_returns_180() {
    let result = builtin_degrees(vec![Value::Float(std::f64::consts::PI)]).unwrap();
    match result {
        Value::Float(f) => assert!((f - 180.0).abs() < 1e-10),
        _ => panic!("expected Float"),
    }
}

#[test]
fn radians_of_180_returns_pi() {
    let result = builtin_radians(vec![Value::Int(180)]).unwrap();
    match result {
        Value::Float(f) => assert!((f - std::f64::consts::PI).abs() < 1e-10),
        _ => panic!("expected Float"),
    }
}

#[test]
fn haversin_of_0_returns_0() {
    let result = builtin_haversin(vec![Value::Int(0)]).unwrap();
    assert_eq!(result, Value::Float(0.0));
}

#[test]
fn haversin_of_pi_returns_1() {
    let result = builtin_haversin(vec![Value::Float(std::f64::consts::PI)]).unwrap();
    match result {
        Value::Float(f) => assert!((f - 1.0).abs() < 1e-10),
        _ => panic!("expected Float"),
    }
}

#[test]
fn haversin_null_returns_null() {
    let result = builtin_haversin(vec![Value::Null]).unwrap();
    assert_eq!(result, Value::Null);
}

#[test]
fn math_functions_reject_string() {
    assert!(builtin_log(vec![Value::Str("x".into())]).is_err());
    assert!(builtin_exp(vec![Value::Str("x".into())]).is_err());
    assert!(builtin_sin(vec![Value::Str("x".into())]).is_err());
    assert!(builtin_degrees(vec![Value::Str("x".into())]).is_err());
    assert!(builtin_radians(vec![Value::Str("x".into())]).is_err());
    assert!(builtin_haversin(vec![Value::Str("x".into())]).is_err());
}

#[test]
fn pi_rejects_args() {
    assert!(builtin_pi(vec![Value::Int(1)]).is_err());
}

#[test]
fn e_rejects_args() {
    assert!(builtin_e(vec![Value::Int(1)]).is_err());
}

// ── Additional coverage: numerics, strings, collections, range ─────────
#[test]
fn abs_variants() {
    assert_eq!(builtin_abs(vec![Value::Null]).unwrap(), Value::Null);
    assert_eq!(builtin_abs(vec![Value::Int(-5)]).unwrap(), Value::Int(5));
    assert_eq!(
        builtin_abs(vec![Value::Float(-1.5)]).unwrap(),
        Value::Float(1.5)
    );
    assert!(builtin_abs(vec![Value::Str("x".into())]).is_err());
    assert!(builtin_abs(vec![Value::Int(i64::MIN)]).is_err());
}
#[test]
fn ceil_floor_round_sqrt() {
    assert_eq!(
        builtin_ceil(vec![Value::Float(1.2)]).unwrap(),
        Value::Float(2.0)
    );
    assert_eq!(builtin_ceil(vec![Value::Int(1)]).unwrap(), Value::Int(1));
    assert_eq!(builtin_ceil(vec![Value::Null]).unwrap(), Value::Null);
    assert!(builtin_ceil(vec![Value::Str("x".into())]).is_err());
    assert_eq!(
        builtin_floor(vec![Value::Float(1.9)]).unwrap(),
        Value::Float(1.0)
    );
    assert_eq!(builtin_floor(vec![Value::Int(1)]).unwrap(), Value::Int(1));
    assert_eq!(builtin_floor(vec![Value::Null]).unwrap(), Value::Null);
    assert_eq!(
        builtin_round(vec![Value::Float(1.5)]).unwrap(),
        Value::Float(2.0)
    );
    assert_eq!(builtin_round(vec![Value::Int(2)]).unwrap(), Value::Int(2));
    assert_eq!(builtin_round(vec![Value::Null]).unwrap(), Value::Null);
    assert_eq!(
        builtin_sqrt(vec![Value::Int(4)]).unwrap(),
        Value::Float(2.0)
    );
    assert_eq!(
        builtin_sqrt(vec![Value::Float(9.0)]).unwrap(),
        Value::Float(3.0)
    );
    assert_eq!(builtin_sqrt(vec![Value::Null]).unwrap(), Value::Null);
    assert!(builtin_sqrt(vec![Value::Str("x".into())]).is_err());
}
#[test]
fn sign_works() {
    assert_eq!(builtin_sign(vec![Value::Int(-5)]).unwrap(), Value::Int(-1));
    assert_eq!(builtin_sign(vec![Value::Int(0)]).unwrap(), Value::Int(0));
    assert_eq!(builtin_sign(vec![Value::Int(7)]).unwrap(), Value::Int(1));
    assert_eq!(builtin_sign(vec![Value::Null]).unwrap(), Value::Null);
    assert!(builtin_sign(vec![Value::Str("x".into())]).is_err());
    // Float branch
    let _ = builtin_sign(vec![Value::Float(-2.5)]);
}
#[test]
fn tostring_variants() {
    assert_eq!(builtin_tostring(vec![Value::Null]).unwrap(), Value::Null);
    assert_eq!(
        builtin_tostring(vec![Value::Str("hi".into())]).unwrap(),
        Value::Str("hi".into())
    );
    assert_eq!(
        builtin_tostring(vec![Value::Int(42)]).unwrap(),
        Value::Str("42".into())
    );
    assert_eq!(
        builtin_tostring(vec![Value::Bool(true)]).unwrap(),
        Value::Str("true".into())
    );
    assert!(matches!(
        builtin_tostring(vec![Value::Float(1.5)]).unwrap(),
        Value::Str(_)
    ));
    let d = Value::Date(jiff::civil::Date::new(2024, 1, 2).unwrap());
    assert!(matches!(builtin_tostring(vec![d]).unwrap(), Value::Str(_)));
    assert!(builtin_tostring(vec![Value::List(vec![])]).is_err());
}
#[test]
fn tofloat_variants() {
    assert_eq!(builtin_tofloat(vec![Value::Null]).unwrap(), Value::Null);
    assert_eq!(
        builtin_tofloat(vec![Value::Float(1.0)]).unwrap(),
        Value::Float(1.0)
    );
    assert_eq!(
        builtin_tofloat(vec![Value::Int(2)]).unwrap(),
        Value::Float(2.0)
    );
    assert_eq!(
        builtin_tofloat(vec![Value::Str("3.5".into())]).unwrap(),
        Value::Float(3.5)
    );
    assert_eq!(
        builtin_tofloat(vec![Value::Str("zzz".into())]).unwrap(),
        Value::Null
    );
    assert!(builtin_tofloat(vec![Value::List(vec![])]).is_err());
}
#[test]
fn toboolean_variants() {
    assert_eq!(builtin_toboolean(vec![Value::Null]).unwrap(), Value::Null);
    assert_eq!(
        builtin_toboolean(vec![Value::Bool(true)]).unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        builtin_toboolean(vec![Value::Str("True".into())]).unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        builtin_toboolean(vec![Value::Str("false".into())]).unwrap(),
        Value::Bool(false)
    );
    assert_eq!(
        builtin_toboolean(vec![Value::Str("zzz".into())]).unwrap(),
        Value::Null
    );
    assert!(builtin_toboolean(vec![Value::Int(0)]).is_err());
}
#[test]
fn size_and_length() {
    assert_eq!(builtin_size(vec![Value::Null]).unwrap(), Value::Null);
    assert_eq!(
        builtin_size(vec![Value::Str("héllo".into())]).unwrap(),
        Value::Int(5)
    );
    assert_eq!(
        builtin_size(vec![Value::List(vec![Value::Int(1), Value::Int(2)])]).unwrap(),
        Value::Int(2)
    );
    assert!(builtin_size(vec![Value::Int(1)]).is_err());
    assert_eq!(builtin_length(vec![Value::Null]).unwrap(), Value::Null);
    assert_eq!(
        builtin_length(vec![Value::Str("abc".into())]).unwrap(),
        Value::Int(3)
    );
    assert!(builtin_length(vec![Value::Int(1)]).is_err());
}
#[test]
fn case_and_trim() {
    assert_eq!(
        builtin_toupper(vec![Value::Str("ab".into())]).unwrap(),
        Value::Str("AB".into())
    );
    assert_eq!(builtin_toupper(vec![Value::Null]).unwrap(), Value::Null);
    assert!(builtin_toupper(vec![Value::Int(1)]).is_err());
    assert_eq!(
        builtin_tolower(vec![Value::Str("AB".into())]).unwrap(),
        Value::Str("ab".into())
    );
    assert_eq!(builtin_tolower(vec![Value::Null]).unwrap(), Value::Null);
    assert!(builtin_tolower(vec![Value::Int(1)]).is_err());
    assert_eq!(
        builtin_trim(vec![Value::Str("  hi  ".into())]).unwrap(),
        Value::Str("hi".into())
    );
    assert_eq!(builtin_trim(vec![Value::Null]).unwrap(), Value::Null);
    assert!(builtin_trim(vec![Value::Int(1)]).is_err());
    assert_eq!(
        builtin_ltrim(vec![Value::Str("  hi".into())]).unwrap(),
        Value::Str("hi".into())
    );
    assert_eq!(builtin_ltrim(vec![Value::Null]).unwrap(), Value::Null);
    assert!(builtin_ltrim(vec![Value::Int(1)]).is_err());
    assert_eq!(
        builtin_rtrim(vec![Value::Str("hi  ".into())]).unwrap(),
        Value::Str("hi".into())
    );
    assert_eq!(builtin_rtrim(vec![Value::Null]).unwrap(), Value::Null);
    assert!(builtin_rtrim(vec![Value::Int(1)]).is_err());
}
#[test]
fn left_right_substring_replace_split() {
    assert_eq!(
        builtin_left(vec![Value::Str("abcdef".into()), Value::Int(3)]).unwrap(),
        Value::Str("abc".into())
    );
    assert_eq!(
        builtin_left(vec![Value::Null, Value::Int(3)]).unwrap(),
        Value::Null
    );
    assert!(builtin_left(vec![Value::Int(1), Value::Int(1)]).is_err());
    assert_eq!(
        builtin_right(vec![Value::Str("abcdef".into()), Value::Int(2)]).unwrap(),
        Value::Str("ef".into())
    );
    assert_eq!(
        builtin_right(vec![Value::Null, Value::Int(2)]).unwrap(),
        Value::Null
    );
    assert!(builtin_right(vec![Value::Int(1), Value::Int(1)]).is_err());
    assert_eq!(
        builtin_substring(vec![Value::Str("abcdef".into()), Value::Int(2)]).unwrap(),
        Value::Str("cdef".into())
    );
    assert_eq!(
        builtin_substring(vec![
            Value::Str("abcdef".into()),
            Value::Int(1),
            Value::Int(3)
        ])
        .unwrap(),
        Value::Str("bcd".into())
    );
    assert_eq!(
        builtin_substring(vec![Value::Null, Value::Int(0)]).unwrap(),
        Value::Null
    );
    assert!(builtin_substring(vec![Value::Int(1), Value::Int(0)]).is_err());
    assert_eq!(
        builtin_replace(vec![
            Value::Str("hello".into()),
            Value::Str("l".into()),
            Value::Str("L".into())
        ])
        .unwrap(),
        Value::Str("heLLo".into())
    );
    assert_eq!(
        builtin_replace(vec![
            Value::Null,
            Value::Str("a".into()),
            Value::Str("b".into())
        ])
        .unwrap(),
        Value::Null
    );
    assert!(builtin_replace(vec![
        Value::Int(1),
        Value::Str("a".into()),
        Value::Str("b".into())
    ])
    .is_err());
    let r = builtin_split(vec![Value::Str("a,b,c".into()), Value::Str(",".into())]).unwrap();
    if let Value::List(l) = r {
        assert_eq!(l.len(), 3);
    } else {
        panic!()
    }
    assert_eq!(
        builtin_split(vec![Value::Null, Value::Str(",".into())]).unwrap(),
        Value::Null
    );
    assert!(builtin_split(vec![Value::Int(1), Value::Str(",".into())]).is_err());
}
#[test]
fn head_last_tail_reverse() {
    let l = || Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
    assert_eq!(builtin_head(vec![l()]).unwrap(), Value::Int(1));
    assert_eq!(builtin_head(vec![Value::Null]).unwrap(), Value::Null);
    assert_eq!(
        builtin_head(vec![Value::List(vec![])]).unwrap(),
        Value::Null
    );
    assert!(builtin_head(vec![Value::Int(1)]).is_err());
    assert_eq!(builtin_last(vec![l()]).unwrap(), Value::Int(3));
    assert_eq!(builtin_last(vec![Value::Null]).unwrap(), Value::Null);
    assert_eq!(
        builtin_last(vec![Value::List(vec![])]).unwrap(),
        Value::Null
    );
    assert!(builtin_last(vec![Value::Int(1)]).is_err());
    if let Value::List(t) = builtin_tail(vec![l()]).unwrap() {
        assert_eq!(t.len(), 2);
    } else {
        panic!()
    }
    assert_eq!(builtin_tail(vec![Value::Null]).unwrap(), Value::Null);
    assert!(builtin_tail(vec![Value::Int(1)]).is_err());
    if let Value::List(rv) = builtin_reverse(vec![l()]).unwrap() {
        assert_eq!(rv, vec![Value::Int(3), Value::Int(2), Value::Int(1)]);
    } else {
        panic!()
    }
    assert_eq!(
        builtin_reverse(vec![Value::Str("abc".into())]).unwrap(),
        Value::Str("cba".into())
    );
    assert_eq!(builtin_reverse(vec![Value::Null]).unwrap(), Value::Null);
    assert!(builtin_reverse(vec![Value::Int(1)]).is_err());
}
#[test]
fn coalesce_isnull_keys_values_properties() {
    assert_eq!(
        builtin_coalesce(vec![Value::Null, Value::Int(2), Value::Int(3)]).unwrap(),
        Value::Int(2)
    );
    assert_eq!(
        builtin_coalesce(vec![Value::Null, Value::Null]).unwrap(),
        Value::Null
    );
    assert_eq!(
        builtin_isnull(vec![Value::Null]).unwrap(),
        Value::Bool(true)
    );
    assert_eq!(
        builtin_isnull(vec![Value::Int(1)]).unwrap(),
        Value::Bool(false)
    );
    let mut m = indexmap::IndexMap::<Box<str>, Value>::new();
    m.insert("a".into(), Value::Int(1));
    m.insert("b".into(), Value::Null); // filtered out by keys
    let r = builtin_keys(vec![Value::Map(m.clone())]).unwrap();
    if let Value::List(ks) = r {
        assert_eq!(ks.len(), 1);
    } else {
        panic!()
    }
    assert_eq!(builtin_keys(vec![Value::Null]).unwrap(), Value::Null);
    assert!(builtin_keys(vec![Value::Int(1)]).is_err());
    let r = builtin_values(vec![Value::Map(m.clone())]).unwrap();
    if let Value::List(vs) = r {
        assert_eq!(vs.len(), 2);
    } else {
        panic!()
    }
    assert_eq!(builtin_values(vec![Value::Null]).unwrap(), Value::Null);
    assert!(builtin_values(vec![Value::Int(1)]).is_err());
    if let Value::Map(_) = builtin_properties(vec![Value::Map(m)]).unwrap() {
    } else {
        panic!()
    }
    assert_eq!(builtin_properties(vec![Value::Null]).unwrap(), Value::Null);
    assert!(builtin_properties(vec![Value::Int(1)]).is_err());
}
#[test]
fn keys_handles_special_node_edge_keys() {
    let mut m = indexmap::IndexMap::<Box<str>, Value>::new();
    m.insert("v[42]".into(), Value::Int(1));
    m.insert("e[7]".into(), Value::Int(2));
    m.insert("plain".into(), Value::Int(3));
    let r = builtin_keys(vec![Value::Map(m)]).unwrap();
    if let Value::List(ks) = r {
        assert!(ks
            .iter()
            .any(|v| matches!(v, Value::Node(crate::storage::NodeId(42)))));
        assert!(ks
            .iter()
            .any(|v| matches!(v, Value::Edge(crate::storage::EdgeId(7)))));
        assert!(ks
            .iter()
            .any(|v| matches!(v, Value::Str(s) if &**s == "plain")));
    } else {
        panic!()
    }
}
#[test]
fn range_two_and_three_args() {
    if let Value::List(l) = builtin_range(vec![Value::Int(1), Value::Int(3)]).unwrap() {
        assert_eq!(l, vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
    } else {
        panic!()
    }
    if let Value::List(l) =
        builtin_range(vec![Value::Int(0), Value::Int(10), Value::Int(2)]).unwrap()
    {
        assert_eq!(l.len(), 6);
    } else {
        panic!()
    }
    if let Value::List(l) =
        builtin_range(vec![Value::Int(5), Value::Int(0), Value::Int(-1)]).unwrap()
    {
        assert_eq!(l.len(), 6);
    } else {
        panic!()
    }
    assert!(builtin_range(vec![Value::Int(1)]).is_err());
    assert!(builtin_range(vec![
        Value::Int(1),
        Value::Int(1),
        Value::Int(1),
        Value::Int(1)
    ])
    .is_err());
    assert!(builtin_range(vec![Value::Int(1), Value::Int(2), Value::Int(0)]).is_err());
    assert!(builtin_range(vec![Value::Int(1), Value::Int(2), Value::Str("x".into())]).is_err());
    assert!(builtin_range(vec![Value::Str("x".into()), Value::Int(2)]).is_err());
}
#[test]
fn rand_and_java_random_basics() {
    if let Value::Float(f) = builtin_rand(vec![]).unwrap() {
        assert!((0.0..1.0).contains(&f));
    } else {
        panic!()
    }
    // Wrong arity for rand
    assert!(builtin_rand(vec![Value::Int(1)]).is_err());
    // java_random_next_double: 0 args, 1-arg seed, extra-args ignored
    assert!(matches!(
        builtin_java_random_next_double(vec![]).unwrap(),
        Value::Float(_)
    ));
    assert!(matches!(
        builtin_java_random_next_double(vec![Value::Int(1)]).unwrap(),
        Value::Float(_)
    ));
    assert!(matches!(
        builtin_java_random_next_double(vec![Value::Int(2), Value::Int(99)]).unwrap(),
        Value::Float(_)
    ));
}
#[test]
fn require_arity_paths() {
    // ok path
    assert!(require_arity("x", 1, &[Value::Int(1)]).is_ok());
    // error path
    assert!(require_arity("x", 2, &[Value::Int(1)]).is_err());
}
#[test]
fn type_error_returns_error() {
    assert!(type_error("foo", &Value::Int(1)).is_err());
}
