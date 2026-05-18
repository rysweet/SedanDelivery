//! Coverage-focused tests for `compiler/steps/by.rs`.
//!
//! Exercises every shape of `by(...)` modulator against `order`, `group`,
//! `groupCount`, `select`, `path`, `dedup`, `project` and `tree` parents.

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

// ── by() — no-arg identity projection ──────────────────────────────────

#[test]
fn by_no_arg_after_order() {
    let _ = compile("g.V().order().by()");
}

#[test]
fn by_no_arg_after_dedup() {
    let _ = compile("g.V().dedup().by()");
}

#[test]
fn by_no_arg_after_group_count() {
    let _ = compile("g.V().groupCount().by()");
}

// ── by('key') — string property key ────────────────────────────────────

#[test]
fn by_key_after_order() {
    let _ = compile("g.V().order().by(\"name\")");
}

#[test]
fn by_key_after_group() {
    let _ = compile("g.V().group().by(\"name\")");
}

#[test]
fn by_key_after_group_count() {
    let _ = compile("g.V().groupCount().by(\"name\")");
}

#[test]
fn by_key_after_path() {
    let _ = compile("g.V().out().path().by(\"name\")");
}

#[test]
fn by_key_after_dedup() {
    let _ = compile("g.V().dedup().by(\"name\")");
}

// ── by(values('age')) — anonymous traversal projecting a value ─────────

#[test]
fn by_anon_values_traversal_in_group() {
    let _ = compile("g.V().group().by(values(\"name\"))");
}

#[test]
fn by_anon_values_traversal_in_order() {
    let _ = compile("g.V().order().by(values(\"age\"))");
}

#[test]
fn by_anon_values_traversal_in_dedup() {
    let _ = compile("g.V().dedup().by(values(\"name\"))");
}

// ── by(__.outE().count()) — anonymous composition ──────────────────────

#[test]
fn by_anon_composition_count_after_order() {
    let _ = compile("g.V().order().by(__.outE().count())");
}

#[test]
fn by_anon_composition_count_after_group() {
    let _ = compile("g.V().group().by(\"name\").by(__.outE().count())");
}

#[test]
fn by_anon_composition_count_after_project() {
    let _ = compile("g.V().project(\"a\").by(__.outE().count())");
}

#[test]
fn by_anon_composition_count_in_groupcount() {
    let _ = compile("g.V().groupCount().by(__.label())");
}

// ── by(label) / by(id) / by(T.label) / by(T.id) ────────────────────────

#[test]
fn by_label_token_after_group() {
    let _ = compile("g.V().group().by(label)");
}

#[test]
fn by_id_token_after_group() {
    let _ = compile("g.V().group().by(id)");
}

#[test]
fn by_t_label_after_order() {
    let _ = compile("g.V().order().by(T.label)");
}

#[test]
fn by_t_id_after_order() {
    let _ = compile("g.V().order().by(T.id)");
}

#[test]
fn by_t_label_in_groupcount() {
    let _ = compile("g.V().groupCount().by(T.label)");
}

// ── by('name', desc) — order + sort direction ──────────────────────────

#[test]
fn by_key_desc_after_order() {
    let _ = compile("g.V().order().by(\"name\", desc)");
}

#[test]
fn by_key_asc_after_order() {
    let _ = compile("g.V().order().by(\"name\", asc)");
}

#[test]
fn by_t_label_desc() {
    let _ = compile("g.V().order().by(T.label, desc)");
}

#[test]
fn by_t_id_desc() {
    let _ = compile("g.V().order().by(T.id, desc)");
}

#[test]
fn by_anon_count_desc() {
    let _ = compile("g.V().order().by(__.outE().count(), desc)");
}

// ── by(Order.shuffle) ──────────────────────────────────────────────────

#[test]
fn by_shuffle_order() {
    let _ = compile("g.V().order().by(shuffle)");
}

// ── by chains for project ──────────────────────────────────────────────

#[test]
fn by_two_keys_for_project() {
    let _ = compile("g.V().project(\"name\", \"age\").by(\"name\").by(\"age\")");
}

#[test]
fn by_mixed_key_and_traversal_for_project() {
    let _ = compile("g.V().project(\"a\", \"b\").by(\"name\").by(__.bothE().count())");
}

// ── by(Column.keys / Column.values) ────────────────────────────────────

#[test]
fn by_column_keys_after_group() {
    let _ = compile("g.V().groupCount().by(\"name\").select(keys)");
}

// ── by anon containing complex sub-traversal ───────────────────────────

#[test]
fn by_anon_with_filter_chain() {
    let _ = compile("g.V().order().by(__.out().has(\"name\", \"x\").count())");
}

#[test]
fn by_anon_with_select() {
    let _ = compile("g.V().as(\"a\").out().order().by(__.select(\"a\").values(\"age\"))");
}

// ── Bug #1192: by() must propagate parse/compile errors ────────────────
//
// Currently handle_by_project and handle_by_group use `if let Ok(...)`
// patterns that silently fall back to simple property expressions when
// sub-traversal parsing or compilation fails. After fix: use `?` to
// propagate the error as CompileError.

fn try_compile(input: &str) -> Result<Operator, super::CompileError> {
    let steps = parse_traversal(input, &no_param).expect("parse_traversal");
    super::GremlinCompiler::new().compile(&steps)
}

#[test]
fn project_by_invalid_traversal_returns_error() {
    // by() after project() with an unparseable sub-traversal should
    // return an error instead of silently falling back.
    let result = try_compile("g.V().project(\"a\").by(__.completelyFakeStep())");
    assert!(
        result.is_err(),
        "invalid by() sub-traversal after project() should produce CompileError, got: {result:?}"
    );
}

#[test]
fn group_key_by_invalid_traversal_returns_error() {
    // by() after group() with an unparseable sub-traversal for the
    // group key should return an error.
    let result = try_compile("g.V().group().by(__.completelyFakeStep())");
    assert!(
        result.is_err(),
        "invalid by() sub-traversal after group() should produce CompileError, got: {result:?}"
    );
}

#[test]
fn group_value_by_invalid_traversal_returns_error() {
    // Second by() after group() (value aggregation) with an invalid
    // sub-traversal should return an error.
    let result = try_compile("g.V().group().by(\"name\").by(__.completelyFakeStep())");
    assert!(
        result.is_err(),
        "invalid value-by() traversal after group() should produce CompileError, got: {result:?}"
    );
}
