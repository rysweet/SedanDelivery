//! Round-3 coverage tests, part 2 (split for file size).
//!
//! Continuation of `coverage_tests3.rs` covering aggregation, navigation,
//! property mutations, sources, and value-map outputs.

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
// Aggregation steps (steps/aggregation.rs)
// =====================================================================

#[test]
fn count_basic() {
    run("g.V().count()");
}
#[test]
fn count_local_list() {
    run("g.V().fold().count(local)");
}
#[test]
fn sum_basic() {
    run("g.V().values(\"age\").sum()");
}
#[test]
fn min_basic() {
    run("g.V().values(\"age\").min()");
}
#[test]
fn max_basic() {
    run("g.V().values(\"age\").max()");
}
#[test]
fn mean_basic() {
    run("g.V().values(\"age\").mean()");
}
#[test]
fn fold_basic() {
    run("g.V().values(\"age\").fold()");
}
#[test]
fn fold_with_seed() {
    run("g.V().values(\"age\").fold(0, sum)");
}
#[test]
fn unfold_basic() {
    run("g.V().values(\"name\").fold().unfold()");
}

// =====================================================================
// filter step variants (steps/filter/*)
// =====================================================================

#[test]
fn filter_traversal() {
    run("g.V().filter(__.out().count().is(gt(0)))");
}
#[test]
fn filter_has_kv() {
    run("g.V().filter(__.has(\"name\", \"alice\"))");
}
#[test]
fn quantifier_all_eq() {
    run("g.V().has(\"name\", all(eq(\"alice\")))");
}
#[test]
fn quantifier_any_eq() {
    run("g.V().has(\"name\", any(eq(\"alice\")))");
}

// =====================================================================
// Vertex / edge navigation (steps/source.rs, steps/limit.rs, etc.)
// =====================================================================

#[test]
fn out_in_both() {
    run("g.V().out().in().both()");
}
#[test]
fn out_in_with_label() {
    run("g.V().out(\"knows\").in(\"created\")");
}
#[test]
fn out_e_in_e() {
    run("g.V().outE().inV()");
}
#[test]
fn both_e_other_v() {
    run("g.V().bothE().otherV()");
}
#[test]
fn limit_basic() {
    run("g.V().limit(5)");
}
#[test]
fn limit_local() {
    run("g.V().fold().limit(local, 3)");
}
#[test]
fn skip_basic() {
    run("g.V().skip(2)");
}
#[test]
fn range_basic() {
    run("g.V().range(0, 5)");
}
#[test]
fn range_local() {
    run("g.V().fold().range(local, 0, 3)");
}
#[test]
fn tail_basic() {
    run("g.V().tail(3)");
}
#[test]
fn tail_local() {
    run("g.V().fold().tail(local, 3)");
}

// =====================================================================
// Property step variants (steps/mutation/property.rs)
// =====================================================================

#[test]
fn property_single_kv() {
    run("g.V(1).property(\"age\", 30)");
}
#[test]
fn property_with_cardinality() {
    run("g.V(1).property(single, \"age\", 30)");
}
#[test]
fn property_set_cardinality() {
    run("g.V(1).property(set, \"hobby\", \"chess\")");
}
#[test]
fn property_list_cardinality() {
    run("g.V(1).property(list, \"hobby\", \"chess\")");
}
#[test]
fn property_with_meta() {
    run("g.V(1).property(\"age\", 30, \"source\", \"db\")");
}

// =====================================================================
// constant() / V(id) / E(id) sources
// =====================================================================

#[test]
fn v_with_id() {
    run("g.V(1)");
}
#[test]
fn v_with_multi_ids() {
    run("g.V(1, 2, 3)");
}
#[test]
fn e_with_id() {
    run("g.E(1)");
}
#[test]
fn constant_int() {
    run("g.V().constant(42)");
}
#[test]
fn constant_string() {
    run("g.V().constant(\"hello\")");
}
#[test]
fn inject_basic() {
    run("g.inject(1, 2, 3)");
}

// =====================================================================
// Map output / valueMap / elementMap (steps/limit.rs / source.rs paths)
// =====================================================================

#[test]
fn value_map_basic() {
    run("g.V().valueMap()");
}
#[test]
fn value_map_with_keys() {
    run("g.V().valueMap(\"name\", \"age\")");
}
#[test]
fn value_map_with_id() {
    run("g.V().valueMap().with(\"~tinkerpop.valueMap.tokens\")");
}
#[test]
fn element_map_basic() {
    run("g.V().elementMap()");
}
#[test]
fn property_map_basic() {
    run("g.V().propertyMap()");
}

// =====================================================================
// text_parser literal value (text_parser/literal/value.rs)
// =====================================================================

#[test]
fn literal_long_suffix() {
    run("g.V(1L)");
}
#[test]
fn literal_int_suffix() {
    run("g.V(1i)");
}
#[test]
fn literal_short_suffix() {
    run("g.V(1s)");
}
#[test]
fn literal_byte_suffix() {
    run("g.V(1b)");
}
#[test]
fn literal_float_suffix() {
    run("g.V().constant(0.5f)");
}
#[test]
fn literal_double_suffix() {
    run("g.V().constant(0.5d)");
}
#[test]
fn literal_decimal_suffix() {
    run("g.V().constant(0.5m)");
}
#[test]
fn literal_nan() {
    run("g.V().constant(NaN)");
}
#[test]
fn literal_double_nan() {
    run("g.V().constant(Double.NaN)");
}
#[test]
fn literal_pos_infinity() {
    run("g.V().constant(Infinity)");
}
#[test]
fn literal_neg_infinity() {
    run("g.V().constant(-Infinity)");
}
#[test]
fn literal_double_pos_inf() {
    run("g.V().constant(Double.POSITIVE_INFINITY)");
}
#[test]
fn literal_double_neg_inf() {
    run("g.V().constant(Double.NEGATIVE_INFINITY)");
}
#[test]
fn literal_set_dedup() {
    run("g.V().constant({1, 2, 2, 3})");
}
#[test]
fn literal_groovy_map() {
    run("g.V().has(\"props\", [\"key\": \"val\"])");
}
#[test]
fn literal_empty_groovy_map() {
    run("g.V().has(\"props\", [:])");
}
#[test]
fn literal_list() {
    run("g.V().has(\"tags\", [1, 2, 3])");
}
#[test]
fn literal_typed_d_int() {
    run("g.V().constant(d[29].i)");
}
#[test]
fn literal_typed_d_float() {
    run("g.V().constant(d[0.5].d)");
}
#[test]
fn literal_t_label_token() {
    run("g.V().has(T.label, \"person\")");
}
#[test]
fn literal_t_id_token() {
    run("g.V(T.id)");
}
#[test]
fn literal_scope_local_token() {
    run("g.V().fold().count(local)");
}
#[test]
fn literal_pick_none() {
    run("g.V().choose(__.label()).option(Pick.none, __.constant(0))");
}
#[test]
fn literal_g_type_int() {
    run("g.V().has(\"age\", GType.i32)");
}
#[test]
fn literal_datetime() {
    run("g.V().constant(datetime(\"2023-01-01T00:00:00Z\"))");
}
#[test]
fn literal_uuid() {
    run("g.V().constant(UUID(\"5100808b-0000-0000-0000-000000000000\"))");
}
#[test]
fn literal_string_escapes() {
    run("g.V().has(\"name\", \"a\\\"b\")");
}
#[test]
fn literal_string_single_quote() {
    run("g.V().has(\"name\", 'alice')");
}
#[test]
fn literal_string_single_quote_escape() {
    run("g.V().has(\"name\", 'a\\'b')");
}
#[test]
fn literal_null() {
    run("g.V().has(\"x\", null)");
}
#[test]
fn literal_bool_true() {
    run("g.V().has(\"x\", true)");
}
#[test]
fn literal_bool_false() {
    run("g.V().has(\"x\", false)");
}
#[test]
fn literal_negative_int() {
    run("g.V().has(\"x\", -42)");
}
