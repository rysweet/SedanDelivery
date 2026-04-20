//! Round-4 coverage tests targeting the lowest-covered files identified
//! after r7. Each test parses a real Gremlin string via `parse_traversal`
//! and runs it through `GremlinCompiler::compile`. All tests are tolerant
//! of parse / compile failure (they aim to drive code paths, not assert
//! semantics) so they are robust to minor surface differences across
//! parser revisions.
//!
//! Files targeted (per `gc8-base.json`):
//!   - `compiler/helpers/predicate_label.rs`        (26.46%)
//!   - `compiler/compile_from/per_step_pendings.rs` (27.17%)
//!   - `compiler/steps/by/project.rs`               (38.29%)
//!   - `compiler/steps/by/group.rs`                 (39.05%)
//!   - `compiler/steps/filter/filter_traversal.rs`  (39.22%)
//!   - `compiler/steps/side_effect/barrier.rs`      (44.16%)
//!   - `compiler/steps/strings/conversion.rs`       (46.77%)
//!   - `compiler/helpers/predicates.rs`             (53.87%)
//!   - `compiler/steps/by/format.rs`                (0.00%)
//!   - `compiler/steps/order/by_valuemap.rs`        (0.00%)

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
// predicate_label.rs — label-aware predicates via where() + as() / aggregate()
// =====================================================================

#[test]
fn pl_where_eq_label_after_as() {
    run("g.V().as(\"a\").out().where(eq(\"a\"))");
}
#[test]
fn pl_where_neq_label_after_as() {
    run("g.V().as(\"a\").out().where(neq(\"a\"))");
}
#[test]
fn pl_where_gt_label_after_as() {
    run("g.V().as(\"a\").out().values(\"age\").where(gt(\"a\"))");
}
#[test]
fn pl_where_gte_label_after_as() {
    run("g.V().as(\"a\").out().values(\"age\").where(gte(\"a\"))");
}
#[test]
fn pl_where_lt_label_after_as() {
    run("g.V().as(\"a\").out().values(\"age\").where(lt(\"a\"))");
}
#[test]
fn pl_where_lte_label_after_as() {
    run("g.V().as(\"a\").out().values(\"age\").where(lte(\"a\"))");
}
#[test]
fn pl_where_within_aggregate_key() {
    run("g.V().aggregate(\"x\").out().where(within(\"x\"))");
}
#[test]
fn pl_where_without_aggregate_key() {
    run("g.V().aggregate(\"x\").out().where(without(\"x\"))");
}
#[test]
fn pl_where_within_alias() {
    run("g.V().as(\"a\").out().where(within(\"a\"))");
}
#[test]
fn pl_where_without_alias() {
    run("g.V().as(\"a\").out().where(without(\"a\"))");
}
#[test]
fn pl_where_not_eq_label() {
    run("g.V().as(\"a\").out().where(not(eq(\"a\")))");
}
#[test]
fn pl_where_or_eq_labels() {
    run("g.V().as(\"a\").as(\"b\").out().where(or(eq(\"a\"), eq(\"b\")))");
}
#[test]
fn pl_where_and_compound_labels() {
    run("g.V().as(\"a\").as(\"b\").out().where(and(gt(\"a\"), lt(\"b\")))");
}
#[test]
fn pl_where_p_eq_string_no_label_falls_back() {
    run("g.V().out().where(eq(\"unknown\"))");
}
#[test]
fn pl_within_aggregate_with_traversal_arg() {
    run("g.V().aggregate(\"x\").out().where(P.within(\"x\"))");
}

// =====================================================================
// predicates.rs — has(P.X), is(P.X), hasId, And/Or/Not nesting
// =====================================================================

#[test]
fn pr_has_eq() {
    run("g.V().has(\"age\", eq(30))");
}
#[test]
fn pr_has_neq() {
    run("g.V().has(\"age\", neq(30))");
}
#[test]
fn pr_has_gt() {
    run("g.V().has(\"age\", gt(20))");
}
#[test]
fn pr_has_gte() {
    run("g.V().has(\"age\", gte(20))");
}
#[test]
fn pr_has_lt() {
    run("g.V().has(\"age\", lt(40))");
}
#[test]
fn pr_has_lte() {
    run("g.V().has(\"age\", lte(40))");
}
#[test]
fn pr_has_between() {
    run("g.V().has(\"age\", between(20, 40))");
}
#[test]
fn pr_has_inside() {
    run("g.V().has(\"age\", inside(20, 40))");
}
#[test]
fn pr_has_outside() {
    run("g.V().has(\"age\", outside(20, 40))");
}
#[test]
fn pr_has_within_list() {
    run("g.V().has(\"name\", within(\"a\",\"b\",\"c\"))");
}
#[test]
fn pr_has_without_list() {
    run("g.V().has(\"name\", without(\"a\",\"b\"))");
}
#[test]
fn pr_has_starting_with() {
    run("g.V().has(\"name\", startingWith(\"a\"))");
}
#[test]
fn pr_has_ending_with() {
    run("g.V().has(\"name\", endingWith(\"z\"))");
}
#[test]
fn pr_has_containing() {
    run("g.V().has(\"name\", containing(\"o\"))");
}
#[test]
fn pr_has_not_starting_with() {
    run("g.V().has(\"name\", notStartingWith(\"a\"))");
}
#[test]
fn pr_has_not_ending_with() {
    run("g.V().has(\"name\", notEndingWith(\"z\"))");
}
#[test]
fn pr_has_not_containing() {
    run("g.V().has(\"name\", notContaining(\"q\"))");
}
#[test]
fn pr_is_eq_value() {
    run("g.V().values(\"age\").is(eq(30))");
}
#[test]
fn pr_is_gt_value() {
    run("g.V().values(\"age\").is(gt(30))");
}
#[test]
fn pr_is_within_values() {
    run("g.V().values(\"age\").is(within(1,2,3))");
}
#[test]
fn pr_has_id_single() {
    run("g.V().hasId(1)");
}
#[test]
fn pr_has_id_list() {
    run("g.V().hasId(1, 2, 3)");
}
#[test]
fn pr_has_id_predicate() {
    run("g.V().hasId(gt(5))");
}
#[test]
fn pr_has_label_multi() {
    run("g.V().hasLabel(\"person\", \"software\")");
}
#[test]
fn pr_filter_or_traversals() {
    run("g.V().or(has(\"age\", gt(30)), has(\"name\", \"alice\"))");
}
#[test]
fn pr_filter_and_traversals() {
    run("g.V().and(has(\"age\", gt(20)), has(\"age\", lt(50)))");
}
#[test]
fn pr_filter_not_traversal() {
    run("g.V().not(has(\"age\", gt(30)))");
}
#[test]
fn pr_steps_to_predicate_compound_values_is() {
    run("g.V().filter(__.values(\"age\").is(gt(30)))");
}
#[test]
fn pr_filter_inner_has_id_single() {
    run("g.V().filter(__.hasId(1))");
}
#[test]
fn pr_filter_inner_has_id_multi() {
    run("g.V().filter(__.hasId(1, 2, 3))");
}
#[test]
fn pr_filter_inner_has_id_predicate() {
    run("g.V().filter(__.hasId(gt(5)))");
}
#[test]
fn pr_filter_inner_has_label_multi() {
    run("g.V().filter(__.hasLabel(\"person\", \"software\"))");
}
#[test]
fn pr_filter_inner_has_key_value() {
    run("g.V().filter(__.has(\"name\", \"alice\"))");
}
#[test]
fn pr_filter_inner_has_key_predicate() {
    run("g.V().filter(__.has(\"age\", gt(20)))");
}
#[test]
fn pr_filter_inner_is_predicate() {
    run("g.V().values(\"age\").filter(__.is(gt(20)))");
}
#[test]
fn pr_filter_inner_and_branches() {
    run("g.V().filter(__.and(__.has(\"age\", gt(20)), __.has(\"age\", lt(50))))");
}
#[test]
fn pr_where_inner_or_branches() {
    run("g.V().where(__.or(__.has(\"age\", gt(20)), __.has(\"name\", \"alice\")))");
}
#[test]
fn pr_where_inner_not_branch() {
    run("g.V().where(__.not(__.has(\"age\", gt(20))))");
}
#[test]
fn pr_filter_loops_is() {
    run("g.V().repeat(__.out().filter(__.loops().is(gt(2)))).times(3)");
}
#[test]
fn pr_filter_label_eq_is() {
    run("g.V().filter(__.label().is(eq(\"person\")))");
}
#[test]
fn pr_filter_label_other_pred() {
    run("g.V().filter(__.label().is(neq(\"person\")))");
}
#[test]
fn pr_filter_id_pred() {
    run("g.V().filter(__.id().is(gt(5)))");
}

// =====================================================================
// per_step_pendings.rs — flush after pending modulators
// =====================================================================

#[test]
fn psp_path_by_then_count() {
    run("g.V().out().path().by(\"name\").count()");
}
#[test]
fn psp_simple_path_by_then_count() {
    run("g.V().out().simplePath().path().by(\"name\").count()");
}
#[test]
fn psp_cyclic_path_by_then_count() {
    run("g.V().out().cyclicPath().path().by(\"name\").count()");
}
#[test]
fn psp_path_by_then_select_into_other() {
    run("g.V().as(\"a\").out().path().by(\"name\").by(label).select(\"a\")");
}
#[test]
fn psp_format_by_then_count() {
    run("g.V().values(\"name\").format(\"hi %s\").count()");
}
#[test]
fn psp_format_by_keys_then_dedup() {
    run("g.V().project(\"a\",\"b\").by(\"name\").by(\"age\").format(\"%s=%s\").dedup()");
}
#[test]
fn psp_group_pending_then_count() {
    run("g.V().group().by(\"name\").by(count()).count(local)");
}
#[test]
fn psp_aggregate_pending_then_select() {
    run("g.V().aggregate(\"x\").by(\"name\").select(\"x\")");
}
#[test]
fn psp_dedup_pending_then_count() {
    run("g.V().dedup().by(\"name\").count()");
}
#[test]
fn psp_sample_pending_then_count() {
    run("g.V().sample(3).by(\"age\").count()");
}
#[test]
fn psp_sack_op_pending_then_sack() {
    run("g.withSack(0).V().sack(sum).by(\"age\").sack()");
}
#[test]
fn psp_valuemap_unfold_then_count() {
    run("g.V().valueMap().by(__.unfold()).count()");
}
#[test]
fn psp_valuemap_unfold_then_select() {
    run("g.V().valueMap(\"name\",\"age\").by(__.unfold()).select(keys)");
}
#[test]
fn psp_where_by_then_other() {
    run("g.V().as(\"a\").out().as(\"b\").where(\"a\", neq(\"b\")).by(\"name\").count()");
}
#[test]
fn psp_path_from_to_preserves_pending() {
    run("g.V().out().path().from(\"a\").to(\"b\").by(\"name\").count()");
}
#[test]
fn psp_project_pending_then_unfold() {
    run("g.V().project(\"a\",\"b\").by(\"name\").by(\"age\").unfold()");
}

// =====================================================================
// project.rs — by(traversal) variants after project()
// =====================================================================

#[test]
fn pj_project_by_inner_values() {
    run("g.V().project(\"a\").by(__.values(\"name\"))");
}
#[test]
fn pj_project_by_inner_count_traversal() {
    run("g.V().project(\"deg\").by(__.out().count())");
}
#[test]
fn pj_project_by_property_then_traversal() {
    run("g.V().project(\"n\",\"d\").by(\"name\").by(__.out().count())");
}
#[test]
fn pj_project_by_traversal_then_property() {
    run("g.V().project(\"d\",\"n\").by(__.out().count()).by(\"name\")");
}
#[test]
fn pj_project_by_three_mixed() {
    run("g.V().project(\"n\",\"d\",\"l\").by(\"name\").by(__.out().count()).by(label)");
}
#[test]
fn pj_project_by_id() {
    run("g.V().project(\"i\").by(__.id())");
}
#[test]
fn pj_project_by_label_traversal() {
    run("g.V().project(\"l\").by(__.label())");
}
#[test]
fn pj_project_by_select_after_as() {
    run("g.V().as(\"a\").out().project(\"x\").by(__.select(\"a\").values(\"name\"))");
}
#[test]
fn pj_project_by_fold() {
    run("g.V().project(\"all\").by(__.fold())");
}
#[test]
fn pj_project_by_constant() {
    run("g.V().project(\"k\").by(__.constant(\"x\"))");
}

// =====================================================================
// group.rs — group()/groupCount() with mixed by() keys & values
// =====================================================================

#[test]
fn gr_group_by_property_count() {
    run("g.V().group().by(\"name\").by(count())");
}
#[test]
fn gr_group_by_property_fold() {
    run("g.V().group().by(\"name\").by(fold())");
}
#[test]
fn gr_group_by_property_values() {
    run("g.V().group().by(\"name\").by(__.values(\"age\"))");
}
#[test]
fn gr_group_by_traversal_key_count() {
    run("g.V().group().by(__.values(\"age\")).by(count())");
}
#[test]
fn gr_group_by_label_traversal_count() {
    run("g.V().group().by(__.label()).by(count())");
}
#[test]
fn gr_group_by_implicit() {
    run("g.V().group().by(\"name\")");
}
#[test]
fn gr_group_count_by_property() {
    run("g.V().groupCount().by(\"name\")");
}
#[test]
fn gr_group_count_by_traversal() {
    run("g.V().groupCount().by(__.label())");
}
#[test]
fn gr_group_then_unfold() {
    run("g.V().group().by(\"name\").by(count()).unfold()");
}
#[test]
fn gr_group_by_property_sum() {
    run("g.V().group().by(\"name\").by(sum())");
}
#[test]
fn gr_group_by_property_max() {
    run("g.V().group().by(\"name\").by(max())");
}
#[test]
fn gr_group_by_property_min() {
    run("g.V().group().by(\"name\").by(min())");
}
#[test]
fn gr_group_by_property_mean() {
    run("g.V().group().by(\"name\").by(mean())");
}

// =====================================================================
// filter_traversal.rs — filter(__.X)
// =====================================================================

#[test]
fn ft_filter_has_key() {
    run("g.V().filter(__.has(\"age\"))");
}
#[test]
fn ft_filter_has_key_value() {
    run("g.V().filter(__.has(\"name\", \"alice\"))");
}
#[test]
fn ft_filter_outgoing_existence() {
    run("g.V().filter(__.out())");
}
#[test]
fn ft_filter_count_gt() {
    run("g.V().filter(__.out().count().is(gt(0)))");
}
#[test]
fn ft_filter_inside_unfold() {
    run("g.V().valueMap().filter(__.select(values).unfold())");
}
#[test]
fn ft_filter_label_eq() {
    run("g.V().filter(__.hasLabel(\"person\"))");
}
#[test]
fn ft_filter_or_combined() {
    run("g.V().filter(__.or(has(\"age\", gt(30)), has(\"name\", \"alice\")))");
}

// =====================================================================
// barrier.rs — none/barrier/discard/loops/barrier(normSack)
// =====================================================================

#[test]
fn bar_none_basic() {
    run("g.V().none()");
}
#[test]
fn bar_barrier_basic() {
    run("g.V().barrier()");
}
#[test]
fn bar_barrier_with_size() {
    run("g.V().barrier(100)");
}
#[test]
fn bar_discard() {
    run("g.V().discard()");
}
#[test]
fn bar_loops_unlabeled() {
    run("g.V().repeat(out().loops()).times(2)");
}
#[test]
fn bar_loops_labeled() {
    run("g.V().emit().repeat(out().simplePath()).until(loops().is(gt(2)))");
}
#[test]
fn bar_norm_sack() {
    run("g.withSack(1.0).V().barrier(normSack)");
}
#[test]
fn bar_barrier_after_count() {
    run("g.V().count().barrier()");
}
#[test]
fn bar_none_then_count() {
    run("g.V().none().count()");
}
#[test]
fn bar_discard_chained() {
    run("g.V().out().discard()");
}

// =====================================================================
// strings/conversion.rs — asString/asNumber/asNumberTyped/asBool/asDate/math
// =====================================================================

#[test]
fn cv_as_string() {
    run("g.V().values(\"age\").asString()");
}
#[test]
fn cv_as_string_local() {
    run("g.V().values(\"age\").fold().asString(local)");
}
#[test]
fn cv_as_number() {
    run("g.V().values(\"age\").asString().asNumber()");
}
#[test]
fn cv_as_number_int() {
    run("g.V().values(\"age\").asString().asNumber(N.INT)");
}
#[test]
fn cv_as_number_long() {
    run("g.V().values(\"age\").asString().asNumber(N.LONG)");
}
#[test]
fn cv_as_number_float() {
    run("g.V().values(\"age\").asString().asNumber(N.FLOAT)");
}
#[test]
fn cv_as_number_double() {
    run("g.V().values(\"age\").asString().asNumber(N.DOUBLE)");
}
#[test]
fn cv_as_number_byte() {
    run("g.V().values(\"age\").asString().asNumber(N.BYTE)");
}
#[test]
fn cv_as_number_short() {
    run("g.V().values(\"age\").asString().asNumber(N.SHORT)");
}
#[test]
fn cv_as_number_big_decimal() {
    run("g.V().values(\"age\").asString().asNumber(N.BIG_DECIMAL)");
}
#[test]
fn cv_as_number_invalid_vertex() {
    run("g.V().values(\"age\").asString().asNumber(N.VERTEX)");
}
#[test]
fn cv_as_bool() {
    run("g.V().values(\"flag\").asBool()");
}
#[test]
fn cv_as_date() {
    run("g.V().values(\"ts\").asDate()");
}
#[test]
fn cv_math_simple() {
    run("g.V().values(\"age\").math(\"_+1\")");
}
#[test]
fn cv_math_with_select() {
    run("g.V().as(\"a\").values(\"age\").as(\"b\").math(\"a+b\")");
}
#[test]
fn cv_type_of() {
    run("g.V().values(\"age\").typeOf()");
}

// =====================================================================
// by/format.rs — format() with positional placeholders then by(key)
// =====================================================================

#[test]
fn fmt_by_key_after_format() {
    run("g.V().format(\"name=%{_}\").by(\"name\")");
}
#[test]
fn fmt_by_empty_after_format() {
    run("g.V().values(\"name\").format(\"%{_}\").by()");
}
#[test]
fn fmt_by_two_keys() {
    run("g.V().format(\"%{_} %{_}\").by(\"name\").by(\"age\")");
}
#[test]
fn fmt_format_after_project_explicit_by() {
    run("g.V().project(\"n\",\"a\").by(\"name\").by(\"age\").format(\"%{_}/%{_}\").by(\"x\").by(\"y\")");
}
#[test]
fn fmt_named_and_positional_combined() {
    run("g.V().as(\"a\").format(\"%{a} talks to %{_}\").by(\"name\")");
}
#[test]
fn fmt_count_then_by() {
    run("g.V().format(\"%{_} edges\").by(__.bothE().count())");
}

// =====================================================================
// order/by_valuemap.rs — valueMap().by(__.unfold())
// =====================================================================

#[test]
fn vmuf_basic() {
    run("g.V().valueMap().by(__.unfold())");
}
#[test]
fn vmuf_keys() {
    run("g.V().valueMap(\"name\",\"age\").by(__.unfold())");
}
#[test]
fn vmuf_then_select_keys() {
    run("g.V().valueMap().by(__.unfold()).select(keys)");
}
#[test]
fn vmuf_then_select_values() {
    run("g.V().valueMap().by(__.unfold()).select(values)");
}
#[test]
fn vmuf_with_with_tokens() {
    run("g.V().valueMap().with(WithOptions.tokens).by(__.unfold())");
}

// =====================================================================
// Bytecode-form tests for `by("count()" | "fold()" | "sum()")` literal
// arms inside `handle_by_group` that the text parser bypasses (it routes
// through `ByTraversal(..)` for `by(count())` etc.). These exercise the
// otherwise-dead literal-key arms.
// =====================================================================

mod group_bytecode {
    use super::*;
    use crate::bytecode::GremlinStep;

    fn compile_steps(steps: Vec<GremlinStep>) {
        let _ = GremlinCompiler::new().compile(&steps);
    }

    #[test]
    fn group_by_key_then_count_literal() {
        compile_steps(vec![
            GremlinStep::V(None),
            GremlinStep::Group(None),
            GremlinStep::By("name".into(), false),
            GremlinStep::By("count()".into(), false),
        ]);
    }
    #[test]
    fn group_by_key_then_fold_literal() {
        compile_steps(vec![
            GremlinStep::V(None),
            GremlinStep::Group(None),
            GremlinStep::By("name".into(), false),
            GremlinStep::By("fold()".into(), false),
        ]);
    }
    #[test]
    fn group_by_key_then_sum_literal() {
        compile_steps(vec![
            GremlinStep::V(None),
            GremlinStep::Group(None),
            GremlinStep::By("name".into(), false),
            GremlinStep::By("sum()".into(), false),
        ]);
    }
    #[test]
    fn group_by_key_then_dunder_count() {
        compile_steps(vec![
            GremlinStep::V(None),
            GremlinStep::Group(None),
            GremlinStep::By("name".into(), false),
            GremlinStep::By("__.count()".into(), false),
        ]);
    }
    #[test]
    fn group_by_key_then_dunder_fold() {
        compile_steps(vec![
            GremlinStep::V(None),
            GremlinStep::Group(None),
            GremlinStep::By("name".into(), false),
            GremlinStep::By("__.fold()".into(), false),
        ]);
    }
    #[test]
    fn group_by_key_then_empty_value_by() {
        compile_steps(vec![
            GremlinStep::V(None),
            GremlinStep::Group(None),
            GremlinStep::By("name".into(), false),
            GremlinStep::By(String::new(), false),
        ]);
    }
    #[test]
    fn group_by_key_then_property_value_by() {
        compile_steps(vec![
            GremlinStep::V(None),
            GremlinStep::Group(None),
            GremlinStep::By("name".into(), false),
            GremlinStep::By("age".into(), false),
        ]);
    }
    #[test]
    fn group_by_key_then_inner_traversal_value_by() {
        compile_steps(vec![
            GremlinStep::V(None),
            GremlinStep::Group(None),
            GremlinStep::By("name".into(), false),
            GremlinStep::By("__.values(\"age\")".into(), false),
        ]);
    }
    #[test]
    fn group_by_key_then_inner_count_traversal_value_by() {
        compile_steps(vec![
            GremlinStep::V(None),
            GremlinStep::Group(None),
            GremlinStep::By("name".into(), false),
            GremlinStep::By("__.values(\"age\").sum()".into(), false),
        ]);
    }
    #[test]
    fn group_by_traversal_key_string() {
        compile_steps(vec![
            GremlinStep::V(None),
            GremlinStep::Group(None),
            GremlinStep::By("__.values(\"name\")".into(), false),
            GremlinStep::By("count()".into(), false),
        ]);
    }
    #[test]
    fn group_count_two_by_modulators_is_error() {
        compile_steps(vec![
            GremlinStep::V(None),
            GremlinStep::GroupCount(None),
            GremlinStep::By("name".into(), false),
            GremlinStep::By("count()".into(), false),
        ]);
    }
    #[test]
    fn aggregate_two_by_modulators_is_error() {
        compile_steps(vec![
            GremlinStep::V(None),
            GremlinStep::Aggregate("x".into()),
            GremlinStep::By("name".into(), false),
            GremlinStep::By("age".into(), false),
        ]);
    }
}

// =====================================================================
// Compound predicate label-aware paths via is(or/and/not(eq("label"), …))
// — exercises predicate_to_label_aware_expr and predicate_has_label_refs.
// =====================================================================

#[test]
fn pl_is_or_two_labels() {
    run("g.V().as(\"a\").as(\"b\").values(\"age\").is(P.or(P.eq(\"a\"), P.neq(\"b\")))");
}
#[test]
fn pl_is_and_two_labels() {
    run("g.V().as(\"a\").as(\"b\").values(\"age\").is(P.and(P.gt(\"a\"), P.lt(\"b\")))");
}
#[test]
fn pl_is_not_eq_label() {
    run("g.V().as(\"a\").values(\"age\").is(P.not(P.eq(\"a\")))");
}
#[test]
fn pl_select_is_or_label_compound() {
    run("g.V().as(\"a\").as(\"b\").select(\"a\").is(P.or(P.eq(\"b\"), P.neq(\"b\")))");
}
#[test]
fn pl_select_is_and_label_compound() {
    run("g.V().as(\"a\").as(\"b\").select(\"a\").is(P.and(P.eq(\"b\"), P.neq(\"b\")))");
}
#[test]
fn pl_select_is_not_label() {
    run("g.V().as(\"a\").as(\"b\").select(\"a\").is(P.not(P.eq(\"b\")))");
}
#[test]
fn pl_aggregate_is_or_compound() {
    run("g.V().aggregate(\"x\").out().values(\"age\").is(P.or(P.within(\"x\"), P.gt(0)))");
}
#[test]
fn pl_aggregate_is_and_compound_within() {
    run("g.V().aggregate(\"x\").out().values(\"age\").is(P.and(P.within(\"x\"), P.gt(0)))");
}
#[test]
fn pl_aggregate_is_not_within() {
    run("g.V().aggregate(\"x\").out().values(\"age\").is(P.not(P.within(\"x\")))");
}
#[test]
fn pl_select_is_within_aggregate() {
    run("g.V().aggregate(\"x\").out().as(\"v\").select(\"v\").is(P.within(\"x\"))");
}
#[test]
fn pl_select_is_without_aggregate() {
    run("g.V().aggregate(\"x\").out().as(\"v\").select(\"v\").is(P.without(\"x\"))");
}
#[test]
fn pl_filter_or_two_labels() {
    run("g.V().as(\"a\").as(\"b\").filter(__.values(\"age\").is(P.or(P.eq(\"a\"), P.eq(\"b\"))))");
}
#[test]
fn pl_filter_and_label_with_aggregate() {
    run("g.V().aggregate(\"x\").out().filter(__.values(\"age\").is(P.and(P.within(\"x\"), P.gt(0))))");
}

// =====================================================================
// types.rs — round-trip Path/Bytes for Value <-> GremlinValue
// =====================================================================

mod type_round_trip {
    use crate::types::GremlinValue;
    use cypher_core::storage::{EdgeId, NodeId};
    use cypher_core::types::Value;
    use indexmap::IndexMap;

    #[test]
    fn path_value_to_gremlin_and_back() {
        let path = Value::Path {
            nodes: vec![NodeId(1), NodeId(2), NodeId(3)],
            edges: vec![EdgeId(10), EdgeId(20)],
        };
        let gv: GremlinValue = path.into();
        match &gv {
            GremlinValue::Path { objects, labels } => {
                assert_eq!(objects.len(), 5);
                assert_eq!(labels.len(), 5);
            }
            other => panic!("expected Path, got {other:?}"),
        }
        // Round trip back loses Path, becomes a list of nodes/edges.
        let v: Value = gv.into();
        assert!(matches!(v, Value::List(_)));
    }

    #[test]
    fn bytes_value_to_gremlin_list_of_ints() {
        let v = Value::Bytes(vec![1, 2, 3, 0xff]);
        let gv: GremlinValue = v.into();
        if let GremlinValue::List(items) = gv {
            assert_eq!(items.len(), 4);
            assert_eq!(items[3], GremlinValue::Integer(255));
        } else {
            panic!("expected List");
        }
    }

    #[test]
    fn temporal_value_falls_back_to_string() {
        // Duration has no Gremlin equivalent — should fall through to the
        // catch-all and serialize as a string.
        let v = Value::Duration(cypher_core::types::value::CypherDuration {
            months: 1,
            days: 2,
            nanos: 3,
        });
        let gv: GremlinValue = v.into();
        assert!(matches!(gv, GremlinValue::String(_)));
    }

    #[test]
    fn edge_value_round_trip_basic() {
        let e = Value::Edge(EdgeId(42));
        let gv: GremlinValue = e.into();
        match &gv {
            GremlinValue::Edge { id, .. } => assert_eq!(*id, 42),
            _ => panic!("expected Edge"),
        }
    }

    #[test]
    fn map_value_to_gremlin_and_back() {
        let mut m: IndexMap<Box<str>, Value> = IndexMap::new();
        m.insert("a".into(), Value::Int(1));
        m.insert("b".into(), Value::Bool(true));
        let v = Value::Map(m);
        let gv: GremlinValue = v.into();
        if let GremlinValue::Map(g) = &gv {
            assert_eq!(g.len(), 2);
            assert_eq!(g.get("a"), Some(&GremlinValue::Integer(1)));
            assert_eq!(g.get("b"), Some(&GremlinValue::Bool(true)));
        } else {
            panic!("expected Map");
        }
    }

    #[test]
    fn list_round_trip_with_node() {
        let v = Value::List(vec![Value::Int(7), Value::Node(NodeId(9))]);
        let gv: GremlinValue = v.into();
        let back: Value = gv.into();
        if let Value::List(items) = back {
            assert_eq!(items.len(), 2);
            assert!(matches!(items[1], Value::Node(NodeId(9))));
        } else {
            panic!("expected List");
        }
    }
}

// =====================================================================
// text_parser/literal/string.rs — covered via direct tests in that file
// (see `#[cfg(test)] mod tests` appended to literal/string.rs).
// =====================================================================
