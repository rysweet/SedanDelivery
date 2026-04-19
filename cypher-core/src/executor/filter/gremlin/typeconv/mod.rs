//! Gremlin typeconv functions extracted from filter::eval_function_with_vals.
//!
//! Behavior preserved verbatim from the original `filter` module.

#![allow(clippy::needless_pass_by_value)]

mod boolean;
mod date;
mod number;
mod string;
mod typeof_;

pub(crate) use boolean::asbool;
pub(crate) use date::asdate;
pub(crate) use number::{asnumber, asnumber_typed};
pub(crate) use string::{asstring, asstring_local};
pub(crate) use typeof_::type_of;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{EdgeId, NodeId};
    use crate::types::Value;
    use indexmap::IndexMap;

    fn s(v: &str) -> Value {
        Value::Str(v.into())
    }

    #[test]
    fn asstring_scalars_and_specials() {
        assert_eq!(asstring(vec![s("hi")]).unwrap(), s("hi"));
        assert_eq!(asstring(vec![Value::Int(42)]).unwrap(), s("42"));
        assert_eq!(asstring(vec![Value::Float(1.5)]).unwrap(), s("1.5"));
        assert_eq!(asstring(vec![Value::Float(f64::NAN)]).unwrap(), s("NaN"));
        assert_eq!(
            asstring(vec![Value::Float(f64::INFINITY)]).unwrap(),
            s("Infinity")
        );
        assert_eq!(
            asstring(vec![Value::Float(f64::NEG_INFINITY)]).unwrap(),
            s("-Infinity")
        );
        assert_eq!(asstring(vec![Value::Bool(true)]).unwrap(), s("true"));
        assert!(asstring(vec![Value::Null]).is_err());
        assert_eq!(asstring(vec![]).unwrap(), Value::Null);
        assert_eq!(asstring(vec![Value::Node(NodeId(7))]).unwrap(), s("v[7]"));
        assert_eq!(asstring(vec![Value::Edge(EdgeId(9))]).unwrap(), s("e[9]"));
    }

    #[test]
    fn asstring_list_and_map_and_vertex_property() {
        let r = asstring(vec![Value::List(vec![
            s("a"),
            Value::Int(1),
            Value::Float(2.5),
            Value::Bool(false),
            Value::Null,
        ])])
        .unwrap();
        assert_eq!(r, s("[a, 1, 2.5, false, null]"));
        // Vertex-property map
        let mut vp = IndexMap::<Box<str>, Value>::new();
        vp.insert("~key".into(), s("name"));
        vp.insert("~value".into(), s("alice"));
        assert_eq!(
            asstring(vec![Value::Map(vp)]).unwrap(),
            s("vp[name->alice]")
        );
        // Regular map with mixed types
        let mut m = IndexMap::<Box<str>, Value>::new();
        m.insert("a".into(), Value::Int(1));
        m.insert("b".into(), Value::List(vec![Value::Int(2), s("x")]));
        m.insert("~hidden".into(), s("skip"));
        let out = asstring(vec![Value::Map(m)]).unwrap();
        if let Value::Str(out) = out {
            assert!(out.contains("a=1"));
            assert!(out.contains("b=["));
            assert!(!out.contains("~hidden"));
        } else {
            panic!("expected str");
        }
    }

    #[test]
    fn asstring_path() {
        let v = Value::Path {
            nodes: vec![NodeId(1), NodeId(2)],
            edges: vec![EdgeId(10)],
        };
        assert_eq!(asstring(vec![v]).unwrap(), s("path[v[1], v[2], e[10]]"));
    }

    #[test]
    fn asdate_variants() {
        let r = asdate(vec![Value::Int(1700000000000)]).unwrap();
        assert!(matches!(r, Value::DateTime(_)));
        let r = asdate(vec![Value::Float(1700000000000.0)]).unwrap();
        assert!(matches!(r, Value::DateTime(_)));
        assert_eq!(asdate(vec![Value::Null]).unwrap(), Value::Null);
        assert_eq!(asdate(vec![s("nope")]).unwrap(), Value::Null);
        // round-trip DateTime
        if let Value::DateTime(d) = asdate(vec![Value::Int(1)]).unwrap() {
            let r2 = asdate(vec![Value::DateTime(d.clone())]).unwrap();
            assert!(matches!(r2, Value::DateTime(_)));
        }
    }

    #[test]
    fn asstring_local_variants() {
        // List
        let r = asstring_local(vec![Value::List(vec![
            s("a"),
            Value::Int(1),
            Value::Bool(true),
        ])])
        .unwrap();
        assert_eq!(r, Value::List(vec![s("a"), s("1"), s("true")]));
        // Scalar
        assert_eq!(asstring_local(vec![Value::Int(7)]).unwrap(), s("7"));
        // Vertex-property map
        let mut vp = IndexMap::<Box<str>, Value>::new();
        vp.insert("~key".into(), s("k"));
        vp.insert("~value".into(), Value::Int(5));
        assert_eq!(asstring_local(vec![Value::Map(vp)]).unwrap(), s("vp[k->5]"));
        // Empty input
        assert_eq!(asstring_local(vec![]).unwrap(), Value::Null);
        // Null inside list propagates error
        assert!(asstring_local(vec![Value::List(vec![Value::Null])]).is_err());
    }

    #[test]
    fn asnumber_basics() {
        assert_eq!(asnumber(vec![Value::Int(5)]).unwrap(), Value::Int(5));
        assert_eq!(
            asnumber(vec![Value::Float(2.5)]).unwrap(),
            Value::Float(2.5)
        );
        assert_eq!(asnumber(vec![s("42")]).unwrap(), Value::Int(42));
        assert_eq!(asnumber(vec![s("0xff")]).unwrap(), Value::Int(255));
        assert_eq!(asnumber(vec![s("0X10")]).unwrap(), Value::Int(16));
        assert_eq!(asnumber(vec![s("2.5")]).unwrap(), Value::Float(2.5));
        assert!(asnumber(vec![s("foo")]).is_err());
        assert_eq!(asnumber(vec![Value::Bool(true)]).unwrap(), Value::Int(1));
        assert_eq!(asnumber(vec![Value::Bool(false)]).unwrap(), Value::Int(0));
        assert_eq!(asnumber(vec![Value::Null]).unwrap(), Value::Null);
        assert!(asnumber(vec![Value::List(vec![])]).is_err());
        assert_eq!(asnumber(vec![]).unwrap(), Value::Null);
    }

    #[test]
    fn asnumber_typed_combinations() {
        // byte ok and overflow
        assert_eq!(
            asnumber_typed(vec![Value::Int(5), s("byte")]).unwrap(),
            Value::Int(5)
        );
        assert!(asnumber_typed(vec![Value::Int(1000), s("byte")]).is_err());
        // short ok and overflow
        assert_eq!(
            asnumber_typed(vec![Value::Int(5), s("short")]).unwrap(),
            Value::Int(5)
        );
        assert!(asnumber_typed(vec![Value::Int(40000), s("short")]).is_err());
        // float -> int
        assert_eq!(
            asnumber_typed(vec![Value::Float(2.7), s("int")]).unwrap(),
            Value::Int(2)
        );
        // int -> float
        assert_eq!(
            asnumber_typed(vec![Value::Int(3), s("float")]).unwrap(),
            Value::Float(3.0)
        );
        // string -> int / float
        assert_eq!(
            asnumber_typed(vec![s("42"), s("int")]).unwrap(),
            Value::Int(42)
        );
        assert_eq!(
            asnumber_typed(vec![s("2.5"), s("int")]).unwrap(),
            Value::Int(2)
        );
        assert!(asnumber_typed(vec![s("nope"), s("int")]).is_err());
        assert_eq!(
            asnumber_typed(vec![s("2.5"), s("float")]).unwrap(),
            Value::Float(2.5)
        );
        assert!(asnumber_typed(vec![s("nope"), s("float")]).is_err());
        // string -> byte / short
        assert_eq!(
            asnumber_typed(vec![s("5"), s("byte")]).unwrap(),
            Value::Int(5)
        );
        assert!(asnumber_typed(vec![s("9999"), s("byte")]).is_err());
        assert!(asnumber_typed(vec![s("nan"), s("byte")]).is_err());
        assert_eq!(
            asnumber_typed(vec![s("5"), s("short")]).unwrap(),
            Value::Int(5)
        );
        assert!(asnumber_typed(vec![s("999999"), s("short")]).is_err());
        assert!(asnumber_typed(vec![s("nope"), s("short")]).is_err());
        // float -> byte / short including overflow
        assert_eq!(
            asnumber_typed(vec![Value::Float(5.0), s("byte")]).unwrap(),
            Value::Int(5)
        );
        assert!(asnumber_typed(vec![Value::Float(500.0), s("byte")]).is_err());
        assert_eq!(
            asnumber_typed(vec![Value::Float(5.0), s("short")]).unwrap(),
            Value::Int(5)
        );
        assert!(asnumber_typed(vec![Value::Float(50000.0), s("short")]).is_err());
        // invalid token branch
        assert!(asnumber_typed(vec![Value::Int(1), s("__invalid__")]).is_err());
        // fall-through for unknown type tag with int/float/null
        assert_eq!(
            asnumber_typed(vec![Value::Int(7), s("custom")]).unwrap(),
            Value::Int(7)
        );
        assert_eq!(
            asnumber_typed(vec![Value::Float(2.0), s("custom")]).unwrap(),
            Value::Float(2.0)
        );
        assert_eq!(
            asnumber_typed(vec![Value::Null, s("custom")]).unwrap(),
            Value::Null
        );
        assert!(asnumber_typed(vec![s("hi"), s("custom")]).is_err());
        // empty input
        assert_eq!(asnumber_typed(vec![]).unwrap(), Value::Null);
    }

    #[test]
    fn asbool_variants() {
        assert_eq!(asbool(vec![Value::Bool(true)]).unwrap(), Value::Bool(true));
        assert_eq!(asbool(vec![Value::Int(0)]).unwrap(), Value::Bool(false));
        assert_eq!(asbool(vec![Value::Int(1)]).unwrap(), Value::Bool(true));
        assert_eq!(asbool(vec![Value::Float(0.0)]).unwrap(), Value::Bool(false));
        assert_eq!(asbool(vec![Value::Float(2.0)]).unwrap(), Value::Bool(true));
        assert_eq!(
            asbool(vec![Value::Float(f64::NAN)]).unwrap(),
            Value::Bool(false)
        );
        assert_eq!(asbool(vec![s("true")]).unwrap(), Value::Bool(true));
        assert_eq!(asbool(vec![s("False")]).unwrap(), Value::Bool(false));
        assert!(asbool(vec![s("nope")]).is_err());
        assert!(asbool(vec![Value::Null]).is_err());
        assert!(asbool(vec![Value::List(vec![])]).is_err());
        assert!(asbool(vec![]).is_err());
    }

    #[test]
    fn type_of_variants() {
        assert_eq!(type_of(vec![Value::Int(0)]).unwrap(), s("integer"));
        assert_eq!(type_of(vec![Value::Float(0.0)]).unwrap(), s("double"));
        assert_eq!(type_of(vec![s("x")]).unwrap(), s("string"));
        assert_eq!(type_of(vec![Value::Bool(true)]).unwrap(), s("boolean"));
        assert_eq!(type_of(vec![Value::Null]).unwrap(), s("null"));
        assert_eq!(type_of(vec![Value::List(vec![])]).unwrap(), s("list"));
        assert_eq!(
            type_of(vec![Value::Map(IndexMap::new())]).unwrap(),
            s("map")
        );
        assert_eq!(type_of(vec![Value::Node(NodeId(1))]).unwrap(), s("vertex"));
        assert_eq!(type_of(vec![Value::Edge(EdgeId(2))]).unwrap(), s("edge"));
        let v = Value::Path {
            nodes: vec![NodeId(1)],
            edges: vec![],
        };
        assert_eq!(type_of(vec![v]).unwrap(), s("path"));
        assert_eq!(type_of(vec![]).unwrap(), s("unknown"));
    }
}
