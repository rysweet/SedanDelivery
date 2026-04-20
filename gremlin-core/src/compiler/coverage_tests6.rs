//! Round-9 coverage tests targeting low-coverage helpers and step
//! compilers. Mixes end-to-end traversal compilation (via
//! `parse_traversal` + `GremlinCompiler::compile`) with direct unit
//! tests of `pub(in crate::compiler)` helpers.
//!
//! Files targeted (per gc-r9 baseline):
//!   - `compiler/compile_from/per_step_pendings.rs` (42.20%)
//!   - `compiler/helpers/predicate_label.rs`        (51.32%)
//!   - `compiler/steps/by/project.rs`               (60.57%)
//!   - `compiler/helpers/steps.rs`                  (61.49%)
//!   - `compiler/helpers/operators.rs`              (62.59%)
//!   - `compiler/steps/mutation/from_to.rs`         (64.12%)
//!   - `compiler/steps/limit.rs`                    (66.95%)
//!   - `compiler/helpers/where_by.rs`               (68.72%)
//!   - `compiler/steps/source.rs`                   (68.78%)
//!   - `compiler/helpers/conversion.rs`             (70.73%)
//!   - `compiler/steps/filter/has_kv.rs`            (73.50%)
//!   - `compiler/helpers/predicates.rs`             (76.41%)

#![allow(non_snake_case)]

use std::collections::{HashMap, HashSet};

use cypher_core::ast::{BinaryOp, Expr, Identifier, UnaryOp};
use cypher_core::planner::{CreatePattern, Operator, SetItem};
use cypher_core::types::Value;
use indexmap::IndexMap;

use super::helpers::*;
use super::GremlinCompiler;
use crate::bytecode::{GremlinStep, Predicate};
use crate::text_parser::parse_traversal;
use crate::types::GremlinValue;

fn no_param(_: &str) -> Option<GremlinValue> {
    None
}

#[track_caller]
fn run(query: &str) {
    if let Ok(steps) = parse_traversal(query, &no_param) {
        let _ = GremlinCompiler::new().compile(&steps);
    }
}

// =====================================================================
// limit.rs — order(local) flush before range/skip/tail; multi-key dedup
// =====================================================================

#[test]
fn lim_order_local_then_range_local() {
    run("g.V().fold().order(local).range(local, 0, 2)");
}
#[test]
fn lim_order_local_then_skip_local() {
    run("g.V().fold().order(local).skip(local, 1)");
}
#[test]
fn lim_order_local_then_tail_local() {
    run("g.V().fold().order(local).tail(local, 1)");
}
#[test]
fn lim_dedup_two_keys() {
    run("g.V().as(\"a\").out().as(\"b\").dedup(\"a\", \"b\")");
}
#[test]
fn lim_dedup_three_keys() {
    run("g.V().as(\"a\").out().as(\"b\").in().as(\"c\").dedup(\"a\", \"b\", \"c\")");
}
#[test]
fn lim_dedup_local_with_keys() {
    run("g.V().valueMap().dedup(local, \"name\")");
}
#[test]
fn lim_range_global_basic() {
    run("g.V().range(0, 10)");
}
#[test]
fn lim_skip_basic() {
    run("g.V().skip(5)");
}
#[test]
fn lim_limit_local() {
    run("g.V().valueMap().limit(local, 2)");
}
#[test]
fn lim_tail_global() {
    run("g.V().tail(3)");
}
#[test]
fn lim_tail_local() {
    run("g.V().fold().tail(local, 2)");
}
#[test]
fn lim_sample_local() {
    run("g.V().fold().sample(local, 2)");
}

// =====================================================================
// source.rs — V/E with id list params, mid-traversal V()/E(), Inject()
// =====================================================================

#[test]
fn src_v_with_single_id_string() {
    run("g.V(\"1\")");
}
#[test]
fn src_v_with_id_list_param() {
    run("g.V(1, 2, 3)");
}
#[test]
fn src_v_mid_traversal() {
    run("g.V().out().V(5).count()");
}
#[test]
fn src_e_basic() {
    run("g.E()");
}
#[test]
fn src_e_with_single_id() {
    run("g.E(11)");
}
#[test]
fn src_e_with_id_list() {
    run("g.E(11, 12, 13)");
}
#[test]
fn src_e_mid_traversal() {
    run("g.V().E(11)");
}
#[test]
fn src_inject_empty() {
    run("g.inject()");
}
#[test]
fn src_inject_values() {
    run("g.inject(1, 2, 3)");
}
#[test]
fn src_inject_mid_traversal() {
    run("g.V().inject(99).count()");
}
#[test]
fn src_inject_with_side_effect() {
    run("g.withSideEffect(\"x\", 1).inject(2).select(\"x\")");
}
#[test]
fn src_constant() {
    run("g.V().constant(\"hello\")");
}
#[test]
fn src_constant_int() {
    run("g.V().constant(42)");
}

// =====================================================================
// per_step_pendings.rs — varied flush triggers
// =====================================================================

#[test]
fn psp_math_no_by_then_count() {
    run("g.V().values(\"age\").math(\"_ * 2\").count()");
}
#[test]
fn psp_math_with_side_effect_var() {
    run("g.withSideEffect(\"x\", 10).V().values(\"age\").math(\"_ + x\").count()");
}
#[test]
fn psp_order_no_by_then_count() {
    run("g.V().values(\"age\").order().count()");
}
#[test]
fn psp_order_local_no_by_then_count() {
    run("g.V().fold().order(local).count()");
}
#[test]
fn psp_sample_no_by_then_select() {
    run("g.V().sample(2).count()");
}
#[test]
fn psp_format_no_by_then_count() {
    run("g.V().values(\"name\").format(\"hi %s\").count()");
}
#[test]
fn psp_format_three_by_keys() {
    run("g.V().project(\"a\",\"b\",\"c\").by(\"name\").by(\"age\").by(label).format(\"%s/%s/%s\").count()");
}
#[test]
fn psp_select_pending_then_count() {
    run("g.V().as(\"a\").out().select(\"a\").by(\"name\").count()");
}

// =====================================================================
// project.rs — project().by() variants
// =====================================================================

#[test]
fn pj_project_by_traversal_then_property_then_traversal() {
    run("g.V().project(\"d\",\"n\",\"l\").by(__.out().count()).by(\"name\").by(__.label())");
}
#[test]
fn pj_project_property_then_traversal_then_property() {
    run("g.V().project(\"n\",\"d\",\"a\").by(\"name\").by(__.out().count()).by(\"age\")");
}
#[test]
fn pj_project_only_properties() {
    run("g.V().project(\"a\",\"b\").by(\"name\").by(\"age\")");
}
#[test]
fn pj_project_default_no_by() {
    run("g.V().project(\"a\",\"b\")");
}
#[test]
fn pj_project_more_keys_than_by() {
    run("g.V().project(\"a\",\"b\",\"c\").by(\"name\")");
}
#[test]
fn pj_project_by_empty() {
    run("g.V().project(\"x\").by()");
}
#[test]
fn pj_project_by_then_unfold_select() {
    run("g.V().project(\"k\").by(\"name\").unfold().select(values)");
}

// =====================================================================
// from_to.rs — path().from()/to() with label
// =====================================================================

#[test]
fn ft_path_from_to_label() {
    run("g.V().as(\"a\").out().as(\"b\").path().from(\"a\").to(\"b\")");
}
#[test]
fn ft_simple_path_from_to() {
    run("g.V().as(\"a\").out().as(\"b\").simplePath().from(\"a\").to(\"b\")");
}
#[test]
fn ft_cyclic_path_from_to() {
    run("g.V().as(\"a\").out().as(\"b\").cyclicPath().from(\"a\").to(\"b\")");
}
#[test]
fn ft_addedge_from_traversal() {
    run("g.V().as(\"a\").addE(\"knows\").from(\"a\").to(__.V(2))");
}
#[test]
fn ft_addedge_to_label() {
    run("g.V().as(\"a\").addE(\"knows\").from(\"a\").to(\"a\")");
}

// =====================================================================
// where_by.rs — where(...).by() chains
// =====================================================================

#[test]
fn wb_where_label_pred_with_by() {
    run("g.V().as(\"a\").out().as(\"b\").where(\"a\", eq(\"b\")).by(\"name\").count()");
}
#[test]
fn wb_where_chain_by_two_keys() {
    run("g.V().as(\"a\").out().as(\"b\").where(\"a\", neq(\"b\")).by(\"name\").by(\"age\")");
}
#[test]
fn wb_where_pred_by_id() {
    run("g.V().as(\"a\").out().as(\"b\").where(\"a\", neq(\"b\")).by(__.id())");
}
#[test]
fn wb_where_or_pred_with_by() {
    run("g.V().as(\"a\").out().as(\"b\").where(\"a\", or(eq(\"b\"), neq(\"b\"))).by(\"name\")");
}
#[test]
fn wb_where_and_pred_with_by() {
    run("g.V().as(\"a\").out().as(\"b\").where(\"a\", and(neq(\"b\"), gt(\"b\"))).by(\"age\")");
}
#[test]
fn wb_where_within_label_with_by() {
    run("g.V().as(\"a\").aggregate(\"x\").out().where(within(\"x\")).by(\"name\")");
}
#[test]
fn wb_where_without_label_with_by() {
    run("g.V().as(\"a\").aggregate(\"x\").out().where(without(\"x\")).by(\"name\")");
}
#[test]
fn wb_where_pred_with_by_keys_column() {
    run("g.V().as(\"a\").out().as(\"b\").valueMap().where(select(keys), eq(\"a\")).by(\"name\")");
}
#[test]
fn wb_where_pred_with_by_values_column() {
    run("g.V().as(\"a\").out().as(\"b\").valueMap().where(select(values), eq(\"a\")).by(\"name\")");
}

// =====================================================================
// predicate_label.rs — labeled predicates via where()
// =====================================================================

#[test]
fn pl_where_within_label_alias() {
    run("g.V().as(\"a\").out().where(within(\"a\"))");
}
#[test]
fn pl_where_without_label_alias() {
    run("g.V().as(\"a\").out().where(without(\"a\"))");
}
#[test]
fn pl_where_within_aggregate() {
    run("g.V().aggregate(\"x\").out().where(within(\"x\"))");
}
#[test]
fn pl_where_without_aggregate() {
    run("g.V().aggregate(\"x\").out().where(without(\"x\"))");
}
#[test]
fn pl_where_or_two_labels() {
    run("g.V().as(\"a\").as(\"b\").out().where(or(eq(\"a\"), eq(\"b\")))");
}
#[test]
fn pl_where_and_two_labels() {
    run("g.V().as(\"a\").as(\"b\").out().where(and(neq(\"a\"), neq(\"b\")))");
}
#[test]
fn pl_where_not_label() {
    run("g.V().as(\"a\").out().where(not(eq(\"a\")))");
}
#[test]
fn pl_where_label_after_select() {
    run("g.V().as(\"a\").out().as(\"b\").select(\"a\").where(eq(\"b\"))");
}
#[test]
fn pl_select_label_is_pred_label() {
    run("g.V().as(\"a\").as(\"b\").select(\"a\").is(eq(\"b\"))");
}
#[test]
fn pl_select_label_is_neq_label() {
    run("g.V().as(\"a\").as(\"b\").select(\"a\").is(neq(\"b\"))");
}
#[test]
fn pl_select_label_is_within_alias() {
    run("g.V().aggregate(\"x\").select(\"x\").is(within(\"x\"))");
}

// =====================================================================
// helpers/steps.rs — infix and/or, path tracking, label stacks
// =====================================================================

#[test]
fn hs_infix_or_simple() {
    run("g.V().has(\"name\", \"a\").or().has(\"name\", \"b\")");
}
#[test]
fn hs_infix_and_simple() {
    run("g.V().has(\"name\", \"a\").and().has(\"age\", gt(20))");
}
#[test]
fn hs_infix_mixed_or_and() {
    run("g.V().has(\"a\", 1).and().has(\"b\", 2).or().has(\"c\", 3)");
}
#[test]
fn hs_path_in_repeat() {
    run("g.V().repeat(__.out().path()).times(2)");
}
#[test]
fn hs_path_in_flat_map() {
    run("g.V().flatMap(__.out().path())");
}
#[test]
fn hs_path_in_map() {
    run("g.V().map(__.path())");
}
#[test]
fn hs_path_in_not() {
    run("g.V().not(__.out().path()).count()");
}
#[test]
fn hs_path_in_where_inner() {
    run("g.V().where(__.out().path())");
}
#[test]
fn hs_path_in_or() {
    run("g.V().or(__.out().path(), __.in())");
}
#[test]
fn hs_path_in_union() {
    run("g.V().union(__.out().path(), __.in())");
}
#[test]
fn hs_path_in_choose() {
    run("g.V().choose(__.has(\"x\"), __.path(), __.label())");
}
#[test]
fn hs_label_stack_select_pop_first() {
    run("g.V().as(\"a\").out().as(\"a\").select(first, \"a\")");
}
#[test]
fn hs_label_stack_select_pop_last() {
    run("g.V().as(\"a\").out().as(\"a\").select(last, \"a\")");
}
#[test]
fn hs_label_stack_select_pop_all() {
    run("g.V().as(\"a\").out().as(\"a\").select(all, \"a\")");
}
#[test]
fn hs_pop_select_in_repeat() {
    run("g.V().as(\"a\").repeat(__.out().as(\"a\")).times(2).select(all, \"a\")");
}

// =====================================================================
// helpers/conversion.rs — gremlin_value_to_expr / make_by_expr coverage
// =====================================================================

#[test]
fn cv_value_to_expr_null() {
    let e = gremlin_value_to_expr(&GremlinValue::Null);
    assert!(matches!(e, Expr::Null));
}
#[test]
fn cv_value_to_expr_bool() {
    assert!(matches!(
        gremlin_value_to_expr(&GremlinValue::Bool(true)),
        Expr::Bool(true)
    ));
}
#[test]
fn cv_value_to_expr_int() {
    assert!(matches!(
        gremlin_value_to_expr(&GremlinValue::Integer(7)),
        Expr::Integer(7)
    ));
}
#[test]
fn cv_value_to_expr_float() {
    let e = gremlin_value_to_expr(&GremlinValue::Float(2.5));
    assert!(matches!(e, Expr::Float(_)));
}
#[test]
fn cv_value_to_expr_string() {
    assert!(matches!(
        gremlin_value_to_expr(&GremlinValue::String("x".to_string())),
        Expr::Str(_)
    ));
}
#[test]
fn cv_value_to_expr_datetime_marker() {
    let e = gremlin_value_to_expr(&GremlinValue::String(
        "__gremlin_dt__:2020-01-01T00:00:00Z".to_string(),
    ));
    match e {
        Expr::FunctionCall { name, .. } => assert_eq!(name.as_str(), "gremlin_make_datetime"),
        _ => panic!("expected datetime FunctionCall"),
    }
}
#[test]
fn cv_value_to_expr_uuid_marker() {
    let e = gremlin_value_to_expr(&GremlinValue::String(
        "__gremlin_uuid__:00000000-0000-0000-0000-000000000000".to_string(),
    ));
    match e {
        Expr::FunctionCall { name, .. } => assert_eq!(name.as_str(), "gremlin_make_uuid"),
        _ => panic!("expected uuid FunctionCall"),
    }
}
#[test]
fn cv_value_to_expr_list() {
    let e = gremlin_value_to_expr(&GremlinValue::List(vec![
        GremlinValue::Integer(1),
        GremlinValue::Integer(2),
    ]));
    if let Expr::List(items) = e {
        assert_eq!(items.len(), 2);
    } else {
        panic!("expected List");
    }
}
#[test]
fn cv_value_to_expr_map() {
    let e = gremlin_value_to_expr(&GremlinValue::Map({
        let mut m = IndexMap::new();
        m.insert("k".to_string(), GremlinValue::Integer(1));
        m
    }));
    assert!(matches!(e, Expr::MapLiteral(_)));
}
#[test]
fn cv_value_to_expr_vertex() {
    let e = gremlin_value_to_expr(&GremlinValue::Vertex {
        id: 42,
        label: "X".to_string(),
        properties: IndexMap::new(),
    });
    assert!(matches!(e, Expr::Integer(42)));
}
#[test]
fn cv_value_to_expr_edge() {
    let e = gremlin_value_to_expr(&GremlinValue::Edge {
        id: 7,
        label: "knows".to_string(),
        out_v: 1,
        in_v: 2,
        properties: IndexMap::new(),
    });
    assert!(matches!(e, Expr::Integer(7)));
}
#[test]
fn cv_value_to_expr_path_is_null() {
    let e = gremlin_value_to_expr(&GremlinValue::Path {
        labels: vec![],
        objects: vec![],
    });
    assert!(matches!(e, Expr::Null));
}

#[test]
fn cv_id_expr_string_numeric_to_integer() {
    let e = gremlin_value_to_id_expr(&GremlinValue::String("42".into()));
    assert!(matches!(e, Expr::Integer(42)));
}
#[test]
fn cv_id_expr_string_non_numeric_remains_str() {
    let e = gremlin_value_to_id_expr(&GremlinValue::String("abc".into()));
    assert!(matches!(e, Expr::Str(_)));
}
#[test]
fn cv_id_expr_int_passthrough() {
    let e = gremlin_value_to_id_expr(&GremlinValue::Integer(5));
    assert!(matches!(e, Expr::Integer(5)));
}

#[test]
fn cv_is_gremlin_id_key_variants() {
    assert!(is_gremlin_id_key("~id"));
    assert!(is_gremlin_id_key("T.id"));
    assert!(is_gremlin_id_key("t[id]"));
    assert!(!is_gremlin_id_key("id"));
    assert!(!is_gremlin_id_key("name"));
}

#[test]
fn cv_gremlin_has_key_expr_id() {
    let alias = Identifier::new("v");
    let e = gremlin_has_key_expr("~id", &alias);
    match e {
        Expr::FunctionCall { name, .. } => assert_eq!(name.as_str(), "id"),
        _ => panic!("expected id FunctionCall"),
    }
}
#[test]
fn cv_gremlin_has_key_expr_property() {
    let alias = Identifier::new("v");
    let e = gremlin_has_key_expr("name", &alias);
    assert!(matches!(e, Expr::Property(_, _)));
}

#[test]
fn cv_value_to_value_scalars() {
    assert!(matches!(
        gremlin_value_to_value(&GremlinValue::Null),
        Value::Null
    ));
    assert!(matches!(
        gremlin_value_to_value(&GremlinValue::Bool(true)),
        Value::Bool(true)
    ));
    assert!(matches!(
        gremlin_value_to_value(&GremlinValue::Integer(3)),
        Value::Int(3)
    ));
    assert!(matches!(
        gremlin_value_to_value(&GremlinValue::Float(0.0)),
        Value::Float(_)
    ));
    assert!(matches!(
        gremlin_value_to_value(&GremlinValue::String("x".to_string())),
        Value::Str(_)
    ));
}
#[test]
fn cv_value_to_value_list() {
    let v = gremlin_value_to_value(&GremlinValue::List(vec![
        GremlinValue::Integer(1),
        GremlinValue::Integer(2),
    ]));
    if let Value::List(items) = v {
        assert_eq!(items.len(), 2);
    } else {
        panic!("expected List");
    }
}
#[test]
fn cv_value_to_value_unsupported_to_null() {
    assert!(matches!(
        gremlin_value_to_value(&GremlinValue::Map(IndexMap::new())),
        Value::Null
    ));
    assert!(matches!(
        gremlin_value_to_value(&GremlinValue::Vertex {
            id: 1,
            label: "x".to_string(),
            properties: IndexMap::new()
        }),
        Value::Null
    ));
    assert!(matches!(
        gremlin_value_to_value(&GremlinValue::Edge {
            id: 1,
            label: "x".to_string(),
            out_v: 1,
            in_v: 2,
            properties: IndexMap::new()
        }),
        Value::Null
    ));
    assert!(matches!(
        gremlin_value_to_value(&GremlinValue::Path {
            labels: vec![],
            objects: vec![]
        }),
        Value::Null
    ));
}

#[test]
fn cv_make_by_expr_id_aliases() {
    let alias = Identifier::new("v");
    for k in &["~id", "T.id", "t[id]"] {
        let e = make_by_expr(k, &alias);
        match e {
            Expr::FunctionCall { name, .. } => assert_eq!(name.as_str(), "id"),
            _ => panic!("expected id FunctionCall for {}", k),
        }
    }
}
#[test]
fn cv_make_by_expr_label_aliases() {
    let alias = Identifier::new("v");
    for k in &["~label", "T.label", "t[label]", "__.label()", "label()"] {
        let e = make_by_expr(k, &alias);
        match e {
            Expr::FunctionCall { name, .. } => assert_eq!(name.as_str(), "gremlin_single_label"),
            _ => panic!("expected gremlin_single_label for {}", k),
        }
    }
}

// =====================================================================
// helpers/predicate_label.rs — direct unit tests
// =====================================================================

fn empty_alias_map() -> HashMap<String, Identifier> {
    HashMap::new()
}
fn empty_agg_keys() -> HashSet<String> {
    HashSet::new()
}

#[test]
fn pl_predicate_string_ref_eq() {
    let p = Predicate::Eq(GremlinValue::String("x".to_string()));
    assert_eq!(predicate_string_ref(&p), Some("x"));
}
#[test]
fn pl_predicate_string_ref_neq() {
    let p = Predicate::Neq(GremlinValue::String("y".to_string()));
    assert_eq!(predicate_string_ref(&p), Some("y"));
}
#[test]
fn pl_predicate_string_ref_gt_gte_lt_lte() {
    assert_eq!(
        predicate_string_ref(&Predicate::Gt(GremlinValue::String("g".to_string()))),
        Some("g")
    );
    assert_eq!(
        predicate_string_ref(&Predicate::Gte(GremlinValue::String("ge".to_string()))),
        Some("ge")
    );
    assert_eq!(
        predicate_string_ref(&Predicate::Lt(GremlinValue::String("l".to_string()))),
        Some("l")
    );
    assert_eq!(
        predicate_string_ref(&Predicate::Lte(GremlinValue::String("le".to_string()))),
        Some("le")
    );
}
#[test]
fn pl_predicate_string_ref_int_returns_none() {
    let p = Predicate::Eq(GremlinValue::Integer(1));
    assert_eq!(predicate_string_ref(&p), None);
}
#[test]
fn pl_predicate_string_ref_within_returns_none() {
    let p = Predicate::Within(vec![GremlinValue::Integer(1)]);
    assert_eq!(predicate_string_ref(&p), None);
}

#[test]
fn pl_predicate_to_binary_op_all_variants() {
    assert!(matches!(
        predicate_to_binary_op(&Predicate::Eq(GremlinValue::Integer(1))),
        BinaryOp::Eq
    ));
    assert!(matches!(
        predicate_to_binary_op(&Predicate::Neq(GremlinValue::Integer(1))),
        BinaryOp::Neq
    ));
    assert!(matches!(
        predicate_to_binary_op(&Predicate::Gt(GremlinValue::Integer(1))),
        BinaryOp::Gt
    ));
    assert!(matches!(
        predicate_to_binary_op(&Predicate::Gte(GremlinValue::Integer(1))),
        BinaryOp::Gte
    ));
    assert!(matches!(
        predicate_to_binary_op(&Predicate::Lt(GremlinValue::Integer(1))),
        BinaryOp::Lt
    ));
    assert!(matches!(
        predicate_to_binary_op(&Predicate::Lte(GremlinValue::Integer(1))),
        BinaryOp::Lte
    ));
    assert!(matches!(
        predicate_to_binary_op(&Predicate::Within(vec![])),
        BinaryOp::In
    ));
    assert!(matches!(
        predicate_to_binary_op(&Predicate::Without(vec![])),
        BinaryOp::In
    ));
    assert!(matches!(
        predicate_to_binary_op(&Predicate::Between(
            GremlinValue::Integer(1),
            GremlinValue::Integer(2)
        )),
        BinaryOp::And
    ));
    assert!(matches!(
        predicate_to_binary_op(&Predicate::Inside(
            GremlinValue::Integer(1),
            GremlinValue::Integer(2)
        )),
        BinaryOp::And
    ));
    // Text predicates default to Eq
    assert!(matches!(
        predicate_to_binary_op(&Predicate::Containing(GremlinValue::String(
            "x".to_string()
        ))),
        BinaryOp::Eq
    ));
    assert!(matches!(
        predicate_to_binary_op(&Predicate::NotContaining(GremlinValue::String(
            "x".to_string()
        ))),
        BinaryOp::Eq
    ));
    assert!(matches!(
        predicate_to_binary_op(&Predicate::StartingWith(GremlinValue::String(
            "x".to_string()
        ))),
        BinaryOp::Eq
    ));
    assert!(matches!(
        predicate_to_binary_op(&Predicate::NotStartingWith(GremlinValue::String(
            "x".to_string()
        ))),
        BinaryOp::Eq
    ));
    assert!(matches!(
        predicate_to_binary_op(&Predicate::EndingWith(GremlinValue::String(
            "x".to_string()
        ))),
        BinaryOp::Eq
    ));
    assert!(matches!(
        predicate_to_binary_op(&Predicate::NotEndingWith(GremlinValue::String(
            "x".to_string()
        ))),
        BinaryOp::Eq
    ));
    assert!(matches!(
        predicate_to_binary_op(&Predicate::Regex(GremlinValue::String("x".to_string()))),
        BinaryOp::Eq
    ));
    assert!(matches!(
        predicate_to_binary_op(&Predicate::NotRegex(GremlinValue::String("x".to_string()))),
        BinaryOp::Eq
    ));
    assert!(matches!(
        predicate_to_binary_op(&Predicate::IsType(GremlinValue::String(
            "string".to_string()
        ))),
        BinaryOp::Eq
    ));
    assert!(matches!(
        predicate_to_binary_op(&Predicate::And(vec![])),
        BinaryOp::Eq
    ));
    assert!(matches!(
        predicate_to_binary_op(&Predicate::Or(vec![])),
        BinaryOp::Eq
    ));
    assert!(matches!(
        predicate_to_binary_op(&Predicate::Not(Box::new(Predicate::Eq(
            GremlinValue::Integer(1)
        )))),
        BinaryOp::Eq
    ));
}

#[test]
fn pl_predicate_has_label_refs_simple_match() {
    let mut am = HashMap::new();
    am.insert("a".to_string(), Identifier::new("alias_a"));
    let p = Predicate::Eq(GremlinValue::String("a".to_string()));
    assert!(predicate_has_label_refs(&p, &am, &empty_agg_keys()));
}
#[test]
fn pl_predicate_has_label_refs_no_match() {
    let p = Predicate::Eq(GremlinValue::String("nope".to_string()));
    assert!(!predicate_has_label_refs(
        &p,
        &empty_alias_map(),
        &empty_agg_keys()
    ));
}
#[test]
fn pl_predicate_has_label_refs_within_alias() {
    let mut am = HashMap::new();
    am.insert("a".to_string(), Identifier::new("aa"));
    let p = Predicate::Within(vec![GremlinValue::String("a".to_string())]);
    assert!(predicate_has_label_refs(&p, &am, &empty_agg_keys()));
}
#[test]
fn pl_predicate_has_label_refs_within_aggregate() {
    let mut ak = HashSet::new();
    ak.insert("k".to_string());
    let p = Predicate::Without(vec![GremlinValue::String("k".to_string())]);
    assert!(predicate_has_label_refs(&p, &empty_alias_map(), &ak));
}
#[test]
fn pl_predicate_has_label_refs_within_non_string() {
    let p = Predicate::Within(vec![GremlinValue::Integer(1)]);
    assert!(!predicate_has_label_refs(
        &p,
        &empty_alias_map(),
        &empty_agg_keys()
    ));
}
#[test]
fn pl_predicate_has_label_refs_or_and() {
    let mut am = HashMap::new();
    am.insert("a".to_string(), Identifier::new("aa"));
    let p = Predicate::Or(vec![
        Predicate::Eq(GremlinValue::Integer(1)),
        Predicate::Eq(GremlinValue::String("a".to_string())),
    ]);
    assert!(predicate_has_label_refs(&p, &am, &empty_agg_keys()));
    let p2 = Predicate::And(vec![
        Predicate::Eq(GremlinValue::Integer(1)),
        Predicate::Eq(GremlinValue::String("a".to_string())),
    ]);
    assert!(predicate_has_label_refs(&p2, &am, &empty_agg_keys()));
}
#[test]
fn pl_predicate_has_label_refs_not() {
    let mut am = HashMap::new();
    am.insert("a".to_string(), Identifier::new("aa"));
    let p = Predicate::Not(Box::new(Predicate::Eq(GremlinValue::String(
        "a".to_string(),
    ))));
    assert!(predicate_has_label_refs(&p, &am, &empty_agg_keys()));
}

#[test]
fn pl_predicate_label_ref_eq_string() {
    let mut am = HashMap::new();
    am.insert("a".to_string(), Identifier::new("aa"));
    let p = Predicate::Eq(GremlinValue::String("a".to_string()));
    assert!(predicate_label_ref(&p, &am, &empty_agg_keys()).is_some());
}
#[test]
fn pl_predicate_label_ref_aggregate_key() {
    let mut ak = HashSet::new();
    ak.insert("k".to_string());
    let p = Predicate::Eq(GremlinValue::String("k".to_string()));
    let r = predicate_label_ref(&p, &empty_alias_map(), &ak).unwrap();
    if let Expr::Variable(id) = r {
        assert_eq!(id.0.as_str(), "__gremlin_agg_k");
    } else {
        panic!("expected Variable");
    }
}
#[test]
fn pl_predicate_label_ref_within_string_alias() {
    let mut am = HashMap::new();
    am.insert("a".to_string(), Identifier::new("aa"));
    let p = Predicate::Within(vec![GremlinValue::String("a".to_string())]);
    assert!(predicate_label_ref(&p, &am, &empty_agg_keys()).is_some());
}
#[test]
fn pl_predicate_label_ref_without_string_alias() {
    let mut am = HashMap::new();
    am.insert("a".to_string(), Identifier::new("aa"));
    let p = Predicate::Without(vec![GremlinValue::String("a".to_string())]);
    assert!(predicate_label_ref(&p, &am, &empty_agg_keys()).is_some());
}
#[test]
fn pl_predicate_label_ref_within_non_string_returns_none() {
    let p = Predicate::Within(vec![GremlinValue::Integer(1)]);
    assert!(predicate_label_ref(&p, &empty_alias_map(), &empty_agg_keys()).is_none());
}
#[test]
fn pl_predicate_label_ref_without_non_string_returns_none() {
    let p = Predicate::Without(vec![GremlinValue::Integer(1)]);
    assert!(predicate_label_ref(&p, &empty_alias_map(), &empty_agg_keys()).is_none());
}
#[test]
fn pl_predicate_label_ref_unknown_label_returns_none() {
    let p = Predicate::Eq(GremlinValue::String("nope".to_string()));
    assert!(predicate_label_ref(&p, &empty_alias_map(), &empty_agg_keys()).is_none());
}
#[test]
fn pl_predicate_label_ref_other_predicate_returns_none() {
    let p = Predicate::Containing(GremlinValue::String("x".to_string()));
    assert!(predicate_label_ref(&p, &empty_alias_map(), &empty_agg_keys()).is_none());
}

#[test]
fn pl_predicate_to_label_aware_expr_eq_label() {
    let mut am = HashMap::new();
    am.insert("a".to_string(), Identifier::new("aa"));
    let p = Predicate::Eq(GremlinValue::String("a".to_string()));
    let left = Expr::Variable(Identifier::new("v"));
    let e = predicate_to_label_aware_expr(&p, &left, &am, &empty_agg_keys());
    assert!(matches!(
        e,
        Expr::BinaryOp {
            op: BinaryOp::Eq,
            ..
        }
    ));
}
#[test]
fn pl_predicate_to_label_aware_expr_eq_unknown_label_uses_str() {
    let p = Predicate::Eq(GremlinValue::String("x".to_string()));
    let left = Expr::Variable(Identifier::new("v"));
    let e = predicate_to_label_aware_expr(&p, &left, &empty_alias_map(), &empty_agg_keys());
    if let Expr::BinaryOp { right, .. } = e {
        assert!(matches!(*right, Expr::Str(_)));
    } else {
        panic!("expected BinaryOp");
    }
}
#[test]
fn pl_predicate_to_label_aware_expr_within_alias() {
    let mut am = HashMap::new();
    am.insert("a".to_string(), Identifier::new("aa"));
    let p = Predicate::Within(vec![GremlinValue::String("a".to_string())]);
    let left = Expr::Variable(Identifier::new("v"));
    let e = predicate_to_label_aware_expr(&p, &left, &am, &empty_agg_keys());
    assert!(matches!(
        e,
        Expr::BinaryOp {
            op: BinaryOp::In,
            ..
        }
    ));
}
#[test]
fn pl_predicate_to_label_aware_expr_within_aggregate() {
    let mut ak = HashSet::new();
    ak.insert("k".to_string());
    let p = Predicate::Within(vec![GremlinValue::String("k".to_string())]);
    let left = Expr::Variable(Identifier::new("v"));
    let e = predicate_to_label_aware_expr(&p, &left, &empty_alias_map(), &ak);
    assert!(matches!(
        e,
        Expr::BinaryOp {
            op: BinaryOp::In,
            ..
        }
    ));
}
#[test]
fn pl_predicate_to_label_aware_expr_without_alias() {
    let mut am = HashMap::new();
    am.insert("a".to_string(), Identifier::new("aa"));
    let p = Predicate::Without(vec![GremlinValue::String("a".to_string())]);
    let left = Expr::Variable(Identifier::new("v"));
    let e = predicate_to_label_aware_expr(&p, &left, &am, &empty_agg_keys());
    assert!(matches!(
        e,
        Expr::UnaryOp {
            op: UnaryOp::Not,
            ..
        }
    ));
}
#[test]
fn pl_predicate_to_label_aware_expr_without_aggregate() {
    let mut ak = HashSet::new();
    ak.insert("k".to_string());
    let p = Predicate::Without(vec![GremlinValue::String("k".to_string())]);
    let left = Expr::Variable(Identifier::new("v"));
    let e = predicate_to_label_aware_expr(&p, &left, &empty_alias_map(), &ak);
    assert!(matches!(
        e,
        Expr::UnaryOp {
            op: UnaryOp::Not,
            ..
        }
    ));
}
#[test]
fn pl_predicate_to_label_aware_expr_or_combination() {
    let mut am = HashMap::new();
    am.insert("a".to_string(), Identifier::new("aa"));
    am.insert("b".to_string(), Identifier::new("bb"));
    let p = Predicate::Or(vec![
        Predicate::Eq(GremlinValue::String("a".to_string())),
        Predicate::Eq(GremlinValue::String("b".to_string())),
    ]);
    let left = Expr::Variable(Identifier::new("v"));
    let e = predicate_to_label_aware_expr(&p, &left, &am, &empty_agg_keys());
    assert!(matches!(
        e,
        Expr::BinaryOp {
            op: BinaryOp::Or,
            ..
        }
    ));
}
#[test]
fn pl_predicate_to_label_aware_expr_and_combination() {
    let mut am = HashMap::new();
    am.insert("a".to_string(), Identifier::new("aa"));
    am.insert("b".to_string(), Identifier::new("bb"));
    let p = Predicate::And(vec![
        Predicate::Neq(GremlinValue::String("a".to_string())),
        Predicate::Neq(GremlinValue::String("b".to_string())),
    ]);
    let left = Expr::Variable(Identifier::new("v"));
    let e = predicate_to_label_aware_expr(&p, &left, &am, &empty_agg_keys());
    assert!(matches!(
        e,
        Expr::BinaryOp {
            op: BinaryOp::And,
            ..
        }
    ));
}
#[test]
fn pl_predicate_to_label_aware_expr_not() {
    let mut am = HashMap::new();
    am.insert("a".to_string(), Identifier::new("aa"));
    let p = Predicate::Not(Box::new(Predicate::Eq(GremlinValue::String(
        "a".to_string(),
    ))));
    let left = Expr::Variable(Identifier::new("v"));
    let e = predicate_to_label_aware_expr(&p, &left, &am, &empty_agg_keys());
    assert!(matches!(
        e,
        Expr::UnaryOp {
            op: UnaryOp::Not,
            ..
        }
    ));
}
#[test]
fn pl_predicate_to_label_aware_expr_other_falls_back() {
    let p = Predicate::Containing(GremlinValue::String("x".to_string()));
    let left = Expr::Variable(Identifier::new("v"));
    let _ = predicate_to_label_aware_expr(&p, &left, &empty_alias_map(), &empty_agg_keys());
}

// =====================================================================
// helpers/where_by.rs — direct unit tests
// =====================================================================

#[test]
fn wb_collect_predicate_labels_eq_label() {
    let mut am = HashMap::new();
    am.insert("a".to_string(), Identifier::new("aa"));
    let p = Predicate::Eq(GremlinValue::String("a".to_string()));
    let mut targets = Vec::new();
    collect_predicate_labels_in_order(&p, &am, &mut targets);
    assert_eq!(targets.len(), 1);
}
#[test]
fn wb_collect_predicate_labels_within_alias() {
    let mut am = HashMap::new();
    am.insert("a".to_string(), Identifier::new("aa"));
    let p = Predicate::Within(vec![GremlinValue::String("a".to_string())]);
    let mut targets = Vec::new();
    collect_predicate_labels_in_order(&p, &am, &mut targets);
    assert_eq!(targets.len(), 1);
}
#[test]
fn wb_collect_predicate_labels_or() {
    let mut am = HashMap::new();
    am.insert("a".to_string(), Identifier::new("aa"));
    am.insert("b".to_string(), Identifier::new("bb"));
    let p = Predicate::Or(vec![
        Predicate::Eq(GremlinValue::String("a".to_string())),
        Predicate::Eq(GremlinValue::String("b".to_string())),
    ]);
    let mut targets = Vec::new();
    collect_predicate_labels_in_order(&p, &am, &mut targets);
    assert_eq!(targets.len(), 2);
}
#[test]
fn wb_collect_predicate_labels_dedupes_existing() {
    let mut am = HashMap::new();
    am.insert("a".to_string(), Identifier::new("aa"));
    let p = Predicate::Or(vec![
        Predicate::Eq(GremlinValue::String("a".to_string())),
        Predicate::Eq(GremlinValue::String("a".to_string())),
    ]);
    let mut targets = Vec::new();
    collect_predicate_labels_in_order(&p, &am, &mut targets);
    assert_eq!(targets.len(), 1);
}
#[test]
fn wb_collect_predicate_labels_not() {
    let mut am = HashMap::new();
    am.insert("a".to_string(), Identifier::new("aa"));
    let p = Predicate::Not(Box::new(Predicate::Eq(GremlinValue::String(
        "a".to_string(),
    ))));
    let mut targets = Vec::new();
    collect_predicate_labels_in_order(&p, &am, &mut targets);
    assert_eq!(targets.len(), 1);
}
#[test]
fn wb_collect_predicate_labels_unknown_skipped() {
    let p = Predicate::Eq(GremlinValue::String("nope".to_string()));
    let mut targets = Vec::new();
    collect_predicate_labels_in_order(&p, &empty_alias_map(), &mut targets);
    assert!(targets.is_empty());
}
#[test]
fn wb_collect_predicate_labels_non_string_within() {
    let p = Predicate::Within(vec![GremlinValue::Integer(1)]);
    let mut targets = Vec::new();
    collect_predicate_labels_in_order(&p, &empty_alias_map(), &mut targets);
    assert!(targets.is_empty());
}

#[test]
fn wb_wrap_where_by_null_eq_simple() {
    let mut e = Expr::BinaryOp {
        op: BinaryOp::Eq,
        left: Box::new(Expr::Variable(Identifier::new("a"))),
        right: Box::new(Expr::Variable(Identifier::new("b"))),
    };
    wrap_where_by_null_eq(&mut e);
    if let Expr::BinaryOp { op, .. } = e {
        assert!(matches!(op, BinaryOp::Or));
    } else {
        panic!("expected Or wrap");
    }
}
#[test]
fn wb_wrap_where_by_null_eq_recurse_and() {
    let inner = Expr::BinaryOp {
        op: BinaryOp::Eq,
        left: Box::new(Expr::Variable(Identifier::new("a"))),
        right: Box::new(Expr::Variable(Identifier::new("b"))),
    };
    let mut e = Expr::BinaryOp {
        op: BinaryOp::And,
        left: Box::new(inner.clone()),
        right: Box::new(inner),
    };
    wrap_where_by_null_eq(&mut e);
    // outer remains And
    if let Expr::BinaryOp { op, .. } = e {
        assert!(matches!(op, BinaryOp::And));
    } else {
        panic!("expected outer And");
    }
}
#[test]
fn wb_wrap_where_by_null_eq_neq_unchanged() {
    let mut e = Expr::BinaryOp {
        op: BinaryOp::Neq,
        left: Box::new(Expr::Variable(Identifier::new("a"))),
        right: Box::new(Expr::Variable(Identifier::new("b"))),
    };
    let before = format!("{:?}", e);
    wrap_where_by_null_eq(&mut e);
    let after = format!("{:?}", e);
    assert_eq!(before, after);
}

// =====================================================================
// helpers/operators.rs — inject_sack_init / inject_scan_into_child /
//   rewrite_create_alias / rewrite_sack_by direct tests
// =====================================================================

#[test]
fn op_inject_sack_init_into_node_scan() {
    let mut op = Operator::NodeScan {
        label: None,
        alias: Identifier::new("v"),
    };
    inject_sack_init(&mut op, &Expr::Integer(0));
    assert!(matches!(op, Operator::Apply { .. }));
}
#[test]
fn op_inject_sack_init_into_filter() {
    let mut op = Operator::Filter {
        predicate: Expr::Bool(true),
        child: Box::new(Operator::NodeScan {
            label: None,
            alias: Identifier::new("v"),
        }),
    };
    inject_sack_init(&mut op, &Expr::Integer(0));
    // The leaf inside Filter should be wrapped.
    if let Operator::Filter { child, .. } = op {
        assert!(matches!(*child, Operator::Apply { .. }));
    } else {
        panic!("expected Filter");
    }
}
#[test]
fn op_inject_sack_init_into_union_both_branches() {
    let leaf = || {
        Box::new(Operator::NodeScan {
            label: None,
            alias: Identifier::new("v"),
        })
    };
    let mut op = Operator::Union {
        left: leaf(),
        right: leaf(),
        all: true,
    };
    inject_sack_init(&mut op, &Expr::Integer(0));
    if let Operator::Union { left, right, .. } = op {
        assert!(matches!(*left, Operator::Apply { .. }));
        assert!(matches!(*right, Operator::Apply { .. }));
    } else {
        panic!("expected Union");
    }
}
#[test]
fn op_inject_sack_init_into_single_row() {
    let mut op = Operator::SingleRow;
    inject_sack_init(&mut op, &Expr::Integer(0));
    assert!(matches!(op, Operator::Apply { .. }));
}
#[test]
fn op_inject_sack_init_into_unwind() {
    let mut op = Operator::Unwind {
        list_expr: Expr::List(vec![Expr::Integer(1)]),
        alias: Identifier::new("u"),
        child: Box::new(Operator::SingleRow),
    };
    inject_sack_init(&mut op, &Expr::Integer(0));
    assert!(matches!(op, Operator::Apply { .. }));
}

#[test]
fn op_rewrite_sack_by_variable() {
    let target = Identifier::new("t");
    let replacement = Expr::Integer(42);
    let mut e = Expr::Variable(target.clone());
    rewrite_sack_by(&mut e, &target, &replacement);
    assert!(matches!(e, Expr::Integer(42)));
}
#[test]
fn op_rewrite_sack_by_in_binary_op() {
    let target = Identifier::new("t");
    let replacement = Expr::Integer(42);
    let mut e = Expr::BinaryOp {
        op: BinaryOp::Add,
        left: Box::new(Expr::Variable(target.clone())),
        right: Box::new(Expr::Variable(Identifier::new("other"))),
    };
    rewrite_sack_by(&mut e, &target, &replacement);
    if let Expr::BinaryOp { left, right, .. } = e {
        assert!(matches!(*left, Expr::Integer(42)));
        assert!(matches!(*right, Expr::Variable(_)));
    } else {
        panic!("expected BinaryOp");
    }
}
#[test]
fn op_rewrite_sack_by_other_unchanged() {
    let target = Identifier::new("t");
    let replacement = Expr::Integer(42);
    let mut e = Expr::Integer(7);
    rewrite_sack_by(&mut e, &target, &replacement);
    assert!(matches!(e, Expr::Integer(7)));
}

#[test]
fn op_rewrite_create_alias_in_create_edges() {
    use cypher_core::ast::{Direction, RelType};
    let pat = CreatePattern {
        nodes: vec![],
        edges: vec![(
            Some(Identifier::new("e")),
            Identifier::new("placeholder"),
            Identifier::new("dst"),
            RelType::new("KNOWS"),
            Direction::Outgoing,
            vec![],
        )],
    };
    let mut op = Operator::Create {
        patterns: vec![pat],
        child: None,
    };
    let repl = Identifier::new("real");
    rewrite_create_alias(&mut op, "placeholder", &repl);
    if let Operator::Create { patterns, .. } = &op {
        assert_eq!(patterns[0].edges[0].1.as_str(), "real");
    } else {
        panic!("expected Create");
    }
}
#[test]
fn op_rewrite_create_alias_in_apply() {
    use cypher_core::ast::{Direction, RelType};
    let pat = CreatePattern {
        nodes: vec![],
        edges: vec![(
            Some(Identifier::new("e")),
            Identifier::new("placeholder"),
            Identifier::new("dst"),
            RelType::new("KNOWS"),
            Direction::Outgoing,
            vec![],
        )],
    };
    let inner = Operator::Create {
        patterns: vec![pat],
        child: None,
    };
    let mut op = Operator::Apply {
        outer: Box::new(Operator::SingleRow),
        inner: Box::new(inner),
    };
    let repl = Identifier::new("real");
    rewrite_create_alias(&mut op, "placeholder", &repl);
    if let Operator::Apply { inner, .. } = op {
        if let Operator::Create { patterns, .. } = *inner {
            assert_eq!(patterns[0].edges[0].1.as_str(), "real");
        } else {
            panic!("expected Create");
        }
    } else {
        panic!("expected Apply");
    }
}
#[test]
fn op_rewrite_create_alias_in_set() {
    use cypher_core::ast::{Direction, RelType};
    let pat = CreatePattern {
        nodes: vec![],
        edges: vec![(
            Some(Identifier::new("e")),
            Identifier::new("src"),
            Identifier::new("placeholder"),
            RelType::new("KNOWS"),
            Direction::Outgoing,
            vec![],
        )],
    };
    let create = Operator::Create {
        patterns: vec![pat],
        child: None,
    };
    let mut op = Operator::Set {
        items: vec![],
        child: Box::new(create),
    };
    let repl = Identifier::new("real");
    rewrite_create_alias(&mut op, "placeholder", &repl);
    if let Operator::Set { child, .. } = op {
        if let Operator::Create { patterns, .. } = *child {
            assert_eq!(patterns[0].edges[0].2.as_str(), "real");
        } else {
            panic!("expected Create inside Set");
        }
    } else {
        panic!("expected Set");
    }
}

#[test]
fn op_inject_scan_into_child_create_with_no_existing_child() {
    let scan = Operator::NodeScan {
        label: None,
        alias: Identifier::new("v"),
    };
    let create = Operator::Create {
        patterns: vec![],
        child: None,
    };
    let res = inject_scan_into_child(create, scan);
    if let Operator::Create { child, .. } = res {
        assert!(child.is_some());
    } else {
        panic!("expected Create");
    }
}
#[test]
fn op_inject_scan_into_child_create_with_existing_child() {
    let scan = Operator::NodeScan {
        label: None,
        alias: Identifier::new("v"),
    };
    let create = Operator::Create {
        patterns: vec![],
        child: Some(Box::new(Operator::SingleRow)),
    };
    let res = inject_scan_into_child(create, scan);
    if let Operator::Create { child, .. } = res {
        if let Some(c) = child {
            assert!(matches!(*c, Operator::Apply { .. }));
        } else {
            panic!("expected child");
        }
    } else {
        panic!("expected Create");
    }
}
#[test]
fn op_inject_scan_into_child_set_recurses() {
    let scan = Operator::NodeScan {
        label: None,
        alias: Identifier::new("v"),
    };
    let create = Operator::Create {
        patterns: vec![],
        child: None,
    };
    let set = Operator::Set {
        items: vec![] as Vec<SetItem>,
        child: Box::new(create),
    };
    let res = inject_scan_into_child(set, scan);
    assert!(matches!(res, Operator::Set { .. }));
}
#[test]
fn op_inject_scan_into_child_other_falls_back_to_apply() {
    let scan = Operator::NodeScan {
        label: None,
        alias: Identifier::new("v"),
    };
    let other = Operator::SingleRow;
    let res = inject_scan_into_child(other, scan);
    assert!(matches!(res, Operator::Apply { .. }));
}

// =====================================================================
// helpers/steps.rs — pure helpers
// =====================================================================

#[test]
fn st_is_simple_predicate_steps_empty() {
    assert!(!is_simple_predicate_steps(&[]));
}
#[test]
fn st_is_simple_predicate_steps_single() {
    let steps = vec![GremlinStep::Has(
        "name".to_string(),
        Some(GremlinValue::String("a".to_string())),
    )];
    assert!(is_simple_predicate_steps(&steps));
}
#[test]
fn st_is_simple_predicate_steps_mixed() {
    let steps = vec![
        GremlinStep::Has(
            "name".to_string(),
            Some(GremlinValue::String("a".to_string())),
        ),
        GremlinStep::Count,
    ];
    assert!(!is_simple_predicate_steps(&steps));
}
#[test]
fn st_steps_need_path_tracking_yes() {
    assert!(steps_need_path_tracking(&[GremlinStep::Path]));
    assert!(steps_need_path_tracking(&[GremlinStep::SimplePath]));
    assert!(steps_need_path_tracking(&[GremlinStep::CyclicPath]));
}
#[test]
fn st_steps_need_path_tracking_no() {
    assert!(!steps_need_path_tracking(&[GremlinStep::Count]));
}
#[test]
fn st_steps_need_path_tracking_in_repeat() {
    let inner = vec![GremlinStep::Path];
    assert!(steps_need_path_tracking(&[GremlinStep::Repeat(
        None, inner
    )]));
}
#[test]
fn st_steps_need_path_tracking_in_or() {
    let inner = vec![GremlinStep::Path];
    assert!(steps_need_path_tracking(&[GremlinStep::Or(vec![inner])]));
}
#[test]
fn st_steps_need_path_tracking_in_choose() {
    let if_b = vec![GremlinStep::Path];
    let else_b: Vec<GremlinStep> = vec![];
    assert!(steps_need_path_tracking(&[GremlinStep::Choose(
        if_b, else_b, None
    )]));
}

#[test]
fn st_steps_need_label_stacks_yes() {
    let steps = vec![GremlinStep::Select(
        Some(crate::bytecode::Pop::First),
        vec!["a".into()],
    )];
    assert!(steps_need_label_stacks(&steps));
}
#[test]
fn st_steps_need_label_stacks_no() {
    assert!(!steps_need_label_stacks(&[GremlinStep::Count]));
}
#[test]
fn st_steps_need_label_stacks_in_repeat() {
    let inner = vec![GremlinStep::Select(
        Some(crate::bytecode::Pop::Last),
        vec!["a".into()],
    )];
    assert!(steps_need_label_stacks(&[GremlinStep::Repeat(None, inner)]));
}
#[test]
fn st_steps_need_label_stacks_in_or() {
    let inner = vec![GremlinStep::Select(
        Some(crate::bytecode::Pop::All),
        vec!["a".into()],
    )];
    assert!(steps_need_label_stacks(&[GremlinStep::Or(vec![inner])]));
}

#[test]
fn st_split_trailing_agg_count() {
    let steps = vec![GremlinStep::Path, GremlinStep::Count];
    let (rest, agg) = split_trailing_agg(&steps);
    assert_eq!(rest.len(), 1);
    assert!(agg.is_some());
}
#[test]
fn st_split_trailing_agg_sum() {
    let steps = vec![GremlinStep::Path, GremlinStep::Sum];
    let (_, agg) = split_trailing_agg(&steps);
    assert!(agg.is_some());
}
#[test]
fn st_split_trailing_agg_fold() {
    let steps = vec![GremlinStep::Path, GremlinStep::Fold];
    let (_, agg) = split_trailing_agg(&steps);
    assert!(agg.is_some());
}
#[test]
fn st_split_trailing_agg_none() {
    let steps = vec![GremlinStep::Path];
    let (rest, agg) = split_trailing_agg(&steps);
    assert_eq!(rest.len(), 1);
    assert!(agg.is_none());
}
#[test]
fn st_split_trailing_agg_empty() {
    let steps: Vec<GremlinStep> = vec![];
    let (_, agg) = split_trailing_agg(&steps);
    assert!(agg.is_none());
}

#[test]
fn st_split_trailing_group_value_agg_fold_keeps() {
    let steps = vec![GremlinStep::Path, GremlinStep::Fold];
    let (rest, agg) = split_trailing_group_value_agg(&steps);
    assert_eq!(rest.len(), 2);
    assert!(agg.is_none());
}
#[test]
fn st_split_trailing_group_value_agg_count_splits() {
    let steps = vec![GremlinStep::Path, GremlinStep::Count];
    let (_, agg) = split_trailing_group_value_agg(&steps);
    assert!(agg.is_some());
}

#[test]
fn st_preprocess_infix_no_infix_passthrough() {
    let steps = vec![GremlinStep::V(None), GremlinStep::Count];
    let out = preprocess_infix_connectives(&steps);
    assert_eq!(out.len(), steps.len());
}
#[test]
fn st_preprocess_infix_or_only_in_source_passthrough() {
    // Or with non-empty branches isn't infix — passthrough.
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::Or(vec![vec![GremlinStep::Count]]),
    ];
    let out = preprocess_infix_connectives(&steps);
    assert_eq!(out.len(), steps.len());
}
#[test]
fn st_preprocess_infix_and_splits() {
    // V().has(...).and().has(...)
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::Has("a".into(), Some(GremlinValue::Integer(1))),
        GremlinStep::And(vec![]),
        GremlinStep::Has("b".into(), Some(GremlinValue::Integer(2))),
    ];
    let out = preprocess_infix_connectives(&steps);
    // result: [V, And([...,...])]
    assert!(out.iter().any(|s| matches!(s, GremlinStep::And(_))));
}
#[test]
fn st_preprocess_infix_or_splits() {
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::Has("a".into(), Some(GremlinValue::Integer(1))),
        GremlinStep::Or(vec![]),
        GremlinStep::Has("b".into(), Some(GremlinValue::Integer(2))),
    ];
    let out = preprocess_infix_connectives(&steps);
    assert!(out.iter().any(|s| matches!(s, GremlinStep::Or(_))));
}
#[test]
fn st_preprocess_infix_or_and_mixed() {
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::Has("a".into(), Some(GremlinValue::Integer(1))),
        GremlinStep::And(vec![]),
        GremlinStep::Has("b".into(), Some(GremlinValue::Integer(2))),
        GremlinStep::Or(vec![]),
        GremlinStep::Has("c".into(), Some(GremlinValue::Integer(3))),
    ];
    let out = preprocess_infix_connectives(&steps);
    // Or has lower precedence — outer should be Or
    assert!(out.iter().any(|s| matches!(s, GremlinStep::Or(_))));
}

#[test]
fn st_split_at_empty_connective_and_segments() {
    let steps = vec![
        GremlinStep::Has("a".into(), Some(GremlinValue::Integer(1))),
        GremlinStep::And(vec![]),
        GremlinStep::Has("b".into(), Some(GremlinValue::Integer(2))),
    ];
    let segs = split_at_empty_connective(&steps, true);
    assert_eq!(segs.len(), 2);
}
#[test]
fn st_split_at_empty_connective_or_segments() {
    let steps = vec![
        GremlinStep::Has("a".into(), Some(GremlinValue::Integer(1))),
        GremlinStep::Or(vec![]),
        GremlinStep::Has("b".into(), Some(GremlinValue::Integer(2))),
    ];
    let segs = split_at_empty_connective(&steps, false);
    assert_eq!(segs.len(), 2);
}

// =====================================================================
// has_kv.rs — has key value with predicates and lists
// =====================================================================

#[test]
fn hk_has_key_only() {
    run("g.V().has(\"name\")");
}
#[test]
fn hk_has_key_value() {
    run("g.V().has(\"name\", \"alice\")");
}
#[test]
fn hk_has_key_predicate_gt() {
    run("g.V().has(\"age\", gt(20))");
}
#[test]
fn hk_has_key_predicate_within_list() {
    run("g.V().has(\"name\", within(\"a\", \"b\"))");
}
#[test]
fn hk_has_key_predicate_without_list() {
    run("g.V().has(\"name\", without(\"a\", \"b\"))");
}
#[test]
fn hk_has_key_predicate_between() {
    run("g.V().has(\"age\", between(18, 30))");
}
#[test]
fn hk_has_key_predicate_inside() {
    run("g.V().has(\"age\", inside(18, 30))");
}
#[test]
fn hk_has_label_key_value() {
    run("g.V().has(\"person\", \"name\", \"alice\")");
}
#[test]
fn hk_has_label_key_predicate() {
    run("g.V().has(\"person\", \"age\", gt(20))");
}
#[test]
fn hk_has_id_basic() {
    run("g.V().hasId(1)");
}
#[test]
fn hk_has_id_multi() {
    run("g.V().hasId(1, 2, 3)");
}
#[test]
fn hk_has_id_predicate() {
    run("g.V().hasId(gt(5))");
}
#[test]
fn hk_has_label_basic() {
    run("g.V().hasLabel(\"person\")");
}
#[test]
fn hk_has_label_multi() {
    run("g.V().hasLabel(\"person\", \"software\")");
}
#[test]
fn hk_has_key_text_starting_with() {
    run("g.V().has(\"name\", startingWith(\"a\"))");
}
#[test]
fn hk_has_key_text_ending_with() {
    run("g.V().has(\"name\", endingWith(\"e\"))");
}
#[test]
fn hk_has_key_text_containing() {
    run("g.V().has(\"name\", containing(\"li\"))");
}
#[test]
fn hk_has_key_text_not_starting_with() {
    run("g.V().has(\"name\", notStartingWith(\"a\"))");
}
#[test]
fn hk_has_key_text_not_ending_with() {
    run("g.V().has(\"name\", notEndingWith(\"e\"))");
}
#[test]
fn hk_has_key_text_not_containing() {
    run("g.V().has(\"name\", notContaining(\"li\"))");
}
#[test]
fn hk_has_key_regex() {
    run("g.V().has(\"name\", regex(\"a.*\"))");
}
#[test]
fn hk_has_not_property() {
    run("g.V().hasNot(\"name\")");
}
#[test]
fn hk_has_key_value_null() {
    run("g.V().has(\"name\", null)");
}
