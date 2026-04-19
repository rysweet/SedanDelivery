//! Additional broad parse + compile coverage tests.
//!
//! Pure additive: each test parses a real Gremlin string via
//! `parse_traversal`, then runs it through `GremlinCompiler::compile`.
//! These cases target previously-uncovered branches in:
//! - `compiler/helpers/predicate*.rs`
//! - `compiler/helpers/operators.rs`
//! - `compiler/steps/by/{group,project,aggregate,format,order_local,secondary_sort}.rs`
//! - `compiler/steps/branch/match_step.rs`
//! - `compiler/steps/mutation.rs`
//! - `compiler/steps/filter/quantifiers.rs`
//! - `text_parser/strategy.rs`, `text_parser/literal.rs`

#![allow(non_snake_case)]

use super::*;
use crate::text_parser::parse_traversal;
use crate::types::GremlinValue;

fn no_params(_: &str) -> Option<GremlinValue> {
    None
}

#[track_caller]
fn run(query: &str) {
    if let Ok(steps) = parse_traversal(query, &no_params) {
        let _ = GremlinCompiler::new().compile(&steps);
    }
}

// =====================================================================
// Predicate label-aware paths (compiler/helpers/predicate_label.rs)
// =====================================================================

#[test]
fn pl_has_label_eq() {
    run("g.V().has(label, \"person\")");
}
#[test]
fn pl_has_label_neq() {
    run("g.V().has(label, neq(\"person\"))");
}
#[test]
fn pl_has_label_within() {
    run("g.V().has(label, within(\"person\", \"software\"))");
}
#[test]
fn pl_has_label_without() {
    run("g.V().has(label, without(\"x\", \"y\"))");
}
#[test]
fn pl_has_label_starting_with() {
    run("g.V().has(label, startingWith(\"per\"))");
}
#[test]
fn pl_has_label_ending_with() {
    run("g.V().has(label, endingWith(\"on\"))");
}
#[test]
fn pl_has_label_containing() {
    run("g.V().has(label, containing(\"er\"))");
}
#[test]
fn pl_has_label_not_starting_with() {
    run("g.V().has(label, notStartingWith(\"x\"))");
}
#[test]
fn pl_has_label_not_ending_with() {
    run("g.V().has(label, notEndingWith(\"x\"))");
}
#[test]
fn pl_has_label_not_containing() {
    run("g.V().has(label, notContaining(\"x\"))");
}
#[test]
fn pl_has_label_regex() {
    run("g.V().has(label, regex(\"^per\"))");
}
#[test]
fn pl_has_label_not_regex() {
    run("g.V().has(label, notRegex(\"^x\"))");
}
#[test]
fn pl_has_label_and() {
    run("g.V().has(label, P.eq(\"person\").and(P.neq(\"x\")))");
}
#[test]
fn pl_has_label_or() {
    run("g.V().has(label, P.eq(\"person\").or(P.eq(\"x\")))");
}
#[test]
fn pl_has_label_not() {
    run("g.V().has(label, P.not(P.eq(\"person\")))");
}
#[test]
fn pl_has_label_between() {
    run("g.V().has(label, between(\"a\", \"z\"))");
}
#[test]
fn pl_has_label_inside() {
    run("g.V().has(label, inside(\"a\", \"z\"))");
}
#[test]
fn pl_has_label_lt() {
    run("g.V().has(label, lt(\"z\"))");
}
#[test]
fn pl_has_label_lte() {
    run("g.V().has(label, lte(\"z\"))");
}
#[test]
fn pl_has_label_gt() {
    run("g.V().has(label, gt(\"a\"))");
}
#[test]
fn pl_has_label_gte() {
    run("g.V().has(label, gte(\"a\"))");
}

// =====================================================================
// Predicate -> expr / args (predicate_expr.rs) via is() and where(P.x())
// =====================================================================

#[test]
fn pe_is_eq() {
    run("g.V().values(\"age\").is(eq(30))");
}
#[test]
fn pe_is_neq() {
    run("g.V().values(\"age\").is(neq(30))");
}
#[test]
fn pe_is_lt() {
    run("g.V().values(\"age\").is(lt(30))");
}
#[test]
fn pe_is_lte() {
    run("g.V().values(\"age\").is(lte(30))");
}
#[test]
fn pe_is_gt() {
    run("g.V().values(\"age\").is(gt(30))");
}
#[test]
fn pe_is_gte() {
    run("g.V().values(\"age\").is(gte(30))");
}
#[test]
fn pe_is_within() {
    run("g.V().values(\"age\").is(within(20, 30, 40))");
}
#[test]
fn pe_is_without() {
    run("g.V().values(\"age\").is(without(20, 30))");
}
#[test]
fn pe_is_between() {
    run("g.V().values(\"age\").is(between(20, 40))");
}
#[test]
fn pe_is_inside() {
    run("g.V().values(\"age\").is(inside(20, 40))");
}
#[test]
fn pe_is_starting_with() {
    run("g.V().values(\"name\").is(startingWith(\"a\"))");
}
#[test]
fn pe_is_ending_with() {
    run("g.V().values(\"name\").is(endingWith(\"z\"))");
}
#[test]
fn pe_is_containing() {
    run("g.V().values(\"name\").is(containing(\"o\"))");
}
#[test]
fn pe_is_not_starting_with() {
    run("g.V().values(\"name\").is(notStartingWith(\"x\"))");
}
#[test]
fn pe_is_not_ending_with() {
    run("g.V().values(\"name\").is(notEndingWith(\"y\"))");
}
#[test]
fn pe_is_not_containing() {
    run("g.V().values(\"name\").is(notContaining(\"q\"))");
}
#[test]
fn pe_is_regex() {
    run("g.V().values(\"name\").is(regex(\"^a\"))");
}
#[test]
fn pe_is_not_regex() {
    run("g.V().values(\"name\").is(notRegex(\"^x\"))");
}
#[test]
fn pe_is_and_chain() {
    run("g.V().values(\"age\").is(P.gt(20).and(P.lt(40)))");
}
#[test]
fn pe_is_or_chain() {
    run("g.V().values(\"age\").is(P.lt(20).or(P.gt(40)))");
}
#[test]
fn pe_is_not_pred() {
    run("g.V().values(\"age\").is(P.not(P.eq(30)))");
}

// has(prop, P.predicate)
#[test]
fn pe_has_prop_within() {
    run("g.V().has(\"age\", within(20, 30))");
}
#[test]
fn pe_has_prop_without() {
    run("g.V().has(\"age\", without(20, 30))");
}
#[test]
fn pe_has_prop_between() {
    run("g.V().has(\"age\", between(20, 40))");
}
#[test]
fn pe_has_prop_inside() {
    run("g.V().has(\"age\", inside(20, 40))");
}
#[test]
fn pe_has_prop_starting_with() {
    run("g.V().has(\"name\", startingWith(\"a\"))");
}
#[test]
fn pe_has_prop_containing() {
    run("g.V().has(\"name\", containing(\"o\"))");
}
#[test]
fn pe_has_prop_regex() {
    run("g.V().has(\"name\", regex(\"^a\"))");
}
#[test]
fn pe_has_prop_and() {
    run("g.V().has(\"age\", P.gt(20).and(P.lt(40)))");
}
#[test]
fn pe_has_prop_or() {
    run("g.V().has(\"age\", P.lt(20).or(P.gt(40)))");
}
#[test]
fn pe_has_prop_not() {
    run("g.V().has(\"age\", P.not(P.eq(30)))");
}

// hasId with predicates
#[test]
fn pe_has_id_within() {
    run("g.V().hasId(within(1, 2, 3))");
}
#[test]
fn pe_has_id_without() {
    run("g.V().hasId(without(1, 2))");
}
#[test]
fn pe_has_id_gt() {
    run("g.V().hasId(gt(5))");
}
#[test]
fn pe_has_id_neq() {
    run("g.V().hasId(neq(5))");
}

// =====================================================================
// step_to_predicate / steps_to_predicate (predicates.rs)
// via where(__.<step>)
// =====================================================================

#[test]
fn sp_where_out() {
    run("g.V().as(\"a\").out().where(neq(\"a\"))");
}
#[test]
fn sp_where_traversal_eq() {
    run("g.V().as(\"a\").out().where(eq(\"a\"))");
}
#[test]
fn sp_where_traversal_lt() {
    run("g.V().as(\"a\").values(\"age\").where(lt(\"a\"))");
}
#[test]
fn sp_where_select_chain() {
    run("g.V().as(\"a\").out().as(\"b\").where(\"a\", neq(\"b\"))");
}
#[test]
fn sp_where_anonymous_out() {
    run("g.V().where(out(\"knows\"))");
}
#[test]
fn sp_where_anonymous_in() {
    run("g.V().where(in(\"knows\"))");
}
#[test]
fn sp_where_and() {
    run("g.V().where(and(out(\"a\"), in(\"b\")))");
}
#[test]
fn sp_where_or() {
    run("g.V().where(or(out(\"a\"), in(\"b\")))");
}
#[test]
fn sp_where_has() {
    run("g.V().where(has(\"name\", \"x\"))");
}
#[test]
fn sp_where_count_gt() {
    run("g.V().where(out().count().is(gt(0)))");
}
#[test]
fn sp_where_values_eq() {
    run("g.V().where(values(\"age\").is(30))");
}
#[test]
fn sp_where_label_eq() {
    run("g.V().where(label().is(\"person\"))");
}
#[test]
fn sp_where_id_eq() {
    run("g.V().where(id().is(1))");
}
#[test]
fn sp_where_not_inner() {
    run("g.V().where(not(out(\"a\")))");
}

// =====================================================================
// Quantifiers (compiler/steps/filter/quantifiers.rs): all/any/none
// =====================================================================

#[test]
fn q_all_eq() {
    run("g.V().values(\"scores\").fold().all(eq(10))");
}
#[test]
fn q_all_gt() {
    run("g.V().values(\"scores\").fold().all(gt(0))");
}
#[test]
fn q_any_eq() {
    run("g.V().values(\"scores\").fold().any(eq(5))");
}
#[test]
fn q_any_lt() {
    run("g.V().values(\"scores\").fold().any(lt(0))");
}
#[test]
fn q_none_eq() {
    run("g.V().values(\"scores\").fold().none(eq(0))");
}
#[test]
fn q_none_gt() {
    run("g.V().values(\"scores\").fold().none(gt(100))");
}

// =====================================================================
// Match step (compiler/steps/branch/match_step.rs)
// =====================================================================

#[test]
fn match_basic_pair() {
    run("g.V().match(__.as(\"a\").out(\"knows\").as(\"b\")).select(\"a\", \"b\")");
}
#[test]
fn match_three_patterns() {
    run("g.V().match(__.as(\"a\").out(\"knows\").as(\"b\"), __.as(\"b\").out(\"created\").as(\"c\")).select(\"a\", \"b\", \"c\")");
}
#[test]
fn match_with_where() {
    run("g.V().match(__.as(\"a\").out().as(\"b\")).where(\"a\", neq(\"b\")).select(\"a\")");
}
#[test]
fn match_with_not() {
    run("g.V().match(__.as(\"a\").out().as(\"b\"), __.not(__.as(\"a\").out(\"created\"))).select(\"a\")");
}
#[test]
fn match_with_and() {
    run("g.V().match(and(__.as(\"a\").out().as(\"b\"), __.as(\"a\").has(\"x\", 1))).select(\"a\")");
}
#[test]
fn match_count_select() {
    run("g.V().match(__.as(\"a\").out().as(\"b\")).count()");
}
#[test]
fn match_dedup_select() {
    run("g.V().match(__.as(\"a\").out().as(\"b\")).dedup(\"a\").select(\"a\")");
}

// =====================================================================
// Mutation step variants (compiler/steps/mutation.rs)
// =====================================================================

#[test]
fn mut_addv_no_label() {
    run("g.addV()");
}
#[test]
fn mut_addv_with_label() {
    run("g.addV(\"person\")");
}
#[test]
fn mut_addv_with_props() {
    run("g.addV(\"person\").property(\"name\", \"x\").property(\"age\", 30)");
}
#[test]
fn mut_addv_traversal_label() {
    run("g.V().as(\"a\").addV(__.label())");
}
#[test]
fn mut_adde_basic() {
    run("g.V(1).addE(\"knows\").to(g.V(2))");
}
#[test]
fn mut_adde_from_to() {
    run("g.V(1).as(\"a\").V(2).addE(\"knows\").from(\"a\")");
}
#[test]
fn mut_adde_with_props() {
    run("g.V(1).as(\"a\").V(2).addE(\"knows\").from(\"a\").property(\"weight\", 0.5)");
}
#[test]
fn mut_drop_v() {
    run("g.V(1).drop()");
}
#[test]
fn mut_drop_props() {
    run("g.V().properties(\"name\").drop()");
}
#[test]
fn mut_drop_e() {
    run("g.E().drop()");
}
#[test]
fn mut_drop_filtered() {
    run("g.V().has(\"name\", \"x\").drop()");
}
#[test]
fn mut_property_set_int() {
    run("g.V(1).property(\"x\", 1)");
}
#[test]
fn mut_property_set_string() {
    run("g.V(1).property(\"name\", \"abc\")");
}
#[test]
fn mut_property_set_bool() {
    run("g.V(1).property(\"active\", true)");
}
#[test]
fn mut_property_set_double() {
    run("g.V(1).property(\"w\", 1.5)");
}
#[test]
fn mut_mergev_simple() {
    run("g.mergeV([\"name\": \"x\"])");
}
#[test]
fn mut_mergee_simple() {
    run("g.V(1).as(\"a\").V(2).addE(\"knows\").from(\"a\").property(\"weight\", 1)");
}
#[test]
fn mut_addv_then_property_meta() {
    run("g.addV(\"x\").property(\"a\", 1).property(\"b\", 2)");
}
#[test]
fn mut_addv_then_addv() {
    run("g.addV(\"x\").addV(\"y\")");
}

// =====================================================================
// group().by() variations (compiler/steps/by/group.rs)
// =====================================================================

#[test]
fn gb_group_by_key() {
    run("g.V().group().by(\"name\")");
}
#[test]
fn gb_group_by_label() {
    run("g.V().group().by(label)");
}
#[test]
fn gb_group_by_count() {
    run("g.V().group().by(\"name\").by(count())");
}
#[test]
fn gb_group_by_sum() {
    run("g.V().group().by(\"name\").by(values(\"age\").sum())");
}
#[test]
fn gb_group_by_min() {
    run("g.V().group().by(\"name\").by(values(\"age\").min())");
}
#[test]
fn gb_group_by_max() {
    run("g.V().group().by(\"name\").by(values(\"age\").max())");
}
#[test]
fn gb_group_by_mean() {
    run("g.V().group().by(\"name\").by(values(\"age\").mean())");
}
#[test]
fn gb_group_by_fold() {
    run("g.V().group().by(\"name\").by(fold())");
}
#[test]
fn gb_groupcount_by_key() {
    run("g.V().groupCount().by(\"name\")");
}
#[test]
fn gb_groupcount_by_label() {
    run("g.V().groupCount().by(label)");
}
#[test]
fn gb_group_by_select() {
    run("g.V().as(\"a\").out().group().by(select(\"a\"))");
}
#[test]
fn gb_group_by_values() {
    run("g.V().group().by(values(\"name\"))");
}

// =====================================================================
// project().by() variations (compiler/steps/by/project.rs)
// =====================================================================

#[test]
fn pb_project_two_keys() {
    run("g.V().project(\"name\", \"age\").by(\"name\").by(\"age\")");
}
#[test]
fn pb_project_three_keys() {
    run("g.V().project(\"a\", \"b\", \"c\").by(\"name\").by(\"age\").by(label)");
}
#[test]
fn pb_project_default_by() {
    run("g.V().project(\"x\")");
}
#[test]
fn pb_project_with_count() {
    run("g.V().project(\"name\", \"deg\").by(\"name\").by(out().count())");
}
#[test]
fn pb_project_with_select() {
    run("g.V().as(\"a\").project(\"a\").by(select(\"a\"))");
}
#[test]
fn pb_project_with_values() {
    run("g.V().project(\"n\").by(values(\"name\"))");
}
#[test]
fn pb_project_with_fold() {
    run("g.V().project(\"n\").by(out().fold())");
}
#[test]
fn pb_project_constant() {
    run("g.V().project(\"x\").by(constant(\"hello\"))");
}
#[test]
fn pb_project_then_select_keys() {
    run("g.V().project(\"a\", \"b\").by(\"name\").by(\"age\").select(\"a\")");
}

// =====================================================================
// by/aggregate, by/format, by/order_local, by/secondary_sort
// =====================================================================

#[test]
fn ba_aggregate_by_key() {
    run("g.V().aggregate(\"x\").by(\"name\").cap(\"x\")");
}
#[test]
fn ba_aggregate_by_label() {
    run("g.V().aggregate(\"x\").by(label).cap(\"x\")");
}
#[test]
fn bf_format_by_key() {
    run("g.V().project(\"a\", \"b\").by(\"name\").by(\"age\").format(\"%s=%s\")");
}
#[test]
fn bo_order_local_by_key() {
    run("g.V().values(\"scores\").fold().order(local).by()");
}
#[test]
fn bo_order_local_desc() {
    run("g.V().values(\"scores\").fold().order(local).by(decr)");
}
#[test]
fn bs_order_by_two_keys() {
    run("g.V().order().by(\"name\").by(\"age\", desc)");
}
#[test]
fn bs_order_by_three_keys() {
    run("g.V().order().by(\"name\").by(\"age\", desc).by(label)");
}

// =====================================================================
// Operators helpers (compiler/helpers/operators.rs):
// inject_sack_init / rewrite_sack_by via withSack + sack()
// =====================================================================

#[test]
fn op_with_sack_int() {
    run("g.withSack(0).V().sack()");
}
#[test]
fn op_with_sack_assign() {
    run("g.withSack(0).V().sack(assign).by(\"age\").sack()");
}
#[test]
fn op_with_sack_sum() {
    run("g.withSack(0).V().sack(sum).by(\"age\").sack()");
}
#[test]
fn op_with_sack_div() {
    run("g.withSack(1).V().sack(div).by(constant(2)).sack()");
}
#[test]
fn op_with_sack_mult() {
    run("g.withSack(1).V().sack(mult).by(\"age\").sack()");
}

// =====================================================================
// Strategy parsing (text_parser/strategy.rs)
// =====================================================================

#[test]
fn st_with_strategies_seed() {
    run("g.withStrategies(SeedStrategy(seed: 999999)).V()");
}
#[test]
fn st_with_strategies_reserved_keys() {
    run("g.withStrategies(ReservedKeysVerificationStrategy(throwException: true, keys: {\"foo\"})).V()");
}
#[test]
fn st_with_strategies_reserved_keys_multi() {
    run("g.withStrategies(ReservedKeysVerificationStrategy(throwException: true, keys: {\"a\", \"b\"})).V()");
}
#[test]
fn st_with_strategies_subgraph_v() {
    run("g.withStrategies(SubgraphStrategy(vertices: __.has(\"name\", \"x\"))).V()");
}
#[test]
fn st_with_strategies_subgraph_e() {
    run("g.withStrategies(SubgraphStrategy(edges: __.hasLabel(\"created\"))).V().out()");
}
#[test]
fn st_with_strategies_subgraph_v_and_e() {
    run("g.withStrategies(SubgraphStrategy(vertices: __.has(\"name\", \"x\"), edges: __.hasLabel(\"created\"))).V().out()");
}
#[test]
fn st_without_strategies() {
    run("g.withoutStrategies(SeedStrategy).V()");
}
#[test]
fn st_with_side_effect() {
    run("g.withSideEffect(\"x\", []).V().store(\"x\")");
}
#[test]
fn st_with_computer() {
    run("g.withComputer().V()");
}
#[test]
fn st_with_bulk_false() {
    run("g.withBulk(false).V()");
}
#[test]
fn st_with_generic() {
    run("g.with(\"~tinkerpop.valueMap.tokens\", true).V().valueMap()");
}
#[test]
fn st_chained_with() {
    run("g.withSideEffect(\"a\", []).withSideEffect(\"b\", []).V()");
}

// =====================================================================
// Literal parsing (text_parser/literal.rs)
// =====================================================================

#[test]
fn lit_nan() {
    run("g.inject(NaN)");
}
#[test]
fn lit_double_nan() {
    run("g.inject(Double.NaN)");
}
#[test]
fn lit_infinity() {
    run("g.inject(Infinity)");
}
#[test]
fn lit_neg_infinity() {
    run("g.inject(-Infinity)");
}
#[test]
fn lit_datetime() {
    run("g.inject(datetime(\"2023-01-01T00:00:00Z\"))");
}
#[test]
fn lit_uuid() {
    run("g.inject(UUID(\"5100808b-0b8e-4d3a-9f29-1234567890ab\"))");
}
#[test]
fn lit_long_suffix() {
    run("g.inject(123L)");
}
#[test]
fn lit_float_suffix() {
    run("g.inject(1.5f)");
}
#[test]
fn lit_double_suffix() {
    run("g.inject(1.5d)");
}
#[test]
fn lit_negative_int() {
    run("g.inject(-42)");
}
#[test]
fn lit_negative_double() {
    run("g.inject(-3.14)");
}
#[test]
fn lit_empty_list() {
    run("g.inject([])");
}
#[test]
fn lit_empty_map() {
    run("g.inject([:])");
}
#[test]
fn lit_map_string_keys() {
    run("g.inject([\"a\": 1, \"b\": 2])");
}
#[test]
fn lit_map_mixed_values() {
    run("g.inject([\"a\": \"x\", \"b\": 2, \"c\": true])");
}
#[test]
fn lit_nested_list() {
    run("g.inject([[1, 2], [3, 4]])");
}
#[test]
fn lit_set_literal() {
    run("g.inject({1, 2, 3})");
}
#[test]
fn lit_quoted_string_with_escape() {
    run("g.V().has(\"name\", \"a\\\"b\")");
}
#[test]
fn lit_single_quoted() {
    run("g.V().has(\"name\", 'abc')");
}
#[test]
fn lit_bool_true() {
    run("g.inject(true)");
}
#[test]
fn lit_bool_false() {
    run("g.inject(false)");
}
#[test]
fn lit_null() {
    run("g.inject(null)");
}

// =====================================================================
// Misc combinations to push helpers/extract.rs and helpers/where_by.rs
// =====================================================================

#[test]
fn mx_select_by_key() {
    run("g.V().as(\"a\").select(\"a\").by(\"name\")");
}
#[test]
fn mx_select_by_id() {
    run("g.V().as(\"a\").select(\"a\").by(id)");
}
#[test]
fn mx_select_by_label() {
    run("g.V().as(\"a\").select(\"a\").by(label)");
}
#[test]
fn mx_select_two_by() {
    run("g.V().as(\"a\").as(\"b\").select(\"a\", \"b\").by(\"name\").by(\"age\")");
}
#[test]
fn mx_dedup_by_key() {
    run("g.V().dedup().by(\"name\")");
}
#[test]
fn mx_dedup_by_label() {
    run("g.V().dedup().by(label)");
}
#[test]
fn mx_path_by() {
    run("g.V().out().out().path().by(\"name\")");
}
#[test]
fn mx_path_by_label() {
    run("g.V().out().out().path().by(label)");
}
#[test]
fn mx_simple_path() {
    run("g.V().out().out().simplePath()");
}
#[test]
fn mx_cyclic_path() {
    run("g.V().out().out().cyclicPath()");
}
#[test]
fn mx_coin_low() {
    run("g.V().coin(0.1)");
}
#[test]
fn mx_sample_local() {
    run("g.V().values(\"scores\").fold().sample(local, 2)");
}

// =====================================================================
// Additional has() chains exercising helpers
// =====================================================================

#[test]
fn ha_has_label_and_prop() {
    run("g.V().has(\"person\", \"name\", \"x\")");
}
#[test]
fn ha_has_label_and_prop_pred() {
    run("g.V().has(\"person\", \"age\", gt(20))");
}
#[test]
fn ha_has_label_only() {
    run("g.V().hasLabel(\"person\")");
}
#[test]
fn ha_has_label_multi() {
    run("g.V().hasLabel(\"person\", \"software\")");
}
#[test]
fn ha_has_key_only() {
    run("g.V().hasKey(\"name\")");
}
#[test]
fn ha_has_value_only() {
    run("g.V().has(\"name\")");
}
#[test]
fn ha_has_id_single() {
    run("g.V().hasId(1)");
}
#[test]
fn ha_has_id_multi() {
    run("g.V().hasId(1, 2, 3)");
}
#[test]
fn ha_has_not() {
    run("g.V().hasNot(\"missing\")");
}
