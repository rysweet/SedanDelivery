//! Focused unit tests for per-category step parsing.
//!
//! These tests exercise the dispatchers in each `steps/<category>.rs` module
//! and target previously-uncovered branches: predicate forms of `has`,
//! property chains, `by()` modulator shapes, scope-local variants, branch
//! modifiers, error paths, and edge cases.

use super::super::*;
use crate::bytecode::Pop;
use crate::types::GremlinValue;

fn no_params(_name: &str) -> Option<GremlinValue> {
    None
}

fn parse(s: &str) -> Vec<GremlinStep> {
    super::super::parse_traversal(s, &no_params).expect("parse failed")
}

fn parse_err(s: &str) -> TextParseError {
    super::super::parse_traversal(s, &no_params).expect_err("expected parse error")
}

// ── source ──────────────────────────────────────────────────────────────────

#[test]
fn source_v_with_ids() {
    let steps = parse("g.V(1, 2, 3)");
    assert_eq!(steps.len(), 1);
    match &steps[0] {
        GremlinStep::V(Some(args)) => assert_eq!(args.len(), 3),
        other => panic!("expected V(Some), got {:?}", other),
    }
}

#[test]
fn source_e_no_args() {
    assert_eq!(parse("g.E()"), vec![GremlinStep::E(None)]);
}

#[test]
fn source_e_with_id() {
    let steps = parse("g.E(7)");
    matches!(steps[0], GremlinStep::E(Some(_)));
}

#[test]
fn source_addv_label_only() {
    assert_eq!(
        parse(r#"g.addV("person")"#),
        vec![GremlinStep::AddV("person".into())]
    );
}

#[test]
fn source_addv_no_args_empty_label() {
    assert_eq!(parse("g.addV()"), vec![GremlinStep::AddV(String::new())]);
}

#[test]
fn source_adde_label() {
    let steps = parse(r#"g.addE("knows")"#);
    assert_eq!(steps, vec![GremlinStep::AddE("knows".into())]);
}

#[test]
fn source_inject_values() {
    let steps = parse("g.inject(1, 2, 3)");
    match &steps[0] {
        GremlinStep::Inject(args) => assert_eq!(args.len(), 3),
        other => panic!("expected Inject, got {:?}", other),
    }
}

// ── traversal_steps ─────────────────────────────────────────────────────────

#[test]
fn traversal_out_with_label() {
    let steps = parse(r#"g.V().out("knows")"#);
    assert_eq!(steps[1], GremlinStep::Out(vec!["knows".into()]));
}

#[test]
fn traversal_in_no_label() {
    let steps = parse("g.V().in()");
    assert_eq!(steps[1], GremlinStep::In(vec![]));
}

#[test]
fn traversal_both_multi_label() {
    let steps = parse(r#"g.V().both("a","b","c")"#);
    assert_eq!(
        steps[1],
        GremlinStep::Both(vec!["a".into(), "b".into(), "c".into()])
    );
}

#[test]
fn traversal_oute_inv_bothv_otherv() {
    let steps = parse("g.V().outE().inV().bothV().otherV()");
    assert_eq!(steps[1], GremlinStep::OutE(vec![]));
    assert_eq!(steps[2], GremlinStep::InV);
    assert_eq!(steps[3], GremlinStep::BothV);
    assert_eq!(steps[4], GremlinStep::OtherV);
}

#[test]
fn traversal_toe_direction_out() {
    let steps = parse(r#"g.V().toE(Direction.OUT, "knows")"#);
    assert_eq!(steps[1], GremlinStep::OutE(vec!["knows".into()]));
}

#[test]
fn traversal_toe_direction_in() {
    let steps = parse(r#"g.V().toE(Direction.IN, "created")"#);
    assert_eq!(steps[1], GremlinStep::InE(vec!["created".into()]));
}

#[test]
fn traversal_toe_direction_both() {
    let steps = parse(r#"g.V().toE(Direction.BOTH)"#);
    assert_eq!(steps[1], GremlinStep::BothE(vec![]));
}

// ── filter / has predicate forms ────────────────────────────────────────────

#[test]
fn filter_has_existence() {
    let steps = parse(r#"g.V().has("name")"#);
    assert_eq!(steps[1], GremlinStep::Has("name".into(), None));
}

#[test]
fn filter_has_key_value() {
    let steps = parse(r#"g.V().has("age", 30)"#);
    assert_eq!(
        steps[1],
        GremlinStep::Has("age".into(), Some(GremlinValue::Integer(30)))
    );
}

#[test]
fn filter_has_label_key_value() {
    let steps = parse(r#"g.V().has("person", "name", "marko")"#);
    assert_eq!(steps[1], GremlinStep::HasLabel(vec!["person".into()]));
    assert_eq!(
        steps[2],
        GremlinStep::Has("name".into(), Some(GremlinValue::String("marko".into())))
    );
}

#[test]
fn filter_has_with_predicate() {
    let steps = parse(r#"g.V().has("age", P.gt(20))"#);
    matches!(steps[1], GremlinStep::HasPredicate(_, _));
}

#[test]
fn filter_has_label_via_t_label() {
    // has(T.label, "person") — uses the ~label sentinel string directly.
    let steps = parse(r#"g.V().has("~label", "person")"#);
    assert_eq!(steps[1], GremlinStep::HasLabel(vec!["person".into()]));
}

#[test]
fn filter_has_id_via_t_id() {
    let steps = parse(r#"g.V().has("~id", 1)"#);
    matches!(steps[1], GremlinStep::HasId(_));
}

#[test]
fn filter_has_label_multi() {
    let steps = parse(r#"g.V().hasLabel("person","software")"#);
    assert_eq!(
        steps[1],
        GremlinStep::HasLabel(vec!["person".into(), "software".into()])
    );
}

#[test]
fn filter_has_id_predicate() {
    let steps = parse("g.V().hasId(P.neq(1))");
    matches!(steps[1], GremlinStep::HasIdPredicate(_));
}

#[test]
fn filter_has_id_values() {
    let steps = parse("g.V().hasId(1, 2, 3)");
    matches!(steps[1], GremlinStep::HasId(_));
}

#[test]
fn filter_has_not() {
    let steps = parse(r#"g.V().hasNot("missing")"#);
    matches!(steps[1], GremlinStep::Not(_));
}

#[test]
fn filter_has_key() {
    let steps = parse(r#"g.V().properties().hasKey("name")"#);
    assert_eq!(steps[2], GremlinStep::HasKey(vec!["name".into()]));
}

#[test]
fn filter_has_value_predicate() {
    let steps = parse(r#"g.V().properties().hasValue(P.gt(30))"#);
    matches!(steps[2], GremlinStep::HasValuePredicate(_));
}

#[test]
fn filter_has_value_literal() {
    let steps = parse(r#"g.V().properties().hasValue("marko")"#);
    matches!(steps[2], GremlinStep::HasValue(_));
}

#[test]
fn filter_is_predicate() {
    let steps = parse("g.V().values('age').is(P.gt(20))");
    matches!(steps[2], GremlinStep::Is(_));
}

#[test]
fn filter_simple_path_and_cyclic_path() {
    let steps = parse("g.V().simplePath().cyclicPath()");
    assert_eq!(steps[1], GremlinStep::SimplePath);
    assert_eq!(steps[2], GremlinStep::CyclicPath);
}

#[test]
fn filter_dedup_global() {
    let steps = parse("g.V().dedup()");
    assert_eq!(steps[1], GremlinStep::Dedup(vec![]));
}

#[test]
fn filter_dedup_local() {
    let steps = parse("g.V().dedup(Scope.local)");
    matches!(steps[1], GremlinStep::DedupLocal(_));
}

#[test]
fn filter_not_inner_traversal() {
    let steps = parse(r#"g.V().not(__.has("age",30))"#);
    matches!(steps[1], GremlinStep::Not(_));
}

#[test]
fn filter_and_or() {
    let steps_and = parse(r#"g.V().and(__.has("a"), __.has("b"))"#);
    matches!(steps_and[1], GremlinStep::And(_));
    let steps_or = parse(r#"g.V().or(__.has("a"), __.has("b"))"#);
    matches!(steps_or[1], GremlinStep::Or(_));
}

#[test]
fn filter_coin_default() {
    let steps = parse("g.V().coin(0.25)");
    match steps[1] {
        GremlinStep::Coin(p) => assert!((p - 0.25).abs() < 1e-9),
        ref other => panic!("expected Coin, got {:?}", other),
    }
}

// ── projection ──────────────────────────────────────────────────────────────

#[test]
fn projection_values_keys() {
    let steps = parse(r#"g.V().values("name","age")"#);
    assert_eq!(
        steps[1],
        GremlinStep::Values(vec!["name".into(), "age".into()])
    );
}

#[test]
fn projection_value_map_no_tokens() {
    let steps = parse(r#"g.V().valueMap("name")"#);
    assert_eq!(steps[1], GremlinStep::ValueMap(vec!["name".into()], 0));
}

#[test]
fn projection_value_map_with_tokens() {
    let steps = parse(r#"g.V().valueMap(true, "name")"#);
    assert_eq!(steps[1], GremlinStep::ValueMap(vec!["name".into()], 1));
}

#[test]
fn projection_select_with_pop_first() {
    let steps = parse(r#"g.V().select(Pop.first, "a")"#);
    assert_eq!(
        steps[1],
        GremlinStep::Select(Some(Pop::First), vec!["a".into()])
    );
}

#[test]
fn projection_select_multi_keys() {
    let steps = parse(r#"g.V().select("a","b","c")"#);
    assert_eq!(
        steps[1],
        GremlinStep::Select(None, vec!["a".into(), "b".into(), "c".into()])
    );
}

#[test]
fn projection_as_emits_each_alias() {
    let steps = parse(r#"g.V().as("a","b")"#);
    assert_eq!(steps[1], GremlinStep::As("a".into()));
    assert_eq!(steps[2], GremlinStep::As("b".into()));
}

#[test]
fn projection_label_id_element_index() {
    let steps = parse("g.V().label().id().element().index()");
    assert_eq!(steps[1], GremlinStep::Label);
    assert_eq!(steps[2], GremlinStep::Id);
    assert_eq!(steps[3], GremlinStep::Element);
    assert_eq!(steps[4], GremlinStep::Index);
}

#[test]
fn projection_loops_named() {
    let steps = parse(r#"g.V().repeat(__.out()).times(2).loops("a")"#);
    matches!(steps.last().unwrap(), GremlinStep::Loops(Some(_)));
}

#[test]
fn projection_constant() {
    let steps = parse(r#"g.V().constant("X")"#);
    assert_eq!(
        steps[1],
        GremlinStep::Constant(GremlinValue::String("X".into()))
    );
}

// ── mutation ────────────────────────────────────────────────────────────────

#[test]
fn mutation_property_chain() {
    let steps = parse(r#"g.addV("p").property("a", 1).property("b", 2)"#);
    assert_eq!(steps[0], GremlinStep::AddV("p".into()));
    assert_eq!(
        steps[1],
        GremlinStep::Property("a".into(), GremlinValue::Integer(1))
    );
    assert_eq!(
        steps[2],
        GremlinStep::Property("b".into(), GremlinValue::Integer(2))
    );
}

#[test]
fn mutation_property_with_cardinality() {
    let steps = parse(r#"g.addV("p").property(Cardinality.list, "tag", "x")"#);
    assert_eq!(
        steps[1],
        GremlinStep::Property("tag".into(), GremlinValue::String("x".into()))
    );
}

#[test]
fn mutation_drop() {
    let steps = parse("g.V().drop()");
    assert_eq!(steps[1], GremlinStep::Drop);
}

#[test]
fn mutation_merge_v_default_empty_map() {
    let steps = parse("g.mergeV()");
    matches!(steps[0], GremlinStep::MergeV(_));
}

#[test]
fn mutation_merge_e_default_empty_map() {
    let steps = parse("g.mergeE()");
    matches!(steps[0], GremlinStep::MergeE(_));
}

// ── aggregation ─────────────────────────────────────────────────────────────

#[test]
fn aggregation_count_global_vs_local() {
    let g = parse("g.V().count()");
    assert_eq!(g[1], GremlinStep::Count);
    let l = parse("g.V().fold().count(local)");
    matches!(l[2], GremlinStep::StringOpLocal(_));
}

#[test]
fn aggregation_sum_min_max_mean_global() {
    let s = parse("g.V().values('age').sum()");
    assert_eq!(s[2], GremlinStep::Sum);
    let mn = parse("g.V().values('age').min()");
    assert_eq!(mn[2], GremlinStep::Min);
    let mx = parse("g.V().values('age').max()");
    assert_eq!(mx[2], GremlinStep::Max);
    let me = parse("g.V().values('age').mean()");
    assert_eq!(me[2], GremlinStep::Mean);
}

#[test]
fn aggregation_local_variants() {
    assert!(matches!(
        parse("g.V().fold().sum(local)").last().unwrap(),
        GremlinStep::SumLocal
    ));
    assert!(matches!(
        parse("g.V().fold().min(local)").last().unwrap(),
        GremlinStep::MinLocal
    ));
    assert!(matches!(
        parse("g.V().fold().max(local)").last().unwrap(),
        GremlinStep::MaxLocal
    ));
    assert!(matches!(
        parse("g.V().fold().mean(local)").last().unwrap(),
        GremlinStep::MeanLocal
    ));
}

#[test]
fn aggregation_group_and_groupcount_no_key() {
    assert_eq!(parse("g.V().group()")[1], GremlinStep::Group(None));
    assert_eq!(
        parse("g.V().groupCount()")[1],
        GremlinStep::GroupCount(None)
    );
}

#[test]
fn aggregation_fold_no_seed_vs_seed() {
    assert_eq!(parse("g.V().fold()")[1], GremlinStep::Fold);
    let s = parse("g.V().fold(0, sum)");
    matches!(s[1], GremlinStep::FoldSeed(_, _));
}

#[test]
fn aggregation_unfold() {
    assert_eq!(parse("g.V().fold().unfold()")[2], GremlinStep::Unfold);
}

#[test]
fn aggregation_aggregate_with_scope() {
    let steps = parse(r#"g.V().aggregate(local, "x")"#);
    assert_eq!(steps[1], GremlinStep::Aggregate("x".into()));
}

#[test]
fn aggregation_cap() {
    let steps = parse(r#"g.V().aggregate("x").cap("x")"#);
    assert_eq!(steps[2], GremlinStep::Cap(vec!["x".into()]));
}

#[test]
fn aggregation_store_aliased_to_aggregate() {
    let steps = parse(r#"g.V().store("x")"#);
    assert_eq!(steps[1], GremlinStep::Aggregate("x".into()));
}

// ── branch ──────────────────────────────────────────────────────────────────

#[test]
fn branch_repeat_until_emit() {
    let steps = parse("g.V().repeat(__.out()).until(__.has('x')).emit()");
    matches!(steps[1], GremlinStep::Repeat(_, _));
    matches!(steps[2], GremlinStep::Until(_));
    assert_eq!(steps[3], GremlinStep::Emit);
}

#[test]
fn branch_repeat_with_label() {
    let steps = parse(r#"g.V().repeat("a", __.out())"#);
    matches!(steps[1], GremlinStep::Repeat(Some(_), _));
}

#[test]
fn branch_emit_with_pred() {
    let steps = parse(r#"g.V().repeat(__.out()).emit(__.has("x"))"#);
    matches!(steps.last().unwrap(), GremlinStep::EmitPred(_));
}

#[test]
fn branch_times() {
    let steps = parse("g.V().times(3)");
    assert_eq!(steps[1], GremlinStep::Times(3));
}

#[test]
fn branch_union_coalesce_optional() {
    let u = parse("g.V().union(__.out(), __.in())");
    matches!(u[1], GremlinStep::Union(_));
    let c = parse("g.V().coalesce(__.out(), __.in())");
    matches!(c[1], GremlinStep::Coalesce(_));
    let o = parse("g.V().optional(__.out())");
    matches!(o[1], GremlinStep::Optional(_));
}

#[test]
fn branch_choose_three_args() {
    let steps = parse(r#"g.V().choose(__.has("a"), __.constant(1), __.constant(2))"#);
    match &steps[1] {
        GremlinStep::Choose(_, _, Some(_)) => {}
        other => panic!("expected three-arm Choose, got {:?}", other),
    }
}

#[test]
fn branch_local_aliased_to_map() {
    let steps = parse("g.V().local(__.out())");
    matches!(steps[1], GremlinStep::Map(_));
}

#[test]
fn branch_side_effect() {
    let steps = parse(r#"g.V().sideEffect(__.aggregate("x"))"#);
    matches!(steps[1], GremlinStep::SideEffect(_));
}

#[test]
fn branch_map_flatmap_match() {
    matches!(parse("g.V().map(__.label())")[1], GremlinStep::Map(_));
    matches!(parse("g.V().flatMap(__.out())")[1], GremlinStep::FlatMap(_));
    matches!(
        parse(r#"g.V().match(__.as("a"), __.as("b"))"#)[1],
        GremlinStep::Match(_)
    );
}

// ── limit / order / by ──────────────────────────────────────────────────────

#[test]
fn limit_n_global() {
    assert_eq!(parse("g.V().limit(5)")[1], GremlinStep::Limit(5));
}

#[test]
fn limit_n_local() {
    let steps = parse("g.V().fold().limit(local, 3)");
    matches!(steps[2], GremlinStep::LimitLocal(3));
}

#[test]
fn limit_range() {
    assert_eq!(parse("g.V().range(1, 4)")[1], GremlinStep::Range(1, 4));
}

#[test]
fn limit_skip() {
    assert_eq!(parse("g.V().skip(2)")[1], GremlinStep::Skip(2));
}

#[test]
fn limit_tail_default_one() {
    assert_eq!(parse("g.V().tail()")[1], GremlinStep::Tail(1));
}

#[test]
fn limit_tail_n() {
    assert_eq!(parse("g.V().tail(3)")[1], GremlinStep::Tail(3));
}

#[test]
fn limit_sample_local() {
    let steps = parse("g.V().fold().sample(local, 2)");
    matches!(steps[2], GremlinStep::SampleLocal(2));
}

#[test]
fn limit_order_global_and_local() {
    assert_eq!(parse("g.V().order()")[1], GremlinStep::Order);
    assert_eq!(
        parse("g.V().fold().order(local)")[2],
        GremlinStep::SortLocal
    );
}

#[test]
fn limit_by_t_label_desc() {
    let steps = parse("g.V().order().by(T.label, Order.desc)");
    assert_eq!(steps[2], GremlinStep::By("~label".into(), true));
}

#[test]
fn limit_by_bare_label() {
    let steps = parse("g.V().order().by(label)");
    assert_eq!(steps[2], GremlinStep::By("~label".into(), false));
}

#[test]
fn limit_by_string_key_asc() {
    let steps = parse(r#"g.V().order().by("name")"#);
    assert_eq!(steps[2], GremlinStep::By("name".into(), false));
}

#[test]
fn limit_by_inner_traversal() {
    let steps = parse(r#"g.V().order().by(__.values("age"))"#);
    matches!(steps[2], GremlinStep::ByTraversal(_, _));
}

#[test]
fn limit_by_no_args_identity() {
    let steps = parse("g.V().order().by()");
    assert_eq!(steps[2], GremlinStep::By(String::new(), false));
}

#[test]
fn limit_barrier_default() {
    assert_eq!(parse("g.V().barrier()")[1], GremlinStep::Barrier);
}

#[test]
fn limit_barrier_norm_sack() {
    let steps = parse("g.V().barrier(normSack)");
    assert_eq!(steps[1], GremlinStep::BarrierNormSack);
}

// ── path ────────────────────────────────────────────────────────────────────

#[test]
fn path_step() {
    assert_eq!(parse("g.V().path()")[1], GremlinStep::Path);
}

#[test]
fn path_from_to_with_alias() {
    let steps = parse(r#"g.V().as("a").addE("knows").to("a")"#);
    assert!(steps.iter().any(|s| matches!(s, GremlinStep::To(_))));
}

// ── strings ─────────────────────────────────────────────────────────────────

#[test]
fn strings_to_lower_upper_trim() {
    assert_eq!(
        parse("g.V().values('n').toLower()")[2],
        GremlinStep::ToLower
    );
    assert_eq!(
        parse("g.V().values('n').toUpper()")[2],
        GremlinStep::ToUpper
    );
    assert_eq!(parse("g.V().values('n').trim()")[2], GremlinStep::Trim);
    assert_eq!(parse("g.V().values('n').lTrim()")[2], GremlinStep::LTrim);
    assert_eq!(parse("g.V().values('n').rTrim()")[2], GremlinStep::RTrim);
}

#[test]
fn strings_length_reverse() {
    assert_eq!(parse("g.V().values('n').length()")[2], GremlinStep::Length);
    assert_eq!(
        parse("g.V().values('n').reverse()")[2],
        GremlinStep::Reverse
    );
}

#[test]
fn strings_replace() {
    let steps = parse(r#"g.V().values('n').replace("a", "b")"#);
    assert_eq!(steps[2], GremlinStep::Replace("a".into(), "b".into()));
}

#[test]
fn strings_replace_too_few_args_errs() {
    let err = parse_err(r#"g.V().values('n').replace("a")"#);
    matches!(err, TextParseError::InvalidArgument(_));
}

#[test]
fn strings_split() {
    let steps = parse(r#"g.V().values('n').split(",")"#);
    assert_eq!(steps[2], GremlinStep::Split(",".into()));
}

#[test]
fn strings_substring_two_args() {
    let steps = parse("g.V().values('n').substring(1, 3)");
    assert_eq!(steps[2], GremlinStep::Substring(1, Some(3)));
}

#[test]
fn strings_substring_one_arg() {
    let steps = parse("g.V().values('n').substring(2)");
    assert_eq!(steps[2], GremlinStep::Substring(2, None));
}

#[test]
fn strings_concat_literals() {
    let steps = parse(r#"g.V().values('n').concat("x", "y")"#);
    matches!(steps[2], GremlinStep::Concat(_));
}

#[test]
fn strings_format_pattern() {
    let steps = parse(r#"g.V().format("Hello %{name}")"#);
    assert_eq!(steps[1], GremlinStep::Format("Hello %{name}".into()));
}

#[test]
fn strings_as_string_as_number_as_bool() {
    assert_eq!(
        parse("g.V().values('n').asString()")[2],
        GremlinStep::AsString
    );
    assert_eq!(
        parse("g.V().values('n').asNumber()")[2],
        GremlinStep::AsNumber
    );
    assert_eq!(parse("g.V().values('n').asBool()")[2], GremlinStep::AsBool);
}

#[test]
fn strings_type_of() {
    assert_eq!(parse("g.V().values('n').typeOf()")[2], GremlinStep::TypeOf);
}

// ── collection ──────────────────────────────────────────────────────────────

#[test]
fn collection_combine_intersect_difference() {
    let steps = parse("g.V().fold().combine([1])");
    matches!(steps[2], GremlinStep::Combine(_));
    let steps = parse("g.V().fold().intersect([1])");
    matches!(steps[2], GremlinStep::Intersect(_));
    let steps = parse("g.V().fold().difference([1])");
    matches!(steps[2], GremlinStep::Difference(_));
}

#[test]
fn collection_all_any_with_predicate() {
    let steps = parse("g.V().fold().all(P.gt(0))");
    matches!(steps[2], GremlinStep::All(_));
    let steps = parse("g.V().fold().any(P.gt(0))");
    matches!(steps[2], GremlinStep::Any(_));
}

// ── misc ────────────────────────────────────────────────────────────────────

#[test]
fn misc_sack_no_args_returns_value() {
    let steps = parse("g.V().sack()");
    assert_eq!(steps[1], GremlinStep::Sack);
}

#[test]
fn misc_sack_with_operator() {
    let steps = parse("g.V().sack(Operator.sum)");
    assert_eq!(steps[1], GremlinStep::SackOp("sum".into()));
}

#[test]
fn misc_iterate_is_unknown_step() {
    let err = parse_err("g.V().iterate()");
    matches!(err, TextParseError::UnknownStep(_));
}

#[test]
fn misc_with_options_tokens_labels_sets_token_mode_on_value_map() {
    let steps = parse("g.V().valueMap().with(WithOptions.tokens, WithOptions.labels)");
    // The valueMap step should be mutated to token_mode=2 (labels).
    match &steps[1] {
        GremlinStep::ValueMap(_, mode) => assert_eq!(*mode, 2),
        other => panic!("expected ValueMap, got {:?}", other),
    }
}

#[test]
fn misc_io_with_filename() {
    let steps = parse(r#"g.io("graph.kryo")"#);
    assert_eq!(steps[0], GremlinStep::Io("graph.kryo".into()));
}

// ── error paths ─────────────────────────────────────────────────────────────

#[test]
fn error_unknown_step() {
    let err = parse_err("g.V().nopeNotAStep()");
    match err {
        TextParseError::UnknownStep(s) => assert_eq!(s, "nopeNotAStep"),
        other => panic!("expected UnknownStep, got {:?}", other),
    }
}
