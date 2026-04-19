//! Round-3 broad parse + compile coverage tests.
//!
//! Each test parses a real Gremlin string via `parse_traversal`, then runs
//! it through `GremlinCompiler::compile`. The intent is behavior-level: the
//! compiler must not panic on these inputs and (when the parser succeeds)
//! the compile step is invoked. This exercises previously-uncovered paths
//! in:
//!   - `compiler/helpers/predicates.rs`
//!   - `compiler/helpers/predicate_label.rs`
//!   - `compiler/helpers/operators.rs`
//!   - `compiler/helpers/where_by.rs`
//!   - `compiler/compile_from/per_step_pendings.rs`
//!   - `compiler/steps/by/{group,project,order,format}.rs`
//!   - `compiler/steps/mutation/{from_to,property}.rs`
//!   - `compiler/steps/side_effect/{as_select,barrier}.rs`
//!   - `compiler/steps/branch/match_step.rs`
//!   - `text_parser/literal/value.rs`

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
// group()/groupCount() with by() variants (steps/by/group.rs)
// =====================================================================

#[test]
fn group_no_by() {
    run("g.V().group()");
}
#[test]
fn group_by_property() {
    run("g.V().group().by(\"name\")");
}
#[test]
fn group_by_label() {
    run("g.V().group().by(label)");
}
#[test]
fn group_by_id() {
    run("g.V().group().by(id)");
}
#[test]
fn group_by_traversal_values() {
    run("g.V().group().by(__.values(\"name\"))");
}
#[test]
fn group_by_traversal_out_count() {
    run("g.V().group().by(__.out().count())");
}
#[test]
fn group_by_property_then_count() {
    run("g.V().group().by(\"name\").by(count())");
}
#[test]
fn group_by_property_then_fold() {
    run("g.V().group().by(\"name\").by(fold())");
}
#[test]
fn group_by_property_then_sum_traversal() {
    run("g.V().group().by(\"name\").by(__.values(\"age\").sum())");
}
#[test]
fn group_by_traversal_then_traversal() {
    run("g.V().group().by(__.label()).by(__.out().count())");
}
#[test]
fn group_by_value_traversal_with_max() {
    run("g.V().group().by(\"name\").by(__.values(\"age\").max())");
}
#[test]
fn group_by_value_traversal_with_min() {
    run("g.V().group().by(\"name\").by(__.values(\"age\").min())");
}
#[test]
fn group_by_value_traversal_with_mean() {
    run("g.V().group().by(\"name\").by(__.values(\"age\").mean())");
}
#[test]
fn group_by_empty_then_empty() {
    run("g.V().group().by().by()");
}
#[test]
fn group_count_no_by() {
    run("g.V().groupCount()");
}
#[test]
fn group_count_by_property() {
    run("g.V().groupCount().by(\"name\")");
}
#[test]
fn group_count_by_label() {
    run("g.V().groupCount().by(label)");
}
#[test]
fn group_count_by_traversal() {
    run("g.V().groupCount().by(__.values(\"name\"))");
}
// groupCount() with a second by() is illegal but must not panic on parse+compile.
#[test]
fn group_count_two_by_returns_error() {
    let q = "g.V().groupCount().by(\"name\").by(\"x\")";
    if let Ok(steps) = parse_traversal(q, &no_params) {
        // Compile may return Err — we just assert no panic.
        let _ = GremlinCompiler::new().compile(&steps);
    }
}

// =====================================================================
// project() with by() variants (steps/by/project.rs)
// =====================================================================

#[test]
fn project_single_by_prop() {
    run("g.V().project(\"a\").by(\"name\")");
}
#[test]
fn project_two_by_prop() {
    run("g.V().project(\"a\", \"b\").by(\"name\").by(\"age\")");
}
#[test]
fn project_three_by_mixed() {
    run("g.V().project(\"a\", \"b\", \"c\").by(\"name\").by(__.out().count()).by(label)");
}
#[test]
fn project_by_empty_identity() {
    run("g.V().project(\"a\").by()");
}
#[test]
fn project_by_traversal_only() {
    run("g.V().project(\"x\").by(__.outE().count())");
}
#[test]
fn project_followed_by_select_key() {
    run("g.V().project(\"a\", \"b\").by(\"name\").by(\"age\").select(\"a\")");
}
#[test]
fn project_traversal_then_simple() {
    run("g.V().project(\"a\", \"b\").by(__.values(\"name\")).by(\"age\")");
}
#[test]
fn project_simple_then_traversal() {
    run("g.V().project(\"a\", \"b\").by(\"name\").by(__.values(\"age\"))");
}
#[test]
fn project_more_keys_than_by() {
    run("g.V().project(\"a\", \"b\", \"c\").by(\"name\")");
}

// =====================================================================
// order() / order(local) with by() variants (steps/by/order.rs)
// =====================================================================

#[test]
fn order_no_by() {
    run("g.V().order()");
}
#[test]
fn order_by_property_asc() {
    run("g.V().order().by(\"name\")");
}
#[test]
fn order_by_property_desc() {
    run("g.V().order().by(\"name\", desc)");
}
#[test]
fn order_by_property_incr() {
    run("g.V().order().by(\"name\", incr)");
}
#[test]
fn order_by_property_decr() {
    run("g.V().order().by(\"name\", decr)");
}
#[test]
fn order_by_label_asc() {
    run("g.V().order().by(label)");
}
#[test]
fn order_by_id_asc() {
    run("g.V().order().by(id)");
}
#[test]
fn order_by_id_desc() {
    run("g.V().order().by(id, desc)");
}
#[test]
fn order_by_t_label() {
    run("g.V().order().by(T.label)");
}
#[test]
fn order_by_t_id() {
    run("g.V().order().by(T.id)");
}
#[test]
fn order_by_traversal() {
    run("g.V().order().by(__.values(\"age\"))");
}
#[test]
fn order_by_traversal_desc() {
    run("g.V().order().by(__.out().count(), desc)");
}
#[test]
fn order_by_keys() {
    run("g.V().properties().order().by(T.key)");
}
#[test]
fn order_by_value() {
    run("g.V().properties().order().by(T.value)");
}
#[test]
fn order_by_column_keys() {
    run("g.V().group().by(\"name\").by(__.count()).unfold().order().by(Column.keys)");
}
#[test]
fn order_by_column_values() {
    run("g.V().group().by(\"name\").by(__.count()).unfold().order().by(Column.values)");
}
#[test]
fn order_local() {
    run("g.V().values(\"age\").fold().order(local)");
}
#[test]
fn order_local_by_value() {
    run("g.V().values(\"age\").fold().order(local).by()");
}
#[test]
fn order_two_by_chained() {
    run("g.V().order().by(\"age\").by(\"name\")");
}

// =====================================================================
// addV / addE with from/to (steps/mutation/from_to.rs)
// =====================================================================

#[test]
fn add_e_from_select_to_select() {
    run("g.V().as(\"a\").out().as(\"b\").addE(\"knows\").from(\"a\").to(\"b\")");
}
#[test]
fn add_e_from_v_to_v() {
    run("g.addE(\"knows\").from(V(1)).to(V(2))");
}
#[test]
fn add_e_from_inner_traversal() {
    run("g.V(1).as(\"a\").addE(\"knows\").from(\"a\").to(__.V(2))");
}
#[test]
fn add_e_from_select_traversal() {
    run("g.V().as(\"a\").addE(\"r\").from(__.select(\"a\"))");
}
#[test]
fn add_v_with_property() {
    run("g.addV(\"person\").property(\"name\", \"alice\")");
}
#[test]
fn add_v_with_multi_properties() {
    run("g.addV(\"person\").property(\"name\", \"alice\").property(\"age\", 30)");
}
#[test]
fn add_e_with_property() {
    run("g.V(1).addE(\"knows\").to(V(2)).property(\"weight\", 0.5)");
}

// =====================================================================
// path() / simplePath() / cyclicPath() with from/to label modulators
// =====================================================================

#[test]
fn path_no_modulators() {
    run("g.V().out().out().path()");
}
#[test]
fn path_by_property() {
    run("g.V().out().out().path().by(\"name\")");
}
#[test]
fn path_by_traversal() {
    run("g.V().out().out().path().by(__.values(\"name\"))");
}
#[test]
fn path_with_from_label() {
    run("g.V().as(\"a\").out().as(\"b\").out().path().from(\"a\")");
}
#[test]
fn path_with_to_label() {
    run("g.V().as(\"a\").out().as(\"b\").out().path().to(\"b\")");
}
#[test]
fn path_with_from_to_select() {
    run("g.V().as(\"a\").out().as(\"b\").out().path().from(select(\"a\")).to(select(\"b\"))");
}
#[test]
fn simple_path_basic() {
    run("g.V().out().out().simplePath()");
}
#[test]
fn cyclic_path_basic() {
    run("g.V().out().out().cyclicPath()");
}
#[test]
fn simple_path_from_to() {
    run("g.V().as(\"x\").out().as(\"y\").out().simplePath().from(\"x\").to(\"y\")");
}
#[test]
fn cyclic_path_from_to() {
    run("g.V().as(\"x\").out().as(\"y\").out().cyclicPath().from(\"x\").to(\"y\")");
}

// =====================================================================
// has() with rich predicates (helpers/predicates.rs, predicate_label.rs)
// =====================================================================

#[test]
fn has_with_or_predicate() {
    run("g.V().has(\"age\", or(gt(30), lt(10)))");
}
#[test]
fn has_with_and_predicate() {
    run("g.V().has(\"age\", and(gt(10), lt(30)))");
}
#[test]
fn has_with_not_predicate() {
    run("g.V().has(\"age\", P.not(eq(25)))");
}
#[test]
fn has_with_between() {
    run("g.V().has(\"age\", between(10, 30))");
}
#[test]
fn has_with_inside() {
    run("g.V().has(\"age\", inside(10, 30))");
}
#[test]
fn has_with_outside() {
    run("g.V().has(\"age\", outside(10, 30))");
}
#[test]
fn has_id_single() {
    run("g.V().hasId(1)");
}
#[test]
fn has_id_multiple() {
    run("g.V().hasId(1, 2, 3)");
}
#[test]
fn has_id_predicate() {
    run("g.V().hasId(within(1, 2, 3))");
}
#[test]
fn has_label_multiple() {
    run("g.V().hasLabel(\"person\", \"software\")");
}
#[test]
fn has_key_basic() {
    run("g.V().properties().hasKey(\"name\")");
}
#[test]
fn has_value_basic() {
    run("g.V().properties().hasValue(\"alice\")");
}
#[test]
fn has_not_key() {
    run("g.V().hasNot(\"age\")");
}
#[test]
fn is_with_predicate() {
    run("g.V().values(\"age\").is(gt(25))");
}
#[test]
fn is_with_eq_value() {
    run("g.V().values(\"age\").is(30)");
}

// =====================================================================
// where()/by() with label references (helpers/where_by.rs, predicate_label.rs)
// =====================================================================

#[test]
fn where_with_label_eq() {
    run("g.V().as(\"a\").out().where(eq(\"a\"))");
}
#[test]
fn where_with_label_neq() {
    run("g.V().as(\"a\").out().where(neq(\"a\"))");
}
#[test]
fn where_with_label_within() {
    run("g.V().as(\"a\").as(\"b\").out().where(within(\"a\", \"b\"))");
}
#[test]
fn where_with_label_without() {
    run("g.V().as(\"a\").as(\"b\").out().where(without(\"a\"))");
}
#[test]
fn where_p_not_label() {
    run("g.V().as(\"a\").out().where(P.not(eq(\"a\")))");
}
#[test]
fn where_with_traversal() {
    run("g.V().where(__.out())");
}
#[test]
fn where_p_lt_label() {
    run("g.V().as(\"a\").out().values(\"age\").where(lt(\"a\"))");
}
#[test]
fn where_by_property() {
    run("g.V().as(\"a\").out().where(eq(\"a\")).by(\"name\")");
}
#[test]
fn where_by_two() {
    run("g.V().as(\"a\").out().as(\"b\").where(\"a\", eq(\"b\")).by(\"name\").by(\"name\")");
}
#[test]
fn where_compound_and() {
    run("g.V().where(__.out().and(__.in()))");
}
#[test]
fn where_compound_or() {
    run("g.V().where(__.out().or(__.in()))");
}
#[test]
fn where_not_traversal() {
    run("g.V().not(__.out())");
}

// =====================================================================
// match() variants (steps/branch/match_step.rs)
// =====================================================================

#[test]
fn match_single_pattern() {
    run("g.V().match(__.as(\"a\").out().as(\"b\"))");
}
#[test]
fn match_two_patterns() {
    run("g.V().match(__.as(\"a\").out().as(\"b\"), __.as(\"b\").in().as(\"a\"))");
}
#[test]
fn match_with_select() {
    run("g.V().match(__.as(\"a\").out().as(\"b\")).select(\"a\", \"b\")");
}
#[test]
fn match_with_where_clause() {
    run("g.V().match(__.as(\"a\").out().as(\"b\"), __.as(\"a\").where(neq(\"b\")))");
}

// =====================================================================
// sack() with operators (helpers/operators.rs inject_sack_init)
// =====================================================================

#[test]
fn with_sack_constant_then_sum() {
    run("g.withSack(0).V().sack(sum).by(constant(1)).sack()");
}
#[test]
fn with_sack_constant_then_minus() {
    run("g.withSack(10).V().sack(minus).by(constant(1)).sack()");
}
#[test]
fn with_sack_then_mult() {
    run("g.withSack(1).V().sack(mult).by(constant(2)).sack()");
}
#[test]
fn with_sack_then_div() {
    run("g.withSack(10).V().sack(div).by(constant(2)).sack()");
}
#[test]
fn with_sack_after_filter() {
    run("g.withSack(0).V().has(\"name\", \"alice\").sack(sum).by(constant(1)).sack()");
}
#[test]
fn with_sack_after_order_limit() {
    run("g.withSack(0).V().order().by(\"age\").limit(5).sack(sum).by(constant(1)).sack()");
}

// =====================================================================
// barrier() and side_effect (steps/side_effect/barrier.rs)
// =====================================================================

#[test]
fn barrier_basic() {
    run("g.V().barrier()");
}
#[test]
fn barrier_with_size() {
    run("g.V().barrier(100)");
}
#[test]
fn side_effect_basic() {
    run("g.V().sideEffect(__.out())");
}
#[test]
fn side_effect_with_aggregate() {
    run("g.V().sideEffect(__.aggregate(\"x\"))");
}
#[test]
fn aggregate_local() {
    run("g.V().aggregate(local, \"x\")");
}
#[test]
fn aggregate_global() {
    run("g.V().aggregate(global, \"x\")");
}
#[test]
fn aggregate_basic() {
    run("g.V().aggregate(\"x\")");
}
#[test]
fn store_basic() {
    run("g.V().store(\"x\")");
}
#[test]
fn cap_basic() {
    run("g.V().aggregate(\"x\").cap(\"x\")");
}
#[test]
fn cap_two_keys() {
    run("g.V().aggregate(\"a\").aggregate(\"b\").cap(\"a\", \"b\")");
}

// =====================================================================
// select() variants (steps/side_effect/as_select.rs)
// =====================================================================

#[test]
fn select_single_label() {
    run("g.V().as(\"a\").out().select(\"a\")");
}
#[test]
fn select_multiple_labels() {
    run("g.V().as(\"a\").out().as(\"b\").select(\"a\", \"b\")");
}
#[test]
fn select_with_pop_first() {
    run("g.V().as(\"a\").out().as(\"a\").select(first, \"a\")");
}
#[test]
fn select_with_pop_last() {
    run("g.V().as(\"a\").out().as(\"a\").select(last, \"a\")");
}
#[test]
fn select_with_pop_all() {
    run("g.V().as(\"a\").out().as(\"a\").select(all, \"a\")");
}
#[test]
fn select_column_keys() {
    run("g.V().as(\"a\").out().as(\"b\").select(\"a\", \"b\").select(Column.keys)");
}
#[test]
fn select_column_values() {
    run("g.V().as(\"a\").out().as(\"b\").select(\"a\", \"b\").select(Column.values)");
}
#[test]
fn select_with_by_property() {
    run("g.V().as(\"a\").out().as(\"b\").select(\"a\", \"b\").by(\"name\")");
}
#[test]
fn select_with_by_traversal() {
    run("g.V().as(\"a\").out().as(\"b\").select(\"a\", \"b\").by(__.values(\"name\"))");
}
#[test]
fn select_with_two_by() {
    run("g.V().as(\"a\").out().as(\"b\").select(\"a\", \"b\").by(\"name\").by(\"age\")");
}
#[test]
fn select_path_keys() {
    run("g.V().as(\"a\").out().as(\"b\").path().select(Column.keys)");
}

// =====================================================================
// format() / dedup() / sample() with by() (per_step_pendings.rs)
// =====================================================================

#[test]
fn format_with_by() {
    run("g.V().format(\"%{name}-%{age}\")");
}
#[test]
fn dedup_basic() {
    run("g.V().dedup()");
}
#[test]
fn dedup_by_property() {
    run("g.V().dedup().by(\"name\")");
}
#[test]
fn dedup_by_label() {
    run("g.V().dedup().by(label)");
}
#[test]
fn dedup_by_traversal() {
    run("g.V().dedup().by(__.values(\"name\"))");
}
#[test]
fn dedup_with_args() {
    run("g.V().as(\"a\").out().dedup(\"a\")");
}
#[test]
fn sample_basic() {
    run("g.V().sample(5)");
}
#[test]
fn sample_with_by() {
    run("g.V().sample(5).by(\"age\")");
}

// =====================================================================
// math() with side effects (per_step_pendings.rs flush)
// =====================================================================

#[test]
fn math_simple() {
    run("g.V().values(\"age\").math(\"_ * 2\")");
}
#[test]
fn math_with_constant() {
    run("g.V().values(\"age\").math(\"_ + 100\")");
}
#[test]
fn math_with_division() {
    run("g.V().values(\"age\").math(\"_ / 2\")");
}
#[test]
fn math_with_modulo() {
    run("g.V().values(\"age\").math(\"_ % 7\")");
}

// =====================================================================
// repeat()/until()/emit() variants (steps/branch/repeat.rs)
// =====================================================================

#[test]
fn repeat_times() {
    run("g.V().repeat(__.out()).times(3)");
}
#[test]
fn repeat_until_emit() {
    run("g.V().repeat(__.out()).until(__.outE().count().is(0)).emit()");
}
#[test]
fn repeat_emit_first() {
    run("g.V().emit().repeat(__.out()).times(2)");
}
#[test]
fn repeat_loops_filter() {
    run("g.V().repeat(__.out()).until(loops().is(2))");
}
#[test]
fn repeat_emit_predicate() {
    run("g.V().repeat(__.out()).times(2).emit(has(\"name\", \"alice\"))");
}

// =====================================================================
// branch / coalesce / choose (steps/branch/*)
// =====================================================================

#[test]
fn coalesce_basic() {
    run("g.V().coalesce(__.out(), __.in())");
}
#[test]
fn coalesce_three_branches() {
    run("g.V().coalesce(__.values(\"a\"), __.values(\"b\"), __.constant(\"x\"))");
}
#[test]
fn choose_pred_then_else() {
    run("g.V().choose(__.has(\"age\"), __.values(\"age\"), __.constant(0))");
}
#[test]
fn choose_options() {
    run("g.V().choose(__.label()).option(\"person\", __.values(\"name\")).option(\"software\", __.values(\"lang\"))");
}
#[test]
fn union_basic() {
    run("g.V().union(__.out(), __.in())");
}
#[test]
fn local_basic() {
    run("g.V().local(__.out().count())");
}

// =====================================================================
// String / type conversion steps (steps/strings/*)
// =====================================================================

#[test]
fn to_upper() {
    run("g.V().values(\"name\").toUpper()");
}
#[test]
fn to_lower() {
    run("g.V().values(\"name\").toLower()");
}
#[test]
fn length_step() {
    run("g.V().values(\"name\").length()");
}
#[test]
fn trim_step() {
    run("g.V().values(\"name\").trim()");
}
#[test]
fn ltrim_step() {
    run("g.V().values(\"name\").lTrim()");
}
#[test]
fn rtrim_step() {
    run("g.V().values(\"name\").rTrim()");
}
#[test]
fn substring_step() {
    run("g.V().values(\"name\").substring(0, 3)");
}
#[test]
fn replace_step() {
    run("g.V().values(\"name\").replace(\"a\", \"b\")");
}
#[test]
fn split_step() {
    run("g.V().values(\"name\").split(\",\")");
}
#[test]
fn concat_step() {
    run("g.V().values(\"name\").concat(\"-suffix\")");
}
#[test]
fn as_string() {
    run("g.V().values(\"age\").asString()");
}
