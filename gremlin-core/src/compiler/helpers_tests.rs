//! Coverage-focused tests for `compiler/helpers.rs`.
//!
//! Each test compiles a traversal end-to-end through the public
//! `parse_traversal` + `GremlinCompiler::compile` API. Helpers in
//! `helpers.rs` are exercised indirectly via different traversal shapes
//! (path tracking, label stacks, predicate construction, sack injection,
//! aggregate prescans, etc.). Tests intentionally use loose structural
//! assertions to remain robust across plan refactors.

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

fn compile_dbg(input: &str) -> String {
    format!("{:?}", compile(input))
}

// ── steps_need_path_tracking / label_stacks ─────────────────────────────

#[test]
fn helpers_path_step_enables_path_tracking() {
    let s = compile_dbg("g.V().out(\"knows\").path()");
    assert!(s.contains("Project") || s.contains("path"));
}

#[test]
fn helpers_simple_path_enables_path_tracking() {
    let _ = compile("g.V().out().simplePath()");
}

#[test]
fn helpers_cyclic_path_enables_path_tracking() {
    let _ = compile("g.V().out().cyclicPath()");
}

#[test]
fn helpers_select_pop_first_enables_label_stacks() {
    let _ = compile("g.V().as(\"a\").out().as(\"a\").select(Pop.first, \"a\")");
}

#[test]
fn helpers_select_pop_last_enables_label_stacks() {
    let _ = compile("g.V().as(\"a\").out().as(\"a\").select(Pop.last, \"a\")");
}

#[test]
fn helpers_select_pop_all_enables_label_stacks() {
    let _ = compile("g.V().as(\"a\").out().as(\"a\").select(Pop.all, \"a\")");
}

#[test]
fn helpers_select_pop_mixed_enables_label_stacks() {
    let _ = compile("g.V().as(\"a\").out().as(\"a\").select(Pop.mixed, \"a\")");
}

#[test]
fn helpers_path_with_by_traversal() {
    let _ = compile("g.V().out().path().by(\"name\")");
}

// ── aggregate prescan / withSideEffect / cap ────────────────────────────

#[test]
fn helpers_aggregate_then_cap_prescan() {
    let _ = compile("g.V().aggregate(\"x\").cap(\"x\")");
}

#[test]
fn helpers_groupcount_prescan() {
    let s = compile_dbg("g.V().groupCount(\"g\").cap(\"g\")");
    assert!(s.contains("Cap") || s.contains("group"));
}

#[test]
fn helpers_with_side_effect_init_value() {
    let _ = compile("g.withSideEffect(\"a\", 0).V().count()");
}

#[test]
fn helpers_with_side_effect_set_init() {
    // initialise side-effect with a Set ([] in text Gremlin)
    let _ = compile("g.withSideEffect(\"a\", []).V().aggregate(\"a\").cap(\"a\")");
}

#[test]
fn helpers_aggregate_local_then_cap() {
    let _ = compile("g.V().aggregate(local, \"a\").cap(\"a\")");
}

// ── predicate-to-expr (eq/neq/gt/gte/lt/lte/within/without/between/inside) ─

#[test]
fn helpers_predicate_eq() {
    let _ = compile("g.V().has(\"age\", eq(29))");
}

#[test]
fn helpers_predicate_neq() {
    let _ = compile("g.V().has(\"age\", neq(29))");
}

#[test]
fn helpers_predicate_gt() {
    let _ = compile("g.V().has(\"age\", gt(29))");
}

#[test]
fn helpers_predicate_gte() {
    let _ = compile("g.V().has(\"age\", gte(29))");
}

#[test]
fn helpers_predicate_lt() {
    let _ = compile("g.V().has(\"age\", lt(29))");
}

#[test]
fn helpers_predicate_lte() {
    let _ = compile("g.V().has(\"age\", lte(29))");
}

#[test]
fn helpers_predicate_within_list() {
    let _ = compile("g.V().has(\"age\", within(29, 30, 31))");
}

#[test]
fn helpers_predicate_without_list() {
    let _ = compile("g.V().has(\"age\", without(29, 30))");
}

#[test]
fn helpers_predicate_between() {
    let _ = compile("g.V().has(\"age\", between(20, 40))");
}

#[test]
fn helpers_predicate_inside() {
    let _ = compile("g.V().has(\"age\", inside(20, 40))");
}

#[test]
fn helpers_predicate_starting_with() {
    let _ = compile("g.V().has(\"name\", startingWith(\"m\"))");
}

#[test]
fn helpers_predicate_ending_with() {
    let _ = compile("g.V().has(\"name\", endingWith(\"o\"))");
}

#[test]
fn helpers_predicate_containing() {
    let _ = compile("g.V().has(\"name\", containing(\"ar\"))");
}

#[test]
fn helpers_predicate_not_containing() {
    let _ = compile("g.V().has(\"name\", notContaining(\"x\"))");
}

#[test]
fn helpers_predicate_not_starting_with() {
    let _ = compile("g.V().has(\"name\", notStartingWith(\"m\"))");
}

#[test]
fn helpers_predicate_not_ending_with() {
    let _ = compile("g.V().has(\"name\", notEndingWith(\"o\"))");
}

#[test]
fn helpers_predicate_regex() {
    let _ = compile("g.V().has(\"name\", regex(\"^m.*\"))");
}

#[test]
fn helpers_predicate_not_regex() {
    let _ = compile("g.V().has(\"name\", notRegex(\"^m.*\"))");
}

#[test]
fn helpers_predicate_and_combined() {
    let _ = compile("g.V().has(\"age\", and(gt(20), lt(40)))");
}

#[test]
fn helpers_predicate_or_combined() {
    let _ = compile("g.V().has(\"age\", or(lt(20), gt(40)))");
}

// ── has-id / id key recognition ────────────────────────────────────────

#[test]
fn helpers_has_id_single() {
    let _ = compile("g.V().hasId(1)");
}

#[test]
fn helpers_has_id_multi() {
    let _ = compile("g.V().hasId(1, 2, 3)");
}

#[test]
fn helpers_has_id_predicate() {
    let _ = compile("g.V().hasId(within(1, 2, 3))");
}

#[test]
fn helpers_has_label_then_property() {
    let _ = compile("g.V().hasLabel(\"person\").has(\"age\", gt(20))");
}

// ── infix connective preprocessing (and/or step forms) ─────────────────

#[test]
fn helpers_and_step_infix() {
    let _ = compile("g.V().and(has(\"age\", gt(20)), has(\"name\"))");
}

#[test]
fn helpers_or_step_infix() {
    let _ = compile("g.V().or(has(\"age\", gt(20)), has(\"name\"))");
}

#[test]
fn helpers_not_step() {
    let _ = compile("g.V().not(has(\"name\", \"marko\"))");
}

// ── sack injection paths ────────────────────────────────────────────────

#[test]
fn helpers_with_sack_init_constant() {
    let _ = compile("g.withSack(0).V().sack()");
}

#[test]
fn helpers_with_sack_op_sum() {
    let _ = compile("g.withSack(0).V().sack(sum).by(\"age\")");
}

#[test]
fn helpers_with_sack_op_assign() {
    let _ = compile("g.withSack(0).V().sack(assign).by(\"age\")");
}

// ── group / fold / unfold ───────────────────────────────────────────────

#[test]
fn helpers_group_with_by() {
    let _ = compile("g.V().group().by(\"name\")");
}

#[test]
fn helpers_group_with_by_count() {
    let _ = compile("g.V().group().by(\"name\").by(count())");
}

#[test]
fn helpers_fold_then_unfold() {
    let _ = compile("g.V().fold().unfold()");
}

#[test]
fn helpers_fold_with_seed_sum() {
    let _ = compile("g.V().values(\"age\").fold(0, sum)");
}

// ── extract_math_variables coverage via math() ─────────────────────────

#[test]
fn helpers_math_simple() {
    let _ = compile("g.V().math(\"_ + 1\")");
}

#[test]
fn helpers_math_two_variables() {
    let _ = compile("g.V().as(\"a\").out().as(\"b\").math(\"a + b\").by(\"age\")");
}

// ── select traversal label resolution ───────────────────────────────────

#[test]
fn helpers_select_single_label() {
    let _ = compile("g.V().as(\"a\").select(\"a\")");
}

#[test]
fn helpers_select_multi_label() {
    let _ = compile("g.V().as(\"a\").out().as(\"b\").select(\"a\", \"b\")");
}

#[test]
fn helpers_select_with_by_key() {
    let _ = compile("g.V().as(\"a\").out().as(\"b\").select(\"a\", \"b\").by(\"name\")");
}

// ── value-to-expr (different GremlinValue variants) ─────────────────────

#[test]
fn helpers_value_string() {
    let _ = compile("g.V().has(\"name\", \"marko\")");
}

#[test]
fn helpers_value_int() {
    let _ = compile("g.V().has(\"age\", 29)");
}

#[test]
fn helpers_value_double() {
    let _ = compile("g.V().has(\"weight\", 0.5)");
}

#[test]
fn helpers_value_bool() {
    let _ = compile("g.V().has(\"active\", true)");
}

// ── where / by modulator targets ────────────────────────────────────────

#[test]
fn helpers_where_with_by() {
    let _ = compile("g.V().as(\"a\").out().where(eq(\"a\")).by(\"name\")");
}

#[test]
fn helpers_where_with_two_bys() {
    let _ = compile("g.V().as(\"a\").out().where(\"a\", eq(\"b\")).by(\"name\").by(\"name\")");
}

// ── reserved keys / read-only / edge label verification strategies ──────

#[test]
fn helpers_read_only_strategy_passthrough() {
    let _ = compile("g.withStrategies(ReadOnlyStrategy).V().count()");
}

#[test]
fn helpers_edge_label_verify_passthrough() {
    let _ = compile(
        "g.withStrategies(new EdgeLabelVerificationStrategy(throwException: true)).V().out(\"knows\")",
    );
}

#[test]
fn helpers_productive_by_strategy_passthrough() {
    let _ = compile("g.withStrategies(ProductiveByStrategy).V().group().by(\"name\")");
}

// ── coalesce / choose / union (branching helpers) ──────────────────────

#[test]
fn helpers_coalesce_two_branches() {
    let _ = compile("g.V().coalesce(values(\"name\"), constant(\"none\"))");
}

#[test]
fn helpers_choose_two_branches() {
    let _ = compile("g.V().choose(has(\"name\"), values(\"name\"), constant(\"none\"))");
}

#[test]
fn helpers_union_two_branches() {
    let _ = compile("g.V().union(values(\"name\"), values(\"age\"))");
}

// ── repeat / times / until / emit ───────────────────────────────────────

#[test]
fn helpers_repeat_times() {
    let _ = compile("g.V().repeat(out()).times(2)");
}

#[test]
fn helpers_repeat_until() {
    let _ = compile("g.V().repeat(out()).until(has(\"name\", \"x\"))");
}

#[test]
fn helpers_repeat_emit() {
    let _ = compile("g.V().repeat(out()).times(2).emit()");
}

// ── inject / identity ───────────────────────────────────────────────────

#[test]
fn helpers_inject_then_unfold() {
    let _ = compile("g.inject(1, 2, 3).unfold()");
}

#[test]
fn helpers_identity_step() {
    let _ = compile("g.V().identity()");
}
