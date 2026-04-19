//! Tests for the recursive-descent parser — clause forms (CREATE, MERGE, SET, REMOVE, UNWIND, DELETE, OPTIONAL MATCH).
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

// ── CREATE ────────────────────────────────────────────────────────────────

#[test]
fn create_single_node() {
    let q = parse("CREATE (n:Person {name: 'Bob'})");
    assert_eq!(q.clauses.len(), 1);
    let Clause::Create(cc) = &q.clauses[0] else {
        panic!()
    };
    assert_eq!(cc.patterns.len(), 1);
}

#[test]
fn create_relationship() {
    let q = parse("CREATE (a:Person)-[:KNOWS]->(b:Person)");
    let Clause::Create(cc) = &q.clauses[0] else {
        panic!()
    };
    assert_eq!(cc.patterns[0].steps.len(), 1);
    assert_eq!(cc.patterns[0].steps[0].rel.direction, Direction::Outgoing);
    assert_eq!(
        cc.patterns[0].steps[0].rel.rel_types,
        vec![RelType::new("KNOWS")]
    );
}

#[test]
fn create_multiple_patterns() {
    let q = parse("CREATE (a:A), (b:B), (a)-[:R]->(b)");
    let Clause::Create(cc) = &q.clauses[0] else {
        panic!()
    };
    assert_eq!(cc.patterns.len(), 3);
}

// ── MERGE ON CREATE SET / ON MATCH SET ────────────────────────────────────

#[test]
fn merge_on_create_set() {
    let q = parse("MERGE (n:Person {name: 'Alice'}) ON CREATE SET n.created = true");
    let Clause::Merge(mc) = &q.clauses[0] else {
        panic!()
    };
    assert_eq!(mc.on_create.len(), 1);
    assert!(mc.on_match.is_empty());
}

#[test]
fn merge_on_match_set() {
    let q = parse("MERGE (n:Person {name: 'Bob'}) ON MATCH SET n.updated = true");
    let Clause::Merge(mc) = &q.clauses[0] else {
        panic!()
    };
    assert!(mc.on_create.is_empty());
    assert_eq!(mc.on_match.len(), 1);
}

#[test]
fn merge_both_actions() {
    let q = parse("MERGE (n:Person {name: 'Carol'}) ON CREATE SET n.created = true ON MATCH SET n.updated = true");
    let Clause::Merge(mc) = &q.clauses[0] else {
        panic!()
    };
    assert_eq!(mc.on_create.len(), 1);
    assert_eq!(mc.on_match.len(), 1);
}

// ── SET and REMOVE ────────────────────────────────────────────────────────

#[test]
fn set_property() {
    let q = parse("MATCH (n) SET n.name = 'Alice'");
    let Clause::Set(sc) = &q.clauses[1] else {
        panic!()
    };
    assert!(matches!(&sc.items[0], SetClauseItem::SetProperty { prop, .. } if prop == "name"));
}

#[test]
fn set_label() {
    let q = parse("MATCH (n) SET n:Person");
    let Clause::Set(sc) = &q.clauses[1] else {
        panic!()
    };
    assert!(
        matches!(&sc.items[0], SetClauseItem::SetLabels { labels, .. } if labels[0] == Label::new("Person"))
    );
}

#[test]
fn remove_property() {
    let q = parse("MATCH (n) REMOVE n.name");
    let Clause::Remove(rc) = &q.clauses[1] else {
        panic!()
    };
    assert!(
        matches!(&rc.items[0], RemoveClauseItem::RemoveProperty { prop, .. } if prop == "name")
    );
}

#[test]
fn remove_label() {
    let q = parse("MATCH (n:Person) REMOVE n:Person");
    let Clause::Remove(rc) = &q.clauses[1] else {
        panic!()
    };
    assert!(
        matches!(&rc.items[0], RemoveClauseItem::RemoveLabels { labels, .. } if !labels.is_empty())
    );
}

// ── UNWIND ────────────────────────────────────────────────────────────────

#[test]
fn unwind_clause() {
    let q = parse("UNWIND [1, 2, 3] AS x RETURN x");
    assert!(matches!(&q.clauses[0], Clause::Unwind(_)));
    let Clause::Unwind(uc) = &q.clauses[0] else {
        panic!()
    };
    assert_eq!(uc.alias, Identifier::new("x"));
}

// ── DELETE ────────────────────────────────────────────────────────────────

#[test]
fn delete_node() {
    let q = parse("MATCH (n) DELETE n");
    let Clause::Delete(dc) = &q.clauses[1] else {
        panic!()
    };
    assert!(!dc.detach);
    assert_eq!(dc.exprs.len(), 1);
}

#[test]
fn detach_delete() {
    let q = parse("MATCH (n) DETACH DELETE n");
    let Clause::Delete(dc) = &q.clauses[1] else {
        panic!()
    };
    assert!(dc.detach);
}

// ── OPTIONAL MATCH ────────────────────────────────────────────────────────

#[test]
fn optional_match() {
    let q = parse("MATCH (n) OPTIONAL MATCH (n)-[r]->(m) RETURN m");
    assert!(matches!(&q.clauses[1], Clause::OptionalMatch(_)));
}

// ── Multiple path steps ───────────────────────────────────────────────────

#[test]
fn multi_step_path() {
    let q = parse("MATCH (a)-[:R1]->(b)-[:R2]->(c) RETURN c");
    let Clause::Match(mc) = &q.clauses[0] else {
        panic!()
    };
    assert_eq!(mc.patterns[0].steps.len(), 2);
}
