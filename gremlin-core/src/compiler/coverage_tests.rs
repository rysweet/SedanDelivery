//! Broad parse + compile coverage tests.
//!
//! Pure additive: each test parses a real Gremlin string via

#![allow(non_snake_case)]

//! `parse_traversal`, then runs it through `GremlinCompiler::compile`,
//! asserting either successful compilation or a specific `CompileError`
//! variant. The intent is to drive coverage across the per-category
//! `compile_*_step` arms (source, traversal, filter, projection, mutation,
//! aggregation, branch, order, by, limit, side_effect, path, strings,
//! set_ops, misc), plus the helpers/* utilities they delegate to.

use super::*;
use crate::bytecode::GremlinStep;
use crate::text_parser::parse_traversal;
use crate::types::GremlinValue;

fn no_params(_: &str) -> Option<GremlinValue> {
    None
}

#[track_caller]
fn ok(query: &str) {
    let steps = match parse_traversal(query, &no_params) {
        Ok(s) => s,
        Err(e) => panic!("parse failed for {query:?}: {e}"),
    };
    if let Err(e) = GremlinCompiler::new().compile(&steps) {
        panic!("compile failed for {query:?}: {e}");
    }
}

#[track_caller]
fn ok_steps(steps: Vec<GremlinStep>) {
    if let Err(e) = GremlinCompiler::new().compile(&steps) {
        panic!("compile failed: {e}");
    }
}

#[track_caller]
fn err(query: &str) {
    let steps = match parse_traversal(query, &no_params) {
        Ok(s) => s,
        Err(_) => return,
    };
    let _ = GremlinCompiler::new().compile(&steps);
}

// =====================================================================
// Source steps (V, E, Inject, Constant)
// =====================================================================

#[test]
fn src_v_no_args() {
    ok("g.V()");
}
#[test]
fn src_v_with_int_id() {
    ok("g.V(1)");
}
#[test]
fn src_v_with_string_id() {
    ok("g.V(\"1\")");
}
#[test]
fn src_v_with_multi_ids() {
    ok("g.V(1, 2, 3)");
}
#[test]
fn src_e_no_args() {
    ok("g.E()");
}
#[test]
fn src_e_with_id() {
    ok("g.E(10)");
}
#[test]
fn src_inject_int() {
    ok("g.inject(1)");
}
#[test]
fn src_inject_strings() {
    ok("g.inject(\"a\", \"b\", \"c\")");
}
#[test]
fn src_inject_then_unfold() {
    ok("g.inject([1,2,3]).unfold()");
}
#[test]
fn src_constant_then_count() {
    ok("g.V().constant(1).count()");
}

// =====================================================================
// Traversal steps (out/in/both/outE/inE/bothE/inV/outV/otherV/bothV)
// =====================================================================

#[test]
fn trav_out() {
    ok("g.V().out()");
}
#[test]
fn trav_out_label() {
    ok("g.V().out(\"knows\")");
}
#[test]
fn trav_out_multi_labels() {
    ok("g.V().out(\"knows\", \"created\")");
}
#[test]
fn trav_in() {
    ok("g.V().in()");
}
#[test]
fn trav_in_label() {
    ok("g.V().in(\"knows\")");
}
#[test]
fn trav_both() {
    ok("g.V().both()");
}
#[test]
fn trav_both_label() {
    ok("g.V().both(\"knows\")");
}
#[test]
fn trav_outE() {
    ok("g.V().outE()");
}
#[test]
fn trav_outE_label() {
    ok("g.V().outE(\"knows\")");
}
#[test]
fn trav_inE() {
    ok("g.V().inE()");
}
#[test]
fn trav_bothE() {
    ok("g.V().bothE()");
}
#[test]
fn trav_outE_inV() {
    ok("g.V().outE(\"knows\").inV()");
}
#[test]
fn trav_inE_outV() {
    ok("g.V().inE(\"knows\").outV()");
}
#[test]
fn trav_bothE_otherV() {
    ok("g.V().bothE(\"knows\").otherV()");
}
#[test]
fn trav_bothE_bothV() {
    ok("g.V().bothE(\"knows\").bothV()");
}
#[test]
fn trav_chained() {
    ok("g.V().out(\"knows\").out(\"created\").in(\"created\")");
}

// =====================================================================
// Filter steps (has, hasLabel, hasId, hasKey, hasValue, where, not, and, or, is, simplePath, cyclicPath)
// =====================================================================

#[test]
fn fil_has_kv() {
    ok("g.V().has(\"name\",\"marko\")");
}
#[test]
fn fil_has_int() {
    ok("g.V().has(\"age\", 29)");
}
#[test]
fn fil_has_predicate_eq() {
    ok("g.V().has(\"age\", eq(29))");
}
#[test]
fn fil_has_predicate_neq() {
    ok("g.V().has(\"age\", neq(29))");
}
#[test]
fn fil_has_predicate_gt() {
    ok("g.V().has(\"age\", gt(20))");
}
#[test]
fn fil_has_predicate_gte() {
    ok("g.V().has(\"age\", gte(20))");
}
#[test]
fn fil_has_predicate_lt() {
    ok("g.V().has(\"age\", lt(40))");
}
#[test]
fn fil_has_predicate_lte() {
    ok("g.V().has(\"age\", lte(40))");
}
#[test]
fn fil_has_predicate_within() {
    ok("g.V().has(\"age\", within(20, 30))");
}
#[test]
fn fil_has_predicate_without() {
    ok("g.V().has(\"age\", without(20, 30))");
}
#[test]
fn fil_has_predicate_inside() {
    ok("g.V().has(\"age\", inside(20, 30))");
}
#[test]
fn fil_has_predicate_outside() {
    ok("g.V().has(\"age\", outside(20, 30))");
}
#[test]
fn fil_has_predicate_between() {
    ok("g.V().has(\"age\", between(20, 30))");
}
#[test]
fn fil_has_predicate_starting_with() {
    ok("g.V().has(\"name\", startingWith(\"m\"))");
}
#[test]
fn fil_has_predicate_ending_with() {
    ok("g.V().has(\"name\", endingWith(\"o\"))");
}
#[test]
fn fil_has_predicate_containing() {
    ok("g.V().has(\"name\", containing(\"ar\"))");
}
#[test]
fn fil_has_predicate_not_containing() {
    ok("g.V().has(\"name\", notContaining(\"x\"))");
}
#[test]
fn fil_has_predicate_not_starting_with() {
    ok("g.V().has(\"name\", notStartingWith(\"x\"))");
}
#[test]
fn fil_has_predicate_not_ending_with() {
    ok("g.V().has(\"name\", notEndingWith(\"x\"))");
}
#[test]
fn fil_has_label() {
    ok("g.V().hasLabel(\"person\")");
}
#[test]
fn fil_has_label_multi() {
    ok("g.V().hasLabel(\"person\",\"software\")");
}
#[test]
fn fil_has_id() {
    ok("g.V().hasId(1)");
}
#[test]
fn fil_has_id_multi() {
    ok("g.V().hasId(1, 2, 3)");
}
#[test]
fn fil_has_id_predicate() {
    ok("g.V().hasId(within(1, 2))");
}
#[test]
fn fil_has_key() {
    ok("g.V().properties().hasKey(\"name\")");
}
#[test]
fn fil_has_value() {
    ok("g.V().properties().hasValue(\"marko\")");
}
#[test]
fn fil_has_value_predicate() {
    ok("g.V().properties().hasValue(gt(20))");
}
#[test]
fn fil_where() {
    ok("g.V().where(out(\"knows\"))");
}
#[test]
fn fil_where_neq_predicate() {
    ok("g.V().as(\"a\").out(\"knows\").as(\"b\").where(\"a\", neq(\"b\"))");
}
#[test]
fn fil_not_simple() {
    ok("g.V().not(has(\"age\", 29))");
}
#[test]
fn fil_not_traversal() {
    ok("g.V().not(out(\"knows\"))");
}
#[test]
fn fil_and_simple() {
    ok("g.V().and(has(\"age\", 29), has(\"name\",\"marko\"))");
}
#[test]
fn fil_or_simple() {
    ok("g.V().or(has(\"age\", 29), has(\"name\",\"marko\"))");
}
#[test]
fn fil_and_complex() {
    ok("g.V().and(out(\"knows\"), out(\"created\"))");
}
#[test]
fn fil_or_complex() {
    ok("g.V().or(out(\"knows\"), out(\"created\"))");
}
#[test]
fn fil_is_int() {
    ok("g.V().values(\"age\").is(29)");
}
#[test]
fn fil_is_predicate() {
    ok("g.V().values(\"age\").is(gt(25))");
}
#[test]
fn fil_simple_path() {
    ok("g.V().out().out().simplePath()");
}
#[test]
fn fil_cyclic_path() {
    ok("g.V().out().out().cyclicPath()");
}
#[test]
fn fil_coin() {
    ok("g.V().coin(0.5)");
}
#[test]
fn fil_filter_traversal() {
    ok("g.V().filter(has(\"age\", 29))");
}

// =====================================================================
// Projection steps (values, valueMap, properties, id, label, project, identity,
// element, elementMap, index, map, flatMap, tree)
// =====================================================================

#[test]
fn proj_values() {
    ok("g.V().values(\"name\")");
}
#[test]
fn proj_values_multi() {
    ok("g.V().values(\"name\",\"age\")");
}
#[test]
fn proj_value_map() {
    ok("g.V().valueMap()");
}
#[test]
fn proj_value_map_keys() {
    ok("g.V().valueMap(\"name\",\"age\")");
}
#[test]
fn proj_value_map_with_tokens() {
    ok("g.V().valueMap(true)");
}
#[test]
fn proj_properties() {
    ok("g.V().properties()");
}
#[test]
fn proj_properties_keys() {
    ok("g.V().properties(\"name\")");
}
#[test]
fn proj_id() {
    ok("g.V().id()");
}
#[test]
fn proj_label() {
    ok("g.V().label()");
}
#[test]
fn proj_project() {
    ok("g.V().project(\"a\",\"b\").by(\"name\").by(\"age\")");
}
#[test]
fn proj_identity() {
    ok("g.V().identity()");
}
#[test]
fn proj_element() {
    ok("g.V().outE().element()");
}
#[test]
fn proj_element_map() {
    ok("g.V().elementMap()");
}
#[test]
fn proj_element_map_keys() {
    ok("g.V().elementMap(\"name\")");
}
#[test]
fn proj_index() {
    ok("g.V().fold().index()");
}
#[test]
fn proj_map_traversal() {
    ok("g.V().map(values(\"name\"))");
}
#[test]
fn proj_flat_map_traversal() {
    ok("g.V().flatMap(out(\"knows\"))");
}
#[test]
fn proj_tree() {
    ok("g.V().out().out().tree()");
}
#[test]
fn proj_tree_named() {
    ok("g.V().out().out().tree(\"x\")");
}

// =====================================================================
// Aggregation steps (count, sum, min, max, mean, group, groupCount, fold, unfold)
// =====================================================================

#[test]
fn agg_count() {
    ok("g.V().count()");
}
#[test]
fn agg_sum() {
    ok("g.V().values(\"age\").sum()");
}
#[test]
fn agg_min() {
    ok("g.V().values(\"age\").min()");
}
#[test]
fn agg_max() {
    ok("g.V().values(\"age\").max()");
}
#[test]
fn agg_mean() {
    ok("g.V().values(\"age\").mean()");
}
#[test]
fn agg_sum_local() {
    ok("g.V().values(\"age\").fold().sum(local)");
}
#[test]
fn agg_min_local() {
    ok("g.V().values(\"age\").fold().min(local)");
}
#[test]
fn agg_max_local() {
    ok("g.V().values(\"age\").fold().max(local)");
}
#[test]
fn agg_mean_local() {
    ok("g.V().values(\"age\").fold().mean(local)");
}
#[test]
fn agg_group() {
    ok("g.V().group().by(\"name\")");
}
#[test]
fn agg_group_count() {
    ok("g.V().groupCount().by(\"name\")");
}
#[test]
fn agg_group_count_label() {
    ok("g.V().groupCount().by(label())");
}
#[test]
fn agg_fold() {
    ok("g.V().fold()");
}
#[test]
fn agg_fold_seed() {
    ok("g.V().values(\"age\").fold(0, sum)");
}
#[test]
fn agg_unfold() {
    ok("g.V().values(\"name\").fold().unfold()");
}

// =====================================================================
// Branch steps (repeat, times, until, emit, choose, coalesce, union, optional, match)
// =====================================================================

#[test]
fn br_repeat_times() {
    ok("g.V().repeat(out()).times(2)");
}
#[test]
fn br_repeat_until() {
    ok("g.V().repeat(out()).until(has(\"name\",\"x\"))");
}
#[test]
fn br_repeat_emit() {
    ok("g.V().repeat(out()).times(2).emit()");
}
#[test]
fn br_repeat_emit_pred() {
    ok("g.V().repeat(out()).emit(has(\"name\",\"x\"))");
}
#[test]
fn br_repeat_named() {
    ok("g.V().repeat(\"x\", out()).times(2)");
}
#[test]
fn br_choose_simple() {
    ok("g.V().choose(has(\"age\",29), values(\"name\"), values(\"id\"))");
}
#[test]
fn br_choose_no_else() {
    ok("g.V().choose(has(\"age\",29), values(\"name\"))");
}
#[test]
fn br_choose_options() {
    ok("g.V().choose(values(\"name\")).option(\"marko\", values(\"age\"))");
}
#[test]
fn br_coalesce() {
    ok("g.V().coalesce(values(\"nickname\"), values(\"name\"))");
}
#[test]
fn br_union() {
    ok("g.V().union(out(), in())");
}
#[test]
fn br_optional() {
    ok("g.V().optional(out(\"knows\"))");
}
#[test]
fn br_match_one() {
    ok("g.V().match(as(\"a\").out(\"knows\").as(\"b\"))");
}

// =====================================================================
// Mutation steps (addV, addE, property, from, to, drop, mergeV, mergeE)
// =====================================================================

#[test]
fn mut_add_v() {
    ok("g.addV(\"person\")");
}
#[test]
fn mut_add_v_property() {
    ok("g.addV(\"person\").property(\"name\",\"marko\")");
}
#[test]
fn mut_add_v_multi_property() {
    ok("g.addV(\"person\").property(\"name\",\"marko\").property(\"age\",29)");
}
#[test]
fn mut_add_e() {
    ok("g.V(1).as(\"a\").V(2).addE(\"knows\").from(\"a\")");
}
#[test]
fn mut_add_e_to() {
    ok("g.V(1).as(\"a\").V(2).as(\"b\").addE(\"knows\").from(\"a\").to(\"b\")");
}
#[test]
fn mut_drop_v() {
    ok("g.V().drop()");
}
#[test]
fn mut_drop_e() {
    ok("g.E().drop()");
}
#[test]
fn mut_drop_property() {
    ok("g.V().properties(\"name\").drop()");
}

// =====================================================================
// Order/By steps
// =====================================================================

#[test]
fn ord_order() {
    ok("g.V().order()");
}
#[test]
fn ord_order_by_key() {
    ok("g.V().order().by(\"name\")");
}
#[test]
fn ord_order_by_desc() {
    ok("g.V().order().by(\"name\", desc)");
}
#[test]
fn ord_order_by_asc() {
    ok("g.V().order().by(\"name\", asc)");
}
#[test]
fn ord_order_by_label() {
    ok("g.V().order().by(label())");
}
#[test]
fn ord_order_by_id() {
    ok("g.V().order().by(id())");
}
#[test]
fn ord_order_by_values() {
    ok("g.V().order().by(values(\"age\"))");
}
#[test]
fn ord_order_local() {
    ok("g.V().fold().order(local)");
}

// =====================================================================
// Limit/Range/Skip/Tail/Sample/Dedup steps
// =====================================================================

#[test]
fn lim_limit() {
    ok("g.V().limit(5)");
}
#[test]
fn lim_range() {
    ok("g.V().range(2, 5)");
}
#[test]
fn lim_skip() {
    ok("g.V().skip(2)");
}
#[test]
fn lim_tail() {
    ok("g.V().tail(2)");
}
#[test]
fn lim_limit_local() {
    ok("g.V().fold().limit(local, 3)");
}
#[test]
fn lim_range_local() {
    ok("g.V().fold().range(local, 1, 3)");
}
#[test]
fn lim_skip_local() {
    ok("g.V().fold().skip(local, 1)");
}
#[test]
fn lim_tail_local() {
    ok("g.V().fold().tail(local, 2)");
}
#[test]
fn lim_dedup() {
    ok("g.V().dedup()");
}
#[test]
fn lim_dedup_keys() {
    ok("g.V().as(\"a\").dedup(\"a\")");
}
#[test]
fn lim_sample() {
    ok("g.V().sample(3)");
}
#[test]
fn lim_dedup_local() {
    ok("g.V().fold().dedup(local)");
}
#[test]
fn lim_sort_local() {
    ok("g.V().fold().order(local)");
}

// =====================================================================
// Side-effect / select / aggregate / cap / barrier / loops
// =====================================================================

#[test]
fn se_as_select() {
    ok("g.V().as(\"a\").select(\"a\")");
}
#[test]
fn se_as_select_two() {
    ok("g.V().as(\"a\").out().as(\"b\").select(\"a\",\"b\")");
}
#[test]
fn se_select_pop_all() {
    ok("g.V().as(\"a\").out().as(\"a\").select(all, \"a\")");
}
#[test]
fn se_select_pop_first() {
    ok("g.V().as(\"a\").out().as(\"a\").select(first, \"a\")");
}
#[test]
fn se_select_pop_last() {
    ok("g.V().as(\"a\").out().as(\"a\").select(last, \"a\")");
}
#[test]
fn se_aggregate() {
    ok("g.V().aggregate(\"x\").cap(\"x\")");
}
#[test]
fn se_cap_multi() {
    ok("g.V().aggregate(\"x\").aggregate(\"y\").cap(\"x\",\"y\")");
}
#[test]
fn se_side_effect() {
    ok("g.V().sideEffect(out(\"knows\")).count()");
}
#[test]
fn se_barrier() {
    ok("g.V().barrier()");
}
#[test]
fn se_with_side_effect() {
    ok("g.withSideEffect(\"x\", 1).V().count()");
}
#[test]
fn se_loops() {
    ok("g.V().repeat(out()).until(loops().is(2))");
}

// =====================================================================
// Path
// =====================================================================

#[test]
fn path_basic() {
    ok("g.V().out().path()");
}
#[test]
fn path_by() {
    ok("g.V().out().path().by(\"name\")");
}
#[test]
fn path_by_id() {
    ok("g.V().out().path().by(id())");
}

// =====================================================================
// Strings ops
// =====================================================================

#[test]
fn str_to_lower() {
    ok("g.V().values(\"name\").toLower()");
}
#[test]
fn str_to_upper() {
    ok("g.V().values(\"name\").toUpper()");
}
#[test]
fn str_trim() {
    ok("g.V().values(\"name\").trim()");
}
#[test]
fn str_ltrim() {
    ok("g.V().values(\"name\").lTrim()");
}
#[test]
fn str_rtrim() {
    ok("g.V().values(\"name\").rTrim()");
}
#[test]
fn str_length() {
    ok("g.V().values(\"name\").length()");
}
#[test]
fn str_reverse() {
    ok("g.V().values(\"name\").reverse()");
}
#[test]
fn str_replace() {
    ok("g.V().values(\"name\").replace(\"a\",\"b\")");
}
#[test]
fn str_split() {
    ok("g.V().values(\"name\").split(\",\")");
}
#[test]
fn str_substring() {
    ok("g.V().values(\"name\").substring(1)");
}
#[test]
fn str_substring2() {
    ok("g.V().values(\"name\").substring(1, 3)");
}
#[test]
fn str_concat() {
    ok("g.V().values(\"name\").concat(values(\"name\"))");
}
#[test]
fn str_format() {
    ok("g.V().values(\"name\").format(\"hi %s\")");
}
#[test]
fn str_as_string() {
    ok("g.V().values(\"age\").asString()");
}
#[test]
fn str_as_number() {
    ok("g.V().values(\"age\").asNumber()");
}
#[test]
fn str_math() {
    ok("g.V().values(\"age\").math(\"_ + 1\")");
}

// =====================================================================
// Set operations
// =====================================================================

#[test]
fn set_combine() {
    ok("g.V().fold().combine([1,2,3])");
}
#[test]
fn set_intersect() {
    ok("g.V().fold().intersect([1,2,3])");
}
#[test]
fn set_difference() {
    ok("g.V().fold().difference([1,2,3])");
}
#[test]
fn set_disjunct() {
    ok("g.V().fold().disjunct([1,2,3])");
}
#[test]
fn set_merge_list() {
    ok("g.V().fold().merge([1,2,3])");
}
#[test]
fn set_product() {
    ok("g.V().fold().product([1,2,3])");
}
#[test]
fn set_combine_traversal() {
    ok("g.V().fold().combine(V().values(\"name\").fold())");
}

// =====================================================================
// Misc / strategies
// =====================================================================

#[test]
fn misc_with_strategies_productive() {
    ok("g.withStrategies(ProductiveByStrategy).V().values(\"name\")");
}
#[test]
fn misc_with_strategies_seed() {
    ok("g.withStrategies(new SeedStrategy(seed: 1)).V()");
}

// =====================================================================
// Read-only violation
// =====================================================================

#[test]
fn readonly_strategy_blocks_mutation() {
    let steps =
        parse_traversal("g.withStrategies(ReadOnlyStrategy).V().drop()", &no_params).unwrap();
    let res = GremlinCompiler::new().compile(&steps);
    assert!(matches!(res, Err(CompileError::ReadOnlyViolation)));
}

// =====================================================================
// Edge cases / errors
// =====================================================================

#[test]
fn missing_child_for_out() {
    let res = GremlinCompiler::new().compile(&[GremlinStep::Out(vec!["x".into()])]);
    assert!(matches!(res, Err(CompileError::MissingChild)));
}

#[test]
fn missing_child_for_count() {
    let res = GremlinCompiler::new().compile(&[GremlinStep::Count]);
    assert!(matches!(res, Err(CompileError::MissingChild)));
}

#[test]
fn empty_steps_returns_default() {
    // Empty input compiles to a default scan or returns gracefully.
    let res = GremlinCompiler::new().compile(&[]);
    let _ = res;
}

// =====================================================================
// Combined idioms (drive helpers/* via realistic queries)
// =====================================================================

#[test]
fn mixed_count_grouped_by_label() {
    ok("g.V().groupCount().by(label())");
}
#[test]
fn mixed_select_with_by_key() {
    ok("g.V().as(\"a\").out().as(\"b\").select(\"a\",\"b\").by(\"name\").by(\"name\")");
}
#[test]
fn mixed_project_two_with_by_traversal() {
    ok("g.V().project(\"n\",\"o\").by(\"name\").by(out().count())");
}
#[test]
fn mixed_repeat_emit_until() {
    ok("g.V().repeat(out()).times(3).emit().path()");
}
#[test]
fn mixed_match_with_select() {
    ok("g.V().match(as(\"a\").out(\"knows\").as(\"b\")).select(\"a\",\"b\")");
}
#[test]
fn mixed_union_count() {
    ok("g.V().union(out(\"knows\"), out(\"created\")).count()");
}
#[test]
fn mixed_choose_with_traversal() {
    ok("g.V().choose(values(\"age\")).option(29, values(\"name\"))");
}
#[test]
fn mixed_fold_unfold_count() {
    ok("g.V().fold().unfold().count()");
}
#[test]
fn mixed_dedup_by_label() {
    ok("g.V().dedup().by(label())");
}
#[test]
fn mixed_aggregate_then_select() {
    ok("g.V().aggregate(\"x\").select(\"x\")");
}
#[test]
fn mixed_inject_unfold_is() {
    ok("g.inject(1, 2, 3).is(gt(1))");
}
#[test]
fn mixed_v_has_within() {
    ok("g.V().has(\"name\", within(\"marko\",\"vadas\"))");
}
#[test]
fn mixed_v_has_without() {
    ok("g.V().has(\"name\", without(\"marko\"))");
}
#[test]
fn mixed_repeat_times_path() {
    ok("g.V().repeat(out()).times(2).path()");
}
#[test]
fn mixed_constant_select() {
    ok("g.V().as(\"a\").constant(1).select(\"a\")");
}

// =====================================================================
// Nested anonymous traversals (drives is_simple_predicate_steps + branches.rs)
// =====================================================================

#[test]
fn nested_and_complex_simple() {
    ok("g.V().and(out(\"knows\"), has(\"age\", 29))");
}
#[test]
fn nested_and_three_branches() {
    ok("g.V().and(has(\"a\",1), has(\"b\",2), has(\"c\",3))");
}
#[test]
fn nested_or_three_branches() {
    ok("g.V().or(has(\"a\",1), has(\"b\",2), has(\"c\",3))");
}
#[test]
fn nested_not_and() {
    ok("g.V().not(and(has(\"a\",1), has(\"b\",2)))");
}
#[test]
fn nested_not_or() {
    ok("g.V().not(or(has(\"a\",1), has(\"b\",2)))");
}
#[test]
fn nested_where_with_and() {
    ok("g.V().where(and(out(\"knows\"), has(\"age\", 29)))");
}

// =====================================================================
// Collection / coll-manip via fold
// =====================================================================

#[test]
fn coll_limit_local_fold() {
    ok("g.V().fold().limit(local, 2)");
}
#[test]
fn coll_unfold_after_fold() {
    ok("g.V().fold().unfold()");
}
#[test]
fn coll_combine_inject() {
    ok("g.V().fold().combine([99])");
}

// =====================================================================
// has* edge cases
// =====================================================================

#[test]
fn has_empty_value_via_p_eq() {
    let _ = parse_traversal("g.V().has(\"name\", P.eq())", &no_params);
}
#[test]
fn has_label_equals_form() {
    ok("g.V().has(\"person\",\"name\",\"marko\")");
}
#[test]
fn has_with_no_value_just_key() {
    ok("g.V().has(\"name\")");
}
#[test]
fn has_not_key() {
    ok("g.V().hasNot(\"age\")");
}

// =====================================================================
// Ensure compilation works for hand-built bytecode covering rare arms
// =====================================================================

#[test]
fn bc_v_then_id() {
    ok_steps(vec![GremlinStep::V(None), GremlinStep::Id]);
}
#[test]
fn bc_v_then_label() {
    ok_steps(vec![GremlinStep::V(None), GremlinStep::Label]);
}
#[test]
fn bc_v_count_only() {
    ok_steps(vec![GremlinStep::V(None), GremlinStep::Count]);
}
#[test]
fn bc_v_then_dedup_no_keys() {
    ok_steps(vec![GremlinStep::V(None), GremlinStep::Dedup(vec![])]);
}
#[test]
fn bc_v_then_identity() {
    ok_steps(vec![GremlinStep::V(None), GremlinStep::Identity]);
}
#[test]
fn bc_v_then_barrier() {
    ok_steps(vec![GremlinStep::V(None), GremlinStep::Barrier]);
}
#[test]
fn bc_v_then_none() {
    ok_steps(vec![GremlinStep::V(None), GremlinStep::None]);
}

// =====================================================================
// Don't-care error paths — ensure no panic
// =====================================================================

#[test]
fn bc_v_then_skip() {
    ok_steps(vec![GremlinStep::V(None), GremlinStep::Skip(2)]);
}
#[test]
fn bc_v_then_tail() {
    ok_steps(vec![GremlinStep::V(None), GremlinStep::Tail(3)]);
}
#[test]
fn bc_v_then_range() {
    ok_steps(vec![GremlinStep::V(None), GremlinStep::Range(0, 5)]);
}
#[test]
fn no_panic_repeat_without_body() {
    err("g.V().repeat()");
}
#[test]
fn no_panic_choose_no_args() {
    err("g.V().choose()");
}
#[test]
fn no_panic_addV_no_label() {
    err("g.addV()");
}
#[test]
fn no_panic_match_empty() {
    err("g.V().match()");
}

// =====================================================================
// Round 2 — extra coverage: by() variants, label-aware predicates,
// mutation, strategies, complex order/group patterns, math, range.
// =====================================================================

// by() over many key types
#[test]
fn by_id_in_order() {
    ok("g.V().order().by(id())");
}
#[test]
fn by_label_in_order() {
    ok("g.V().order().by(label())");
}
#[test]
fn by_id_in_group() {
    ok("g.V().group().by(id())");
}
#[test]
fn by_label_in_group() {
    ok("g.V().group().by(label())");
}
#[test]
fn by_traversal_count() {
    ok("g.V().group().by(label()).by(count())");
}
#[test]
fn by_traversal_values_fold() {
    ok("g.V().group().by(label()).by(values(\"name\").fold())");
}
#[test]
fn by_in_path() {
    ok("g.V().out().path().by(\"name\")");
}
#[test]
fn by_in_path_multi() {
    ok("g.V().out().path().by(\"name\").by(\"age\")");
}
#[test]
fn by_in_dedup() {
    ok("g.V().dedup().by(label())");
}
#[test]
fn by_in_dedup_key() {
    ok("g.V().dedup().by(\"name\")");
}
#[test]
fn by_select_then_by_keys() {
    ok("g.V().as(\"a\").out().as(\"b\").select(\"a\",\"b\").by(\"name\")");
}
#[test]
fn by_project_id_count() {
    ok("g.V().project(\"id\",\"n\").by(id()).by(out().count())");
}

// Math + by
#[test]
fn math_with_by_property() {
    ok("g.V().as(\"a\").out().as(\"b\").math(\"a + b\").by(\"age\")");
}
#[test]
fn math_simple() {
    ok("g.V().values(\"age\").math(\"_ * 2\")");
}
#[test]
fn math_with_two_vars() {
    ok("g.V().as(\"a\").out().as(\"b\").math(\"a / b\").by(\"age\")");
}

// Order by traversal
#[test]
fn order_by_values_age() {
    ok("g.V().order().by(values(\"age\"))");
}
#[test]
fn order_by_values_age_desc() {
    ok("g.V().order().by(values(\"age\"), desc)");
}
#[test]
fn order_by_out_count() {
    ok("g.V().order().by(out().count())");
}
#[test]
fn order_local_by_value() {
    ok("g.V().fold().order(local).by(values(\"name\"))");
}

// Group/groupCount with by
#[test]
fn group_by_label_by_count() {
    ok("g.V().group().by(label()).by(count())");
}
#[test]
fn group_count_by_label() {
    ok("g.V().groupCount().by(label())");
}
#[test]
fn group_by_complex_traversal() {
    ok("g.V().group().by(values(\"name\")).by(values(\"age\").fold())");
}

// Label-aware predicate (where with predicate refs)
#[test]
fn label_aware_where_eq() {
    ok("g.V().as(\"a\").out().where(neq(\"a\"))");
}
#[test]
fn label_aware_where_lt() {
    ok("g.V().as(\"a\").out().values(\"age\").as(\"b\").where(\"b\", gt(\"a\"))");
}
#[test]
fn label_aware_within_aggregate() {
    ok("g.V().aggregate(\"x\").out().where(within(\"x\"))");
}
#[test]
fn label_aware_where_combined() {
    ok("g.V().as(\"a\").out().as(\"b\").where(\"a\", neq(\"b\"))");
}

// MergeV / MergeE
#[test]
fn merge_v_simple() {
    ok("g.mergeV([\"name\":\"marko\"])");
}
#[test]
fn merge_v_with_label() {
    ok("g.mergeV([\"label\":\"person\",\"name\":\"marko\"])");
}
#[test]
fn merge_e_basic() {
    ok("g.V(1).as(\"a\").V(2).as(\"b\").mergeE([\"label\":\"knows\"])");
}

// Property cardinality / value types
#[test]
fn property_int() {
    ok("g.addV(\"x\").property(\"n\", 1)");
}
#[test]
fn property_float() {
    ok("g.addV(\"x\").property(\"n\", 1.5)");
}
#[test]
fn property_string() {
    ok("g.addV(\"x\").property(\"n\", \"hi\")");
}
#[test]
fn property_bool() {
    ok("g.addV(\"x\").property(\"n\", true)");
}
#[test]
fn property_null() {
    ok("g.V().property(\"n\", \"v\")");
}

// Subgraph (currently unsupported step in compiler — exercises the error path)
#[test]
fn sub_subgraph_assign() {
    err("g.V().outE().subgraph(\"sg\").cap(\"sg\")");
}

// Strategy — withStrategies covering different forms
#[test]
fn strat_seed_simple() {
    ok("g.withStrategies(new SeedStrategy(seed: 42)).V()");
}
#[test]
fn strat_productive_by() {
    ok("g.withStrategies(ProductiveByStrategy).V().group().by(\"missing\")");
}
#[test]
fn strat_reserved_keys() {
    ok("g.withStrategies(new ReservedKeysVerificationStrategy(throwException: true, keys: [\"age\"])).V()");
}
#[test]
fn strat_edge_label_verification() {
    ok("g.withStrategies(EdgeLabelVerificationStrategy).V()");
}
#[test]
fn strat_without_connective() {
    ok("g.withStrategies(new SeedStrategy(seed: 7)).V().count()");
}
#[test]
fn strat_with_side_effect_value() {
    ok("g.withSideEffect(\"a\", [1,2,3]).V().select(\"a\")");
}
#[test]
fn strat_chained_with() {
    ok("g.withSideEffect(\"a\", 1).withSideEffect(\"b\", 2).V().select(\"a\",\"b\")");
}

// Range / skip / tail variations
#[test]
fn rng_range_local_2() {
    ok("g.V().fold().range(local, 0, 2)");
}
#[test]
fn rng_skip_local_1() {
    ok("g.V().fold().skip(local, 1)");
}
#[test]
fn rng_tail_local_2() {
    ok("g.V().fold().tail(local, 2)");
}
#[test]
fn rng_sample_local() {
    ok("g.V().fold().sample(local, 2)");
}
#[test]
fn rng_dedup_local_no_keys() {
    ok("g.V().fold().dedup(local)");
}

// Repeat with both until & emit
#[test]
fn rep_until_then_emit() {
    ok("g.V().repeat(out()).until(has(\"name\",\"x\")).emit()");
}
#[test]
fn rep_emit_first() {
    ok("g.V().emit().repeat(out()).times(2)");
}
#[test]
fn rep_until_first() {
    ok("g.V().until(has(\"name\",\"x\")).repeat(out())");
}

// Choose with options for several keys
#[test]
fn choose_options_multi() {
    ok("g.V().choose(values(\"name\")).option(\"marko\", values(\"age\")).option(\"vadas\", values(\"id\"))");
}
#[test]
fn choose_traversal_no_options() {
    ok("g.V().choose(out(\"knows\"), values(\"name\"), values(\"id\"))");
}

// Coalesce / union nested
#[test]
fn coalesce_two_traversals() {
    ok("g.V().coalesce(out(\"knows\"), out(\"created\"))");
}
#[test]
fn union_three() {
    ok("g.V().union(out(), in(), both())");
}
#[test]
fn coalesce_then_dedup() {
    ok("g.V().coalesce(values(\"name\"), values(\"id\")).dedup()");
}

// Match with multiple branches
#[test]
fn match_two_branches() {
    ok("g.V().match(as(\"a\").out(\"knows\").as(\"b\"), as(\"b\").has(\"name\",\"vadas\"))");
}
#[test]
fn match_three_branches() {
    ok("g.V().match(as(\"a\").out(\"knows\").as(\"b\"), as(\"b\").out(\"created\").as(\"c\"), as(\"c\").has(\"name\",\"lop\"))");
}

// Inject + various ops
#[test]
fn inject_unfold_count() {
    ok("g.inject(1, 2, 3).count()");
}
#[test]
fn inject_then_is() {
    ok("g.inject(1, 2, 3).is(gt(1))");
}
#[test]
fn inject_then_fold_unfold() {
    ok("g.inject(1, 2, 3).fold().unfold()");
}
#[test]
fn inject_lists() {
    ok("g.inject([1,2], [3,4]).unfold()");
}

// SideEffect uses
#[test]
fn se_aggregate_to_cap() {
    ok("g.V().aggregate(\"x\").cap(\"x\")");
}
#[test]
fn se_named_aggregate_select() {
    ok("g.V().aggregate(\"x\").select(\"x\")");
}
#[test]
fn se_loops_named() {
    ok("g.V().repeat(out()).until(loops().is(2))");
}

// Strings — replace/split/concat permutations
#[test]
fn str_replace_chained() {
    ok("g.V().values(\"name\").replace(\"a\",\"b\").replace(\"c\",\"d\")");
}
#[test]
fn str_split_then_unfold() {
    ok("g.V().values(\"name\").split(\",\").unfold()");
}
#[test]
fn str_concat_strings() {
    ok("g.V().values(\"name\").concat(\"-suffix\")");
}
#[test]
fn str_format_two_args() {
    ok("g.V().values(\"name\").project(\"x\",\"y\").by().by(constant(1)).format(\"%s=%s\")");
}
#[test]
fn str_substring_local_fold() {
    ok("g.V().values(\"name\").fold().substring(local, 0, 3)");
}
#[test]
fn str_as_bool_value() {
    ok("g.V().values(\"is_active\").asBool()");
}
#[test]
fn str_as_date_simple() {
    ok("g.V().values(\"created\").asDate()");
}
#[test]
fn str_as_number_typed_int() {
    ok("g.V().values(\"age\").asNumber(int)");
}
#[test]
fn str_as_number_typed_long() {
    ok("g.V().values(\"age\").asNumber(long)");
}
#[test]
fn str_as_number_typed_float() {
    ok("g.V().values(\"age\").asNumber(float)");
}
#[test]
fn str_as_number_typed_double() {
    ok("g.V().values(\"age\").asNumber(double)");
}

// String op local family
#[test]
fn str_op_local_to_lower() {
    ok("g.V().values(\"name\").fold().toLower(local)");
}
#[test]
fn str_op_local_to_upper() {
    ok("g.V().values(\"name\").fold().toUpper(local)");
}
#[test]
fn str_op_local_trim() {
    ok("g.V().values(\"name\").fold().trim(local)");
}
#[test]
fn str_op_local_length() {
    ok("g.V().values(\"name\").fold().length(local)");
}
#[test]
fn str_op_local_reverse() {
    ok("g.V().values(\"name\").fold().reverse(local)");
}
#[test]
fn str_op_local_concat() {
    ok("g.V().values(\"name\").fold().concat(\"-\")");
}
#[test]
fn str_op_conjoin() {
    ok("g.V().values(\"name\").fold().conjoin(\",\")");
}

// HasNot / has hierarchies
#[test]
fn has_not_with_chain() {
    ok("g.V().hasNot(\"missing\").count()");
}
#[test]
fn has_chain_three() {
    ok("g.V().has(\"a\",1).has(\"b\",2).has(\"c\",3)");
}

// Properties / valueMap edge cases
#[test]
fn val_map_with_tokens_and_keys() {
    ok("g.V().valueMap(true, \"name\")");
}
#[test]
fn properties_then_value() {
    ok("g.V().properties(\"name\").value()");
}
#[test]
fn properties_then_key() {
    ok("g.V().properties(\"name\").key()");
}
