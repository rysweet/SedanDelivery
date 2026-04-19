//! Tests for the recursive-descent parser — WHERE expressions, comparisons, parameters, CASE, function calls and IS NULL.
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

// ── WHERE with complex expressions ────────────────────────────────────────

#[test]
fn where_and_expression() {
    let q = parse("MATCH (n) WHERE n.age >= 18 AND n.age < 65 RETURN n");
    let Clause::Match(mc) = &q.clauses[0] else {
        panic!()
    };
    assert!(mc.where_clause.is_some());
    assert!(matches!(
        mc.where_clause.as_ref().unwrap(),
        Expr::BinaryOp {
            op: BinaryOp::And,
            ..
        }
    ));
}

#[test]
fn where_or_expression() {
    let q = parse("MATCH (n) WHERE n.age < 18 OR n.age > 65 RETURN n");
    let Clause::Match(mc) = &q.clauses[0] else {
        panic!()
    };
    assert!(matches!(
        mc.where_clause.as_ref().unwrap(),
        Expr::BinaryOp {
            op: BinaryOp::Or,
            ..
        }
    ));
}

#[test]
fn where_not_expression() {
    let q = parse("MATCH (n) WHERE NOT n.active RETURN n");
    let Clause::Match(mc) = &q.clauses[0] else {
        panic!()
    };
    assert!(matches!(
        mc.where_clause.as_ref().unwrap(),
        Expr::UnaryOp {
            op: UnaryOp::Not,
            ..
        }
    ));
}

// ── All comparison operators ──────────────────────────────────────────────

#[test]
fn comparison_eq() {
    let q = parse("MATCH (n) WHERE n.x = 1 RETURN n");
    let Clause::Match(mc) = &q.clauses[0] else {
        panic!()
    };
    assert!(matches!(
        mc.where_clause.as_ref().unwrap(),
        Expr::BinaryOp {
            op: BinaryOp::Eq,
            ..
        }
    ));
}

#[test]
fn comparison_neq() {
    let q = parse("MATCH (n) WHERE n.x <> 1 RETURN n");
    let Clause::Match(mc) = &q.clauses[0] else {
        panic!()
    };
    assert!(matches!(
        mc.where_clause.as_ref().unwrap(),
        Expr::BinaryOp {
            op: BinaryOp::Neq,
            ..
        }
    ));
}

#[test]
fn comparison_lt_lte_gt_gte() {
    for (op_str, expected_op) in [
        ("< 5", BinaryOp::Lt),
        ("<= 5", BinaryOp::Lte),
        ("> 5", BinaryOp::Gt),
        (">= 5", BinaryOp::Gte),
    ] {
        let q = parse(&format!("MATCH (n) WHERE n.x {} RETURN n", op_str));
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        match mc.where_clause.as_ref().unwrap() {
            Expr::BinaryOp { op, .. } => assert_eq!(*op, expected_op),
            _ => panic!("expected BinaryOp"),
        }
    }
}

#[test]
fn comparison_in() {
    let q = parse("MATCH (n) WHERE n.x IN [1, 2, 3] RETURN n");
    let Clause::Match(mc) = &q.clauses[0] else {
        panic!()
    };
    assert!(matches!(
        mc.where_clause.as_ref().unwrap(),
        Expr::BinaryOp {
            op: BinaryOp::In,
            ..
        }
    ));
}

// ── Parameters ────────────────────────────────────────────────────────────

#[test]
fn parameter_in_where() {
    let q = parse("MATCH (n) WHERE n.name = $name RETURN n");
    let Clause::Match(mc) = &q.clauses[0] else {
        panic!()
    };
    match mc.where_clause.as_ref().unwrap() {
        Expr::BinaryOp { right, .. } => {
            assert!(matches!(right.as_ref(), Expr::Parameter(p) if p == "name"));
        }
        _ => panic!("expected BinaryOp"),
    }
}

#[test]
fn parameter_in_create() {
    let q = parse("CREATE (n:Person {name: $name})");
    let Clause::Create(cc) = &q.clauses[0] else {
        panic!()
    };
    let Properties::Map(props) = cc.patterns[0].start.properties.as_ref().unwrap() else {
        panic!("expected Properties::Map");
    };
    assert!(matches!(&props[0].1, Expr::Parameter(p) if p == "name"));
}

#[test]
fn parameter_map_in_create() {
    let q = parse("CREATE (n:Person $create_params)");
    let Clause::Create(cc) = &q.clauses[0] else {
        panic!()
    };
    let props = cc.patterns[0].start.properties.as_ref().unwrap();
    assert!(
        matches!(props, Properties::Parameter(name) if name == "create_params"),
        "expected Properties::Parameter('create_params'), got {:?}",
        props
    );
}

#[test]
fn parameter_map_in_match() {
    let q = parse("MATCH (n:Person $match_params) RETURN n");
    let Clause::Match(mc) = &q.clauses[0] else {
        panic!()
    };
    let props = mc.patterns[0].start.properties.as_ref().unwrap();
    assert!(
        matches!(props, Properties::Parameter(name) if name == "match_params"),
        "expected Properties::Parameter('match_params'), got {:?}",
        props
    );
}

#[test]
fn parameter_map_in_merge() {
    let q = parse("MERGE (n:Person $merge_params) RETURN n");
    let Clause::Merge(mc) = &q.clauses[0] else {
        panic!()
    };
    let props = mc.pattern.start.properties.as_ref().unwrap();
    assert!(
        matches!(props, Properties::Parameter(name) if name == "merge_params"),
        "expected Properties::Parameter('merge_params'), got {:?}",
        props
    );
}

// ── CASE expression ───────────────────────────────────────────────────────

#[test]
fn case_simple() {
    let q = parse("RETURN CASE 1 WHEN 1 THEN 'one' WHEN 2 THEN 'two' ELSE 'other' END");
    let Clause::Return(rc) = &q.clauses[0] else {
        panic!()
    };
    assert!(
        matches!(&rc.items[0].expr, Expr::Case { subject: Some(_), alternatives, default: Some(_) } if alternatives.len() == 2)
    );
}

#[test]
fn case_generic() {
    let q = parse("RETURN CASE WHEN 1 = 1 THEN true ELSE false END");
    let Clause::Return(rc) = &q.clauses[0] else {
        panic!()
    };
    assert!(
        matches!(&rc.items[0].expr, Expr::Case { subject: None, alternatives, .. } if alternatives.len() == 1)
    );
}

// ── Function calls ────────────────────────────────────────────────────────

#[test]
fn count_star() {
    let q = parse("MATCH (n) RETURN count(*)");
    let Clause::Return(rc) = &q.clauses[1] else {
        panic!()
    };
    assert!(matches!(&rc.items[0].expr, Expr::FunctionCall { name, .. } if name == "count"));
}

#[test]
fn count_distinct() {
    let q = parse("MATCH (n) RETURN count(DISTINCT n)");
    let Clause::Return(rc) = &q.clauses[1] else {
        panic!()
    };
    match &rc.items[0].expr {
        Expr::FunctionCall { name, distinct, .. } => {
            assert_eq!(name, "count");
            assert!(distinct);
        }
        _ => panic!("expected FunctionCall"),
    }
}

#[test]
fn function_call_size() {
    let q = parse("MATCH (n) RETURN size(n.name)");
    let Clause::Return(rc) = &q.clauses[1] else {
        panic!()
    };
    assert!(matches!(&rc.items[0].expr, Expr::FunctionCall { name, .. } if name == "size"));
}

#[test]
fn function_call_no_args() {
    let q = parse("RETURN timestamp()");
    let Clause::Return(rc) = &q.clauses[0] else {
        panic!()
    };
    assert!(
        matches!(&rc.items[0].expr, Expr::FunctionCall { name, args, .. } if name == "timestamp" && args.is_empty())
    );
}

// ── IS NULL / IS NOT NULL ─────────────────────────────────────────────────

#[test]
fn is_null() {
    let q = parse("MATCH (n) WHERE n.name IS NULL RETURN n");
    let Clause::Match(mc) = &q.clauses[0] else {
        panic!()
    };
    assert!(matches!(mc.where_clause.as_ref().unwrap(), Expr::IsNull(_)));
}

#[test]
fn is_not_null() {
    let q = parse("MATCH (n) WHERE n.name IS NOT NULL RETURN n");
    let Clause::Match(mc) = &q.clauses[0] else {
        panic!()
    };
    assert!(matches!(
        mc.where_clause.as_ref().unwrap(),
        Expr::IsNotNull(_)
    ));
}
