//! Coverage-focused tests for `compiler/steps/strings.rs`.

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

// ── split ──────────────────────────────────────────────────────────────

#[test]
fn strings_split_csv() {
    let _ = compile("g.V().values(\"name\").split(\",\")");
}

#[test]
fn strings_split_empty_separator() {
    let _ = compile("g.V().values(\"name\").split(\"\")");
}

#[test]
fn strings_split_no_match() {
    let _ = compile("g.V().values(\"name\").split(\"|\")");
}

// ── concat ─────────────────────────────────────────────────────────────

#[test]
fn strings_concat_two_traversals() {
    let _ = compile("g.V().values(\"name\").concat(\"-suffix\")");
}

#[test]
fn strings_concat_chained() {
    let _ = compile("g.V().values(\"name\").concat(\"-\").concat(values(\"age\"))");
}

// ── length ─────────────────────────────────────────────────────────────

#[test]
fn strings_length_on_property() {
    let _ = compile("g.V().values(\"name\").length()");
}

#[test]
fn strings_length_after_concat() {
    let _ = compile("g.V().values(\"name\").concat(\"x\").length()");
}

// ── case folding ───────────────────────────────────────────────────────

#[test]
fn strings_to_lower() {
    let _ = compile("g.V().values(\"name\").toLower()");
}

#[test]
fn strings_to_upper() {
    let _ = compile("g.V().values(\"name\").toUpper()");
}

#[test]
fn strings_to_lower_then_to_upper_chain() {
    let _ = compile("g.V().values(\"name\").toLower().toUpper()");
}

// ── trim variants ──────────────────────────────────────────────────────

#[test]
fn strings_trim() {
    let _ = compile("g.V().values(\"name\").trim()");
}

#[test]
fn strings_l_trim() {
    let _ = compile("g.V().values(\"name\").lTrim()");
}

#[test]
fn strings_r_trim() {
    let _ = compile("g.V().values(\"name\").rTrim()");
}

// ── substring ──────────────────────────────────────────────────────────

#[test]
fn strings_substring_start_only() {
    let _ = compile("g.V().values(\"name\").substring(2)");
}

#[test]
fn strings_substring_start_and_end() {
    let _ = compile("g.V().values(\"name\").substring(0, 3)");
}

#[test]
fn strings_substring_negative_start() {
    let _ = compile("g.V().values(\"name\").substring(-2)");
}

#[test]
fn strings_substring_out_of_range() {
    let _ = compile("g.V().values(\"name\").substring(0, 1000)");
}

// ── replace ────────────────────────────────────────────────────────────

#[test]
fn strings_replace_simple() {
    let _ = compile("g.V().values(\"name\").replace(\"a\", \"b\")");
}

#[test]
fn strings_replace_with_regex_chars() {
    let _ = compile("g.V().values(\"name\").replace(\".\", \"_\")");
}

// ── reverse ────────────────────────────────────────────────────────────

#[test]
fn strings_reverse() {
    let _ = compile("g.V().values(\"name\").reverse()");
}

// ── format ─────────────────────────────────────────────────────────────

#[test]
fn strings_format_with_property_placeholder() {
    let _ = compile("g.V().format(\"%{name}\")");
}

#[test]
fn strings_format_with_underscore_placeholder() {
    let _ = compile("g.V().values(\"name\").format(\"name=%{_}\")");
}

#[test]
fn strings_format_combined_chain() {
    let _ = compile("g.V().has(\"name\").format(\"%{name}-%{age}\")");
}
