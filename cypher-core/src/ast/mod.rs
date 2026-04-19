//! Abstract Syntax Tree types for the GQL/Cypher engine.
//!
//! All heap-allocated string data uses [`smol_str::SmolStr`] for labels, relationship types,
//! and property keys (inline storage for strings up to 22 bytes, heap otherwise). Full
//! identifiers use `String` since they can be arbitrarily long.
//!
//! AST types are organized into topic submodules:
//! - [`common`] — source spans, identifier newtypes, relationship direction.
//! - [`expr`] — expressions and operators.
//! - [`pattern`] — node, relationship, and path patterns.
//! - [`clause`] — top-level clauses and clause bodies.
//!
//! All public items are re-exported here for backward compatibility.

pub mod clause;
pub mod common;
pub mod expr;
pub mod pattern;

pub use clause::{
    CallClause, Clause, CreateClause, DeleteClause, ForeachClause, LoadCsvClause, MatchClause,
    MergeClause, Query, RemoveClause, RemoveClauseItem, ReturnClause, ReturnItem, SetClause,
    SetClauseItem, SortItem, UnwindClause, WithClause,
};
pub use common::{Direction, Identifier, Label, RelType, Span, Spanned};
pub use expr::{BinaryOp, Expr, UnaryOp};
pub use pattern::{
    MapLiteral, NodePattern, PathPattern, PathStep, Properties, RangeQuantifier, RelPattern,
    ShortestPathMode,
};

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use smol_str::SmolStr;

    #[test]
    fn span_len_and_is_empty() {
        let s = Span::new(3, 7);
        assert_eq!(s.len(), 4);
        assert!(!s.is_empty());

        let z = Span::zero();
        assert_eq!(z.len(), 0);
        assert!(z.is_empty());
    }

    #[test]
    fn spanned_deref() {
        let s: Spanned<i32> = Spanned::new(42, Span::zero());
        assert_eq!(*s, 42);
    }

    #[test]
    fn identifier_eq_hash() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(Identifier::new("foo"));
        assert!(set.contains(&Identifier::new("foo")));
        assert!(!set.contains(&Identifier::new("bar")));
    }

    #[test]
    fn label_eq_hash() {
        use std::collections::HashSet;
        let l1 = Label::new("Person");
        let l2 = Label::from("Person");
        let mut set = HashSet::new();
        set.insert(l1.clone());
        assert!(set.contains(&l2));
    }

    #[test]
    fn rel_type_from_string() {
        let r = RelType::from(String::from("KNOWS"));
        assert_eq!(r.as_str(), "KNOWS");
    }

    #[test]
    fn direction_variants_are_distinct() {
        assert_ne!(Direction::Outgoing, Direction::Incoming);
        assert_ne!(Direction::Incoming, Direction::Undirected);
    }

    #[test]
    fn expr_is_literal() {
        assert!(Expr::Integer(1).is_literal());
        assert!(Expr::Float(1.23456).is_literal());
        assert!(Expr::str_lit("hello").is_literal());
        assert!(Expr::Bool(false).is_literal());
        assert!(Expr::Null.is_literal());
        assert!(!Expr::Variable(Identifier::new("n")).is_literal());
    }

    #[test]
    fn binary_op_eq() {
        assert_eq!(BinaryOp::Add, BinaryOp::Add);
        assert_ne!(BinaryOp::Add, BinaryOp::Sub);
    }

    #[test]
    fn expr_property_access() {
        let expr = Expr::Property(
            Box::new(Expr::Variable(Identifier::new("n"))),
            SmolStr::new("name"),
        );
        match expr {
            Expr::Property(inner, key) => {
                assert_eq!(key, "name");
                assert!(matches!(*inner, Expr::Variable(_)));
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn expr_function_call() {
        let expr = Expr::FunctionCall {
            name: SmolStr::new("count"),
            args: vec![Expr::Variable(Identifier::new("n"))],
            distinct: true,
        };
        match expr {
            Expr::FunctionCall {
                name,
                args,
                distinct,
            } => {
                assert_eq!(name, "count");
                assert!(distinct);
                assert_eq!(args.len(), 1);
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn expr_map_literal() {
        let expr = Expr::MapLiteral(vec![
            (SmolStr::new("name"), Expr::str_lit("Alice")),
            (SmolStr::new("age"), Expr::Integer(30)),
        ]);
        match expr {
            Expr::MapLiteral(pairs) => {
                assert_eq!(pairs.len(), 2);
                assert_eq!(pairs[0].0, "name");
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn expr_binary_op_structure() {
        let expr = Expr::BinaryOp {
            op: BinaryOp::Add,
            left: Box::new(Expr::Integer(1)),
            right: Box::new(Expr::Integer(2)),
        };
        match &expr {
            Expr::BinaryOp { op, left, right } => {
                assert_eq!(*op, BinaryOp::Add);
                assert!(matches!(**left, Expr::Integer(1)));
                assert!(matches!(**right, Expr::Integer(2)));
            }
            _ => panic!("unexpected variant"),
        }
    }
}
