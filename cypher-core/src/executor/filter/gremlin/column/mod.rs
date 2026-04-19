//! Gremlin column functions extracted from filter::eval_function_with_vals.
//!
//! Behavior preserved verbatim from the original `filter` module.
//! Split into focused submodules:
//! - [`access`]: key/value/keys/values extraction and select(values)
//! - [`sort`]: local sort over lists and maps
//! - [`aggregate`]: sum/min/max/mean local aggregations

mod access;
mod aggregate;
mod sort;

pub(crate) use access::{column_key, column_keys, column_value, column_values, select_values};
pub(crate) use aggregate::{max_local, mean_local, min_local, sum_local};
pub(crate) use sort::sort_local;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Value;
    use indexmap::IndexMap;

    fn s(v: &str) -> Value {
        Value::Str(v.into())
    }

    fn l(items: Vec<Value>) -> Value {
        Value::List(items)
    }

    #[test]
    fn column_key_value_keys_values() {
        let mut m = IndexMap::<Box<str>, Value>::new();
        m.insert("k".into(), Value::Int(1));
        // single-entry map
        assert_eq!(column_key(vec![Value::Map(m.clone())]).unwrap(), s("k"));
        assert_eq!(
            column_value(vec![Value::Map(m.clone())]).unwrap(),
            Value::Int(1)
        );
        // pass-through and empty
        assert_eq!(column_key(vec![Value::Int(7)]).unwrap(), Value::Int(7));
        assert_eq!(column_key(vec![]).unwrap(), Value::Null);
        assert_eq!(column_value(vec![Value::Int(7)]).unwrap(), Value::Int(7));
        assert_eq!(column_value(vec![]).unwrap(), Value::Null);

        // multi-entry map for keys/values
        let mut m2 = IndexMap::<Box<str>, Value>::new();
        m2.insert("a".into(), Value::Int(1));
        m2.insert("b".into(), Value::Int(2));
        assert_eq!(
            column_keys(vec![Value::Map(m2.clone())]).unwrap(),
            l(vec![s("a"), s("b")])
        );
        assert_eq!(
            column_values(vec![Value::Map(m2.clone())]).unwrap(),
            l(vec![Value::Int(1), Value::Int(2)])
        );
        // List branches
        assert_eq!(
            column_keys(vec![l(vec![s("x"), s("y")])]).unwrap(),
            l(vec![Value::Int(0), Value::Int(1)])
        );
        assert_eq!(
            column_values(vec![l(vec![s("x"), s("y")])]).unwrap(),
            l(vec![s("x"), s("y")])
        );
        // Other / empty
        assert_eq!(column_keys(vec![Value::Int(7)]).unwrap(), Value::Int(7));
        assert_eq!(column_keys(vec![]).unwrap(), Value::Null);
        assert_eq!(column_values(vec![Value::Int(7)]).unwrap(), Value::Int(7));
        assert_eq!(column_values(vec![]).unwrap(), Value::Null);
    }

    #[test]
    fn select_values_branches() {
        // single-entry map with list value
        let mut m = IndexMap::<Box<str>, Value>::new();
        m.insert("k".into(), l(vec![Value::Int(1)]));
        assert_eq!(
            select_values(vec![Value::Map(m)]).unwrap(),
            l(vec![l(vec![Value::Int(1)])])
        );
        // single-entry map with scalar value
        let mut m = IndexMap::<Box<str>, Value>::new();
        m.insert("k".into(), Value::Int(7));
        assert_eq!(select_values(vec![Value::Map(m)]).unwrap(), Value::Int(7));
        // multi-entry map
        let mut m = IndexMap::<Box<str>, Value>::new();
        m.insert("a".into(), Value::Int(1));
        m.insert("b".into(), Value::Int(2));
        assert_eq!(
            select_values(vec![Value::Map(m)]).unwrap(),
            l(vec![Value::Int(1), Value::Int(2)])
        );
        // list passes through cloned
        assert_eq!(
            select_values(vec![l(vec![Value::Int(1)])]).unwrap(),
            l(vec![Value::Int(1)])
        );
        // scalar / empty
        assert_eq!(select_values(vec![Value::Int(5)]).unwrap(), Value::Int(5));
        assert_eq!(select_values(vec![]).unwrap(), Value::Null);
    }

    #[test]
    fn sort_local_list_and_map() {
        // Simple list ascending
        let r = sort_local(vec![l(vec![Value::Int(3), Value::Int(1), Value::Int(2)])]).unwrap();
        assert_eq!(r, l(vec![Value::Int(1), Value::Int(2), Value::Int(3)]));
        // Descending
        let r = sort_local(vec![
            l(vec![Value::Int(3), Value::Int(1), Value::Int(2)]),
            s(""),
            s("desc"),
        ])
        .unwrap();
        assert_eq!(r, l(vec![Value::Int(3), Value::Int(2), Value::Int(1)]));
        // Map sort by keys
        let mut m = IndexMap::<Box<str>, Value>::new();
        m.insert("b".into(), Value::Int(1));
        m.insert("a".into(), Value::Int(2));
        let r = sort_local(vec![Value::Map(m.clone())]).unwrap();
        if let Value::Map(mm) = r {
            let keys: Vec<&str> = mm.keys().map(|k| k.as_ref()).collect();
            assert_eq!(keys, vec!["a", "b"]);
        }
        // Map sort by Column.keys
        let r = sort_local(vec![Value::Map(m.clone()), s("Column.keys")]).unwrap();
        assert!(matches!(r, Value::Map(_)));
        // Map sort by other key (values)
        let r = sort_local(vec![Value::Map(m), s("v"), s("desc")]).unwrap();
        assert!(matches!(r, Value::Map(_)));
        // Pass-through scalar
        assert_eq!(sort_local(vec![Value::Int(7)]).unwrap(), Value::Int(7));
    }

    #[test]
    fn sum_min_max_mean_local() {
        // sum: ints, mixed, empty, null
        assert_eq!(
            sum_local(vec![l(vec![Value::Int(1), Value::Int(2), Value::Null])]).unwrap(),
            Value::Int(3)
        );
        assert_eq!(
            sum_local(vec![l(vec![Value::Int(1), Value::Float(0.5)])]).unwrap(),
            Value::Float(1.5)
        );
        assert_eq!(sum_local(vec![l(vec![])]).unwrap(), Value::Null);
        assert_eq!(sum_local(vec![Value::Int(7)]).unwrap(), Value::Int(7));
        assert_eq!(
            sum_local(vec![Value::Float(1.5)]).unwrap(),
            Value::Float(1.5)
        );
        assert_eq!(sum_local(vec![Value::Null]).unwrap(), Value::Null);
        assert_eq!(sum_local(vec![s("x")]).unwrap(), Value::Null);

        // min/max: list with nulls + scalars
        assert_eq!(
            min_local(vec![l(vec![
                Value::Int(3),
                Value::Int(1),
                Value::Null,
                Value::Int(2)
            ])])
            .unwrap(),
            Value::Int(1)
        );
        assert_eq!(
            max_local(vec![l(vec![
                Value::Int(3),
                Value::Int(1),
                Value::Null,
                Value::Int(2)
            ])])
            .unwrap(),
            Value::Int(3)
        );
        assert_eq!(min_local(vec![l(vec![])]).unwrap(), Value::Null);
        assert_eq!(min_local(vec![Value::Int(5)]).unwrap(), Value::Int(5));
        assert_eq!(
            min_local(vec![Value::Float(1.5)]).unwrap(),
            Value::Float(1.5)
        );
        assert_eq!(min_local(vec![s("a")]).unwrap(), s("a"));
        assert_eq!(min_local(vec![Value::Null]).unwrap(), Value::Null);
        assert_eq!(min_local(vec![Value::Bool(true)]).unwrap(), Value::Null);
        assert_eq!(max_local(vec![Value::Int(5)]).unwrap(), Value::Int(5));
        assert_eq!(
            max_local(vec![Value::Float(1.5)]).unwrap(),
            Value::Float(1.5)
        );
        assert_eq!(max_local(vec![s("a")]).unwrap(), s("a"));
        assert_eq!(max_local(vec![Value::Null]).unwrap(), Value::Null);

        // mean
        assert_eq!(
            mean_local(vec![l(vec![Value::Int(2), Value::Int(4)])]).unwrap(),
            Value::Float(3.0)
        );
        assert_eq!(
            mean_local(vec![l(vec![Value::Float(1.0), Value::Float(3.0)])]).unwrap(),
            Value::Float(2.0)
        );
        assert_eq!(mean_local(vec![l(vec![])]).unwrap(), Value::Null);
        assert_eq!(mean_local(vec![Value::Int(5)]).unwrap(), Value::Float(5.0));
        assert_eq!(
            mean_local(vec![Value::Float(1.5)]).unwrap(),
            Value::Float(1.5)
        );
        assert_eq!(mean_local(vec![Value::Null]).unwrap(), Value::Null);
        assert_eq!(mean_local(vec![s("a")]).unwrap(), Value::Null);
    }
}
