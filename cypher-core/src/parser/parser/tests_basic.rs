//! Tests for the recursive-descent parser — basic MATCH/RETURN and WITH chaining.
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

// ── Basic MATCH/RETURN ────────────────────────────────────────────────────

#[test]
fn simple_match_return() {
    let q = parse("MATCH (n) RETURN n");
    assert_eq!(q.clauses.len(), 2);
    assert!(matches!(&q.clauses[0], Clause::Match(_)));
    assert!(matches!(&q.clauses[1], Clause::Return(_)));
}

#[test]
fn match_with_label() {
    let q = parse("MATCH (n:Person) RETURN n");
    let Clause::Match(mc) = &q.clauses[0] else {
        panic!()
    };
    let node = &mc.patterns[0].start;
    assert_eq!(node.labels, vec![Label::new("Person")]);
}

#[test]
fn match_return_property() {
    let q = parse("MATCH (n:Person) RETURN n.name");
    let Clause::Return(rc) = &q.clauses[1] else {
        panic!()
    };
    assert!(matches!(&rc.items[0].expr, Expr::Property(_, key) if key == "name"));
}

#[test]
fn return_with_alias() {
    let q = parse("MATCH (n) RETURN n.name AS personName");
    let Clause::Return(rc) = &q.clauses[1] else {
        panic!()
    };
    assert_eq!(rc.items[0].alias, Some(Identifier::new("personName")));
}

#[test]
fn return_distinct() {
    let q = parse("MATCH (n:Person) RETURN DISTINCT n.name");
    let Clause::Return(rc) = &q.clauses[1] else {
        panic!()
    };
    assert!(rc.distinct);
}

// ── WITH chaining ─────────────────────────────────────────────────────────

#[test]
fn with_chain() {
    let q = parse("MATCH (n) WITH n MATCH (n)-[r]->(m) RETURN m");
    assert_eq!(q.clauses.len(), 4);
    assert!(matches!(&q.clauses[1], Clause::With(_)));
}

#[test]
fn with_where() {
    let q = parse("MATCH (n) WITH n WHERE n.age > 18 RETURN n");
    let Clause::With(wc) = &q.clauses[1] else {
        panic!()
    };
    assert!(wc.where_clause.is_some());
}
