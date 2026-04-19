//! Tests for the cross-type `cmp_values` ordering helper used by sort/order-by
//! plumbing that interacts with decoded path values.

use crate::executor::engine::helpers::cmp_values;
use crate::types::Value;

#[test]
fn cmp_values_cross_type_ordering() {
    use std::cmp::Ordering;
    assert_eq!(
        cmp_values(&Value::Str("z".into()), &Value::Bool(true)),
        Ordering::Less
    );
    assert_eq!(
        cmp_values(&Value::Bool(false), &Value::Str("a".into())),
        Ordering::Greater
    );
}

#[test]
fn cmp_values_nan_sorts_after_regular_float() {
    use std::cmp::Ordering;
    let nan = Value::Float(f64::NAN);
    let regular = Value::Float(1.0);
    assert_eq!(cmp_values(&nan, &regular), Ordering::Greater);
    assert_eq!(cmp_values(&regular, &nan), Ordering::Less);
}

#[test]
fn cmp_values_list_lexicographic() {
    use std::cmp::Ordering;
    let short = Value::List(vec![Value::Int(1), Value::Int(2)]);
    let long = Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
    let diff = Value::List(vec![Value::Int(1), Value::Int(3)]);
    assert_eq!(cmp_values(&short, &long), Ordering::Less);
    assert_eq!(cmp_values(&short, &diff), Ordering::Less);
    assert_eq!(cmp_values(&short, &short.clone()), Ordering::Equal);
}

#[test]
fn cmp_values_null_sorts_last() {
    use std::cmp::Ordering;
    assert_eq!(cmp_values(&Value::Null, &Value::Int(0)), Ordering::Greater);
    assert_eq!(
        cmp_values(&Value::Null, &Value::Str("z".into())),
        Ordering::Greater
    );
    assert_eq!(cmp_values(&Value::Bool(true), &Value::Null), Ordering::Less);
    assert_eq!(cmp_values(&Value::Null, &Value::Null), Ordering::Equal);
}

#[test]
fn cmp_values_time_utc_normalized() {
    use std::cmp::Ordering;
    let a = Value::Time(jiff::civil::Time::new(10, 35, 0, 0).unwrap(), -28800);
    let d = Value::Time(jiff::civil::Time::new(12, 35, 15, 0).unwrap(), 18000);
    assert_eq!(
        cmp_values(&a, &d),
        Ordering::Greater,
        "A(18:35Z) should be > D(07:35Z)"
    );
    assert_eq!(
        cmp_values(&d, &a),
        Ordering::Less,
        "D(07:35Z) should be < A(18:35Z)"
    );
    let t1 = Value::Time(jiff::civil::Time::new(14, 0, 0, 0).unwrap(), 7200);
    let t2 = Value::Time(jiff::civil::Time::new(13, 0, 0, 0).unwrap(), 3600);
    assert_eq!(cmp_values(&t1, &t2), Ordering::Equal);
}
