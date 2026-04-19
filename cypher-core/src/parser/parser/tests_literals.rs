//! Tests for the recursive-descent parser — literal expressions (arithmetic, lists, maps, regex, string predicates, list comprehension).
//!
//! Extracted from `parser/parser/mod.rs` to keep individual files small and topical.

#![cfg(test)]

use super::parse_query;
use crate::ast::*;
use crate::error::ParseError;

fn parse(src: &str) -> Query {
    parse_query(src).unwrap_or_else(|e| panic!("parse error for {:?}: {}", src, e))
}

#[allow(dead_code)]
fn parse_err(src: &str) -> ParseError {
    parse_query(src).expect_err("expected a parse error")
}

// ── Arithmetic expressions ────────────────────────────────────────────────

#[test]
fn arithmetic_precedence() {
    // 2 + 3 * 4 should parse as 2 + (3 * 4)
    let q = parse("RETURN 2 + 3 * 4");
    let Clause::Return(rc) = &q.clauses[0] else {
        panic!()
    };
    match &rc.items[0].expr {
        Expr::BinaryOp {
            op: BinaryOp::Add,
            right,
            ..
        } => {
            assert!(matches!(
                right.as_ref(),
                Expr::BinaryOp {
                    op: BinaryOp::Mul,
                    ..
                }
            ));
        }
        _ => panic!("expected Add at top level"),
    }
}

#[test]
fn unary_negation() {
    let q = parse("RETURN -42");
    let Clause::Return(rc) = &q.clauses[0] else {
        panic!()
    };
    assert!(matches!(
        &rc.items[0].expr,
        Expr::UnaryOp {
            op: UnaryOp::Neg,
            ..
        }
    ));
}

// ── List literals ─────────────────────────────────────────────────────────

#[test]
fn list_literal() {
    let q = parse("RETURN [1, 2, 3]");
    let Clause::Return(rc) = &q.clauses[0] else {
        panic!()
    };
    assert!(matches!(&rc.items[0].expr, Expr::List(items) if items.len() == 3));
}

#[test]
fn empty_list() {
    let q = parse("RETURN []");
    let Clause::Return(rc) = &q.clauses[0] else {
        panic!()
    };
    assert!(matches!(&rc.items[0].expr, Expr::List(items) if items.is_empty()));
}

// ── Map literal ───────────────────────────────────────────────────────────

#[test]
fn map_literal_expr() {
    let q = parse("RETURN {name: 'Alice', age: 30}");
    let Clause::Return(rc) = &q.clauses[0] else {
        panic!()
    };
    assert!(matches!(&rc.items[0].expr, Expr::MapLiteral(pairs) if pairs.len() == 2));
}

// ── Regex match ───────────────────────────────────────────────────────────

#[test]
fn regex_match_operator() {
    let q = parse("MATCH (n) WHERE n.name =~ 'A.*' RETURN n");
    let Clause::Match(mc) = &q.clauses[0] else {
        panic!()
    };
    assert!(matches!(
        mc.where_clause.as_ref().unwrap(),
        Expr::BinaryOp {
            op: BinaryOp::RegexMatch,
            ..
        }
    ));
}

// ── STARTS WITH / ENDS WITH / CONTAINS ───────────────────────────────────

#[test]
fn starts_with() {
    let q = parse("MATCH (n) WHERE n.name STARTS WITH 'A' RETURN n");
    let Clause::Match(mc) = &q.clauses[0] else {
        panic!()
    };
    assert!(matches!(
        mc.where_clause.as_ref().unwrap(),
        Expr::BinaryOp {
            op: BinaryOp::StartsWith,
            ..
        }
    ));
}

#[test]
fn ends_with() {
    let q = parse("MATCH (n) WHERE n.name ENDS WITH 'son' RETURN n");
    let Clause::Match(mc) = &q.clauses[0] else {
        panic!()
    };
    assert!(matches!(
        mc.where_clause.as_ref().unwrap(),
        Expr::BinaryOp {
            op: BinaryOp::EndsWith,
            ..
        }
    ));
}

#[test]
fn contains_predicate() {
    let q = parse("MATCH (n) WHERE n.name CONTAINS 'ali' RETURN n");
    let Clause::Match(mc) = &q.clauses[0] else {
        panic!()
    };
    assert!(matches!(
        mc.where_clause.as_ref().unwrap(),
        Expr::BinaryOp {
            op: BinaryOp::Contains,
            ..
        }
    ));
}

// ── List comprehension parsing ─────────────────────────────────────────────

#[test]
fn list_comprehension_identity() {
    // [x IN [1,2,3]] — no filter, no explicit map (variable itself)
    let q = parse("RETURN [x IN [1, 2, 3]]");
    let Clause::Return(rc) = &q.clauses[0] else {
        panic!()
    };
    match &rc.items[0].expr {
        Expr::ListComp {
            variable, filter, ..
        } => {
            assert_eq!(variable.0, "x");
            assert!(filter.is_none());
        }
        other => panic!("expected ListComp, got {other:?}"),
    }
}

#[test]
fn list_comprehension_with_filter_and_map() {
    // [x IN range(1,10) WHERE x > 5 | x * 2]
    let q = parse("RETURN [x IN range(1, 10) WHERE x > 5 | x * 2]");
    let Clause::Return(rc) = &q.clauses[0] else {
        panic!()
    };
    match &rc.items[0].expr {
        Expr::ListComp {
            variable,
            filter,
            map,
            ..
        } => {
            assert_eq!(variable.0, "x");
            assert!(filter.is_some(), "expected a filter");
            assert!(
                matches!(**map, Expr::BinaryOp { .. }),
                "expected BinaryOp map expr"
            );
        }
        other => panic!("expected ListComp, got {other:?}"),
    }
}
