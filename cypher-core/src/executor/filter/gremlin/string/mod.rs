//! Gremlin string functions extracted from filter::eval_function_with_vals.
//!
//! Behavior preserved verbatim from the original `filter` module. Decomposed
//! into submodules grouped by string operation family.

#![allow(clippy::needless_pass_by_value)]

mod case;
mod length;
mod replace;
mod split;
mod substring;
mod trim;

pub(crate) use case::{tolower, tolower_local, toupper, toupper_local};
pub(crate) use length::{count_local, length, length_local, reverse};
pub(crate) use replace::{replace, replace_local};
pub(crate) use split::{split, split_local};
pub(crate) use substring::{substring, substring_local};
pub(crate) use trim::{ltrim, ltrim_local, rtrim, rtrim_local, trim, trim_local};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Value;
    use indexmap::IndexMap;

    fn s(v: &str) -> Value {
        Value::Str(v.into())
    }

    fn list(v: Vec<Value>) -> Value {
        Value::List(v)
    }

    #[test]
    fn tolower_basic_and_null_and_list_err() {
        assert_eq!(tolower(vec![s("AbC")]).unwrap(), s("abc"));
        assert_eq!(tolower(vec![Value::Null]).unwrap(), Value::Null);
        assert!(tolower(vec![list(vec![s("X")])]).is_err());
        assert_eq!(tolower(vec![Value::Int(7)]).unwrap(), Value::Null);
        assert_eq!(tolower(vec![]).unwrap(), Value::Null);
    }

    #[test]
    fn toupper_basic_and_null_and_list_err() {
        assert_eq!(toupper(vec![s("AbC")]).unwrap(), s("ABC"));
        assert_eq!(toupper(vec![Value::Null]).unwrap(), Value::Null);
        assert!(toupper(vec![list(vec![s("X")])]).is_err());
        assert_eq!(toupper(vec![Value::Int(7)]).unwrap(), Value::Null);
    }

    #[test]
    fn trim_variants() {
        assert_eq!(trim(vec![s("  hi  ")]).unwrap(), s("hi"));
        assert_eq!(ltrim(vec![s("  hi  ")]).unwrap(), s("hi  "));
        assert_eq!(rtrim(vec![s("  hi  ")]).unwrap(), s("  hi"));
        assert_eq!(trim(vec![Value::Null]).unwrap(), Value::Null);
        assert_eq!(ltrim(vec![Value::Null]).unwrap(), Value::Null);
        assert_eq!(rtrim(vec![Value::Null]).unwrap(), Value::Null);
        assert!(trim(vec![list(vec![])]).is_err());
        assert!(ltrim(vec![list(vec![])]).is_err());
        assert!(rtrim(vec![list(vec![])]).is_err());
        assert_eq!(trim(vec![Value::Int(0)]).unwrap(), Value::Null);
    }

    #[test]
    fn length_basic() {
        assert_eq!(length(vec![s("hello")]).unwrap(), Value::Int(5));
        assert_eq!(length(vec![Value::Null]).unwrap(), Value::Null);
        assert!(length(vec![Value::Int(1)]).is_err());
        assert_eq!(length(vec![]).unwrap(), Value::Int(0));
    }

    #[test]
    fn reverse_string_and_list_and_other() {
        assert_eq!(reverse(vec![s("abc")]).unwrap(), s("cba"));
        assert_eq!(
            reverse(vec![list(vec![Value::Int(1), Value::Int(2)])]).unwrap(),
            list(vec![Value::Int(2), Value::Int(1)])
        );
        assert_eq!(reverse(vec![Value::Null]).unwrap(), Value::Null);
        assert_eq!(reverse(vec![Value::Int(5)]).unwrap(), Value::Int(5));
        assert_eq!(reverse(vec![]).unwrap(), Value::Null);
    }

    #[test]
    fn replace_basic() {
        assert_eq!(
            replace(vec![s("abcabc"), s("a"), s("X")]).unwrap(),
            s("XbcXbc")
        );
        assert_eq!(
            replace(vec![Value::Null, s("a"), s("b")]).unwrap(),
            Value::Null
        );
        assert!(replace(vec![list(vec![]), s("a"), s("b")]).is_err());
        assert_eq!(replace(vec![s("only-one")]).unwrap(), Value::Null);
    }

    #[test]
    fn split_separator_variants() {
        let r = split(vec![s("a,b,c"), s(",")]).unwrap();
        assert_eq!(r, list(vec![s("a"), s("b"), s("c")]));
        // Empty separator splits to chars
        let r = split(vec![s("ab"), s("")]).unwrap();
        assert_eq!(r, list(vec![s("a"), s("b")]));
        // "null" separator → whitespace split
        let r = split(vec![s("a b\tc"), s("null")]).unwrap();
        assert_eq!(r, list(vec![s("a"), s("b"), s("c")]));
        assert_eq!(split(vec![Value::Null, s(",")]).unwrap(), Value::Null);
        assert!(split(vec![list(vec![]), s(",")]).is_err());
        assert_eq!(split(vec![s("x")]).unwrap(), Value::Null);
    }

    #[test]
    fn substring_two_arg_and_three_arg() {
        // start only
        assert_eq!(
            substring(vec![s("hello"), Value::Int(2)]).unwrap(),
            s("llo")
        );
        // start + end (exclusive)
        assert_eq!(
            substring(vec![s("hello"), Value::Int(1), Value::Int(4)]).unwrap(),
            s("ell")
        );
        // negative start
        assert_eq!(
            substring(vec![s("hello"), Value::Int(-2)]).unwrap(),
            s("lo")
        );
        // negative end
        assert_eq!(
            substring(vec![s("hello"), Value::Int(0), Value::Int(-1)]).unwrap(),
            s("hell")
        );
        // start beyond length
        assert_eq!(substring(vec![s("hi"), Value::Int(10)]).unwrap(), s(""));
        // end <= start
        assert_eq!(
            substring(vec![s("hello"), Value::Int(3), Value::Int(2)]).unwrap(),
            s("")
        );
        // Large negative end: cast wraps so result clamps to chars.len().
        let _ = substring(vec![s("hello"), Value::Int(3), Value::Int(-10)]).unwrap();
        assert_eq!(substring(vec![Value::Null]).unwrap(), Value::Null);
        assert!(substring(vec![list(vec![])]).is_err());
        assert_eq!(substring(vec![Value::Int(1)]).unwrap(), Value::Null);
    }

    #[test]
    fn substring_local_variants() {
        let r = substring_local(vec![
            list(vec![s("hello"), s("world"), Value::Int(7)]),
            Value::Int(1),
            Value::Int(3),
        ])
        .unwrap();
        assert_eq!(r, list(vec![s("el"), s("or"), Value::Int(7)]));
        let r = substring_local(vec![list(vec![s("foo")]), Value::Int(-2)]).unwrap();
        assert_eq!(r, list(vec![s("oo")]));
        let r = substring_local(vec![list(vec![s("foo")]), Value::Int(0), Value::Int(-1)]).unwrap();
        assert_eq!(r, list(vec![s("fo")]));
        // Non-list returns Null
        assert_eq!(
            substring_local(vec![s("x"), Value::Int(0)]).unwrap(),
            Value::Null
        );
    }

    #[test]
    fn case_local_variants() {
        let r = tolower_local(vec![list(vec![s("HI"), Value::Null, Value::Int(1)])]).unwrap();
        assert_eq!(r, list(vec![s("hi"), Value::Null, Value::Int(1)]));
        let r = toupper_local(vec![list(vec![s("hi"), Value::Null])]).unwrap();
        assert_eq!(r, list(vec![s("HI"), Value::Null]));
        // Non-list delegates → returns Str/Null per gremlin_tolower/toupper rules.
        let _ = tolower_local(vec![s("AbC")]);
        let _ = toupper_local(vec![s("AbC")]);
    }

    #[test]
    fn trim_local_variants() {
        let r = trim_local(vec![list(vec![s("  hi  "), Value::Null])]).unwrap();
        assert_eq!(r, list(vec![s("hi"), Value::Null]));
        let r = ltrim_local(vec![list(vec![s("  hi  ")])]).unwrap();
        assert_eq!(r, list(vec![s("hi  ")]));
        let r = rtrim_local(vec![list(vec![s("  hi  ")])]).unwrap();
        assert_eq!(r, list(vec![s("  hi")]));
        // Type error on non-string element
        assert!(trim_local(vec![list(vec![Value::Int(1)])]).is_err());
        assert!(ltrim_local(vec![list(vec![Value::Int(1)])]).is_err());
        assert!(rtrim_local(vec![list(vec![Value::Int(1)])]).is_err());
    }

    #[test]
    fn replace_local_variants() {
        let r = replace_local(vec![
            list(vec![s("aa"), s("bb"), Value::Null, Value::Int(3)]),
            s("a"),
            s("X"),
        ])
        .unwrap();
        assert_eq!(r, list(vec![s("XX"), s("bb"), Value::Null, Value::Int(3)]));
    }

    #[test]
    fn split_local_variants() {
        let r = split_local(vec![list(vec![s("a,b"), Value::Null, s("c,d")]), s(",")]).unwrap();
        assert_eq!(
            r,
            list(vec![
                list(vec![s("a"), s("b")]),
                Value::Null,
                list(vec![s("c"), s("d")]),
            ])
        );
        // Empty separator
        let r = split_local(vec![list(vec![s("ab")]), s("")]).unwrap();
        assert_eq!(r, list(vec![list(vec![s("a"), s("b")])]));
    }

    #[test]
    fn count_local_variants() {
        assert_eq!(
            count_local(vec![list(vec![Value::Int(1), Value::Int(2)])]).unwrap(),
            Value::Int(2)
        );
        let mut m = IndexMap::<Box<str>, Value>::new();
        m.insert("k".into(), Value::Int(1));
        assert_eq!(count_local(vec![Value::Map(m)]).unwrap(), Value::Int(1));
        assert_eq!(count_local(vec![s("hello")]).unwrap(), Value::Int(5));
        assert_eq!(count_local(vec![Value::Null]).unwrap(), Value::Null);
        assert_eq!(count_local(vec![Value::Int(99)]).unwrap(), Value::Int(0));
    }

    #[test]
    fn length_local_variants() {
        let r = length_local(vec![list(vec![
            s("hi"),
            list(vec![Value::Int(1)]),
            Value::Null,
            Value::Int(0),
        ])])
        .unwrap();
        assert_eq!(
            r,
            list(vec![
                Value::Int(2),
                Value::Int(1),
                Value::Null,
                Value::Int(0)
            ])
        );
        assert_eq!(length_local(vec![s("hello")]).unwrap(), Value::Int(5));
        let mut m = IndexMap::<Box<str>, Value>::new();
        m.insert("k".into(), Value::Int(1));
        assert_eq!(length_local(vec![Value::Map(m)]).unwrap(), Value::Int(1));
        assert_eq!(length_local(vec![Value::Null]).unwrap(), Value::Null);
        assert_eq!(length_local(vec![Value::Int(99)]).unwrap(), Value::Int(0));
    }
}
