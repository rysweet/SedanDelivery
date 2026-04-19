//! Tests for the recursive-descent parser — pattern syntax, ORDER BY/LIMIT/SKIP, errors and UNION.
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

// ── Label disjunction ─────────────────────────────────────────────────────

#[test]
fn label_disjunction_pipe() {
    let q = parse("MATCH (n:Person|Animal) RETURN n");
    let Clause::Match(mc) = &q.clauses[0] else {
        panic!()
    };
    let node = &mc.patterns[0].start;
    assert_eq!(node.labels.len(), 2);
    assert_eq!(node.labels[0], Label::new("Person"));
    assert_eq!(node.labels[1], Label::new("Animal"));
}

#[test]
fn label_colon_list() {
    let q = parse("MATCH (n:Person:Employee) RETURN n");
    let Clause::Match(mc) = &q.clauses[0] else {
        panic!()
    };
    assert_eq!(mc.patterns[0].start.labels.len(), 2);
}

// ── Variable-length paths ─────────────────────────────────────────────────

#[test]
fn variable_length_range() {
    let q = parse("MATCH (a)-[r:KNOWS*1..5]->(b) RETURN a");
    let Clause::Match(mc) = &q.clauses[0] else {
        panic!()
    };
    let step = &mc.patterns[0].steps[0];
    let range = step.rel.range.as_ref().unwrap();
    assert_eq!(range.min, Some(1));
    assert_eq!(range.max, Some(5));
}

#[test]
fn variable_length_star_only() {
    let q = parse("MATCH (a)-[*]->(b) RETURN a");
    let Clause::Match(mc) = &q.clauses[0] else {
        panic!()
    };
    let step = &mc.patterns[0].steps[0];
    let range = step.rel.range.as_ref().unwrap();
    assert_eq!(range.min, None);
    assert_eq!(range.max, None);
}

#[test]
fn variable_length_exact() {
    let q = parse("MATCH (a)-[*3]->(b) RETURN a");
    let Clause::Match(mc) = &q.clauses[0] else {
        panic!()
    };
    let range = mc.patterns[0].steps[0].rel.range.as_ref().unwrap();
    // *3 with no DoubleDot means exactly 3.
    assert_eq!(range.min, Some(3));
}

// ── Property maps ─────────────────────────────────────────────────────────

#[test]
fn node_with_property_map() {
    let q = parse("MATCH (n:Person {name: 'Alice', age: 30}) RETURN n");
    let Clause::Match(mc) = &q.clauses[0] else {
        panic!()
    };
    let Properties::Map(props) = mc.patterns[0].start.properties.as_ref().unwrap() else {
        panic!("expected Properties::Map");
    };
    assert_eq!(props.len(), 2);
    assert_eq!(props[0].0, "name");
    assert!(matches!(&props[0].1, Expr::Str(s) if s.as_ref() == "Alice"));
    assert_eq!(props[1].0, "age");
    assert!(matches!(&props[1].1, Expr::Integer(30)));
}

// ── Relationship direction ────────────────────────────────────────────────

#[test]
fn incoming_relationship() {
    let q = parse("MATCH (a)<-[r:KNOWS]-(b) RETURN a");
    let Clause::Match(mc) = &q.clauses[0] else {
        panic!()
    };
    assert_eq!(mc.patterns[0].steps[0].rel.direction, Direction::Incoming);
}

#[test]
fn undirected_relationship() {
    let q = parse("MATCH (a)-[r]-(b) RETURN a");
    let Clause::Match(mc) = &q.clauses[0] else {
        panic!()
    };
    assert_eq!(mc.patterns[0].steps[0].rel.direction, Direction::Undirected);
}

// ── Path variable binding ─────────────────────────────────────────────────

#[test]
fn path_variable() {
    let q = parse("MATCH p = (a)-[:R]->(b) RETURN p");
    let Clause::Match(mc) = &q.clauses[0] else {
        panic!()
    };
    assert_eq!(mc.patterns[0].variable, Some(Identifier::new("p")));
}

// ── Rel type disjunction ──────────────────────────────────────────────────

#[test]
fn rel_type_disjunction() {
    let q = parse("MATCH (a)-[r:KNOWS|LIKES]->(b) RETURN a");
    let Clause::Match(mc) = &q.clauses[0] else {
        panic!()
    };
    let rel = &mc.patterns[0].steps[0].rel;
    assert_eq!(rel.rel_types.len(), 2);
    assert_eq!(rel.rel_types[0], RelType::new("KNOWS"));
    assert_eq!(rel.rel_types[1], RelType::new("LIKES"));
}

// ── Subscript access ──────────────────────────────────────────────────────

#[test]
fn subscript_access() {
    let q = parse("RETURN list[0]");
    let Clause::Return(rc) = &q.clauses[0] else {
        panic!()
    };
    assert!(matches!(&rc.items[0].expr, Expr::Subscript(_, _)));
}

// ── ORDER BY + LIMIT + SKIP ───────────────────────────────────────────────

#[test]
fn return_order_by_limit_skip() {
    let q = parse("MATCH (n) RETURN n ORDER BY n.name SKIP 10 LIMIT 5");
    let Clause::Return(rc) = &q.clauses[1] else {
        panic!()
    };
    assert_eq!(rc.order_by.len(), 1);
    assert_eq!(rc.skip, Some(Expr::Integer(10)));
    assert_eq!(rc.limit, Some(Expr::Integer(5)));
}

#[test]
fn order_by_desc() {
    let q = parse("MATCH (n) RETURN n ORDER BY n.age DESC");
    let Clause::Return(rc) = &q.clauses[1] else {
        panic!()
    };
    assert!(!rc.order_by[0].ascending);
}

#[test]
fn order_by_multiple() {
    let q = parse("MATCH (n) RETURN n ORDER BY n.age ASC, n.name DESC");
    let Clause::Return(rc) = &q.clauses[1] else {
        panic!()
    };
    assert_eq!(rc.order_by.len(), 2);
    assert!(rc.order_by[0].ascending);
    assert!(!rc.order_by[1].ascending);
}

// ── Error cases ───────────────────────────────────────────────────────────

#[test]
fn error_unterminated_node_pattern() {
    let err = parse_err("MATCH (n");
    assert!(matches!(err, ParseError::Syntax { .. }));
}

#[test]
fn error_empty_query() {
    let err = parse_err("");
    assert!(matches!(err, ParseError::EmptyQuery));
}

#[test]
fn error_whitespace_only() {
    let err = parse_err("   ");
    assert!(matches!(err, ParseError::EmptyQuery));
}

#[test]
fn error_no_clause() {
    let err = parse_err("42");
    assert!(matches!(err, ParseError::Syntax { .. }));
}

// ── UNION / UNION ALL parsing ────────────────────────────────────────────

#[test]
fn union_basic() {
    let q = parse("RETURN 1 AS x UNION RETURN 2 AS x");
    assert_eq!(q.unions.len(), 1);
    assert!(!q.unions[0].0, "should be UNION (not ALL)");
    assert!(matches!(&q.clauses[0], Clause::Return(_)));
    assert!(matches!(&q.unions[0].1[0], Clause::Return(_)));
}

#[test]
fn union_all() {
    let q = parse("RETURN 1 AS x UNION ALL RETURN 1 AS x");
    assert_eq!(q.unions.len(), 1);
    assert!(q.unions[0].0, "should be UNION ALL");
}

#[test]
fn union_multiple_parts() {
    let q = parse("RETURN 1 AS x UNION RETURN 2 AS x UNION RETURN 3 AS x");
    assert_eq!(q.unions.len(), 2);
    assert!(!q.unions[0].0);
    assert!(!q.unions[1].0);
}

#[test]
fn union_with_match() {
    let q = parse("MATCH (n:A) RETURN n.name AS name UNION MATCH (n:B) RETURN n.name AS name");
    assert_eq!(q.clauses.len(), 2); // MATCH + RETURN
    assert_eq!(q.unions.len(), 1);
    assert_eq!(q.unions[0].1.len(), 2); // MATCH + RETURN
}
