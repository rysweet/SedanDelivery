//! Coverage-focused tests for `compiler/steps/filter.rs`.

use cypher_core::planner::Operator;

use super::GremlinCompiler;
use crate::text_parser::parse_traversal;
use crate::types::GremlinValue;

fn no_param(_: &str) -> Option<GremlinValue> {
    None
}

fn compile(input: &str) -> Operator {
    let steps = parse_traversal(input, &no_param).expect("parse_traversal");
    GremlinCompiler::new()
        .compile(&steps)
        .expect("compile should succeed")
}

// ── has variants ───────────────────────────────────────────────────────

#[test]
fn filter_has_key_only() {
    let _ = compile("g.V().has(\"name\")");
}

#[test]
fn filter_has_key_value() {
    let _ = compile("g.V().has(\"name\", \"marko\")");
}

#[test]
fn filter_has_key_predicate() {
    let _ = compile("g.V().has(\"age\", gt(20))");
}

#[test]
fn filter_has_key_between_predicate() {
    let _ = compile("g.V().has(\"age\", between(20, 40))");
}

#[test]
fn filter_has_label_single() {
    let _ = compile("g.V().hasLabel(\"person\")");
}

#[test]
fn filter_has_label_multi() {
    let _ = compile("g.V().hasLabel(\"person\", \"software\")");
}

#[test]
fn filter_has_label_then_has_key() {
    let _ = compile("g.V().hasLabel(\"person\").has(\"age\", gt(20))");
}

#[test]
fn filter_has_not_key() {
    let _ = compile("g.V().hasNot(\"missing\")");
}

#[test]
fn filter_has_id_single() {
    let _ = compile("g.V().hasId(1)");
}

#[test]
fn filter_has_id_multi() {
    let _ = compile("g.V().hasId(1, 2, 3)");
}

#[test]
fn filter_has_key_eq_predicate() {
    let _ = compile("g.V().has(\"age\", eq(29))");
}

// ── where ──────────────────────────────────────────────────────────────

#[test]
fn filter_where_with_predicate() {
    let _ = compile("g.V().as(\"a\").out().where(neq(\"a\"))");
}

#[test]
fn filter_where_with_traversal() {
    let _ = compile("g.V().where(out().count().is(gt(0)))");
}

#[test]
fn filter_where_with_two_keys_and_predicate() {
    let _ = compile("g.V().as(\"a\").out().as(\"b\").where(\"a\", neq(\"b\"))");
}

// ── filter(traversal) / is(predicate) ──────────────────────────────────

#[test]
fn filter_traversal_with_is_gt() {
    let _ = compile("g.V().filter(values(\"age\").is(gt(5)))");
}

#[test]
fn filter_is_with_eq() {
    let _ = compile("g.V().values(\"age\").is(eq(29))");
}

#[test]
fn filter_is_with_within() {
    let _ = compile("g.V().values(\"age\").is(within(29, 30))");
}

// ── not / and / or ─────────────────────────────────────────────────────

#[test]
fn filter_not_step_with_has() {
    let _ = compile("g.V().not(has(\"name\", \"marko\"))");
}

#[test]
fn filter_and_step_with_two_haves() {
    let _ = compile("g.V().and(has(\"age\", gt(20)), has(\"name\"))");
}

#[test]
fn filter_or_step_with_two_haves() {
    let _ = compile("g.V().or(has(\"age\", lt(20)), has(\"age\", gt(40)))");
}

// ── hasKey / hasValue / hasValuePredicate ──────────────────────────────

#[test]
fn filter_has_key_step() {
    let _ = compile("g.V().properties().hasKey(\"name\")");
}

#[test]
fn filter_has_value_step() {
    let _ = compile("g.V().properties().hasValue(\"marko\")");
}

#[test]
fn filter_has_value_predicate() {
    let _ = compile("g.V().properties(\"age\").hasValue(gt(30))");
}
