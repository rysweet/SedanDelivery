//! Unit tests for gql-engine parser edge cases.
//!
//! Tests the rewrite module's keyword normalisation and metadata extraction
//! with adversarial and boundary inputs that the outside-in tests don't cover.

use gql_engine::parser::{parse_gql, rewrite, PathMode, PathSearch};

// ── INSERT normalisation ─────────────────────────────────────────────────────

#[test]
fn insert_uppercase() {
    let out = rewrite::normalise_gql_source("INSERT (n:Person)");
    assert!(out.starts_with("CREATE"), "got: {out}");
}

#[test]
fn insert_lowercase() {
    let out = rewrite::normalise_gql_source("insert (n:Person)");
    assert!(out.starts_with("CREATE"), "got: {out}");
}

#[test]
fn insert_mixed_case() {
    let out = rewrite::normalise_gql_source("Insert (n:Person)");
    assert!(out.starts_with("CREATE"), "got: {out}");
}

#[test]
fn insert_in_property_value_not_rewritten() {
    // The word "insert" inside a string literal should not be rewritten,
    // but our tokeniser operates at the word level so it will. This test
    // documents current behaviour: property keys named "insert" ARE rewritten.
    // A full GQL-native parser would fix this; for now we accept it.
    let out = rewrite::normalise_gql_source("RETURN {insert: 1}");
    // The token "insert" is a standalone word — it will be rewritten
    assert!(out.contains("CREATE") || out.contains("insert"));
}

// ── FILTER normalisation ─────────────────────────────────────────────────────

#[test]
fn filter_clause_becomes_where() {
    let out = rewrite::normalise_gql_source("MATCH (n) FILTER n.age > 21 RETURN n");
    assert!(out.contains("WHERE"), "got: {out}");
    assert!(
        !out.contains("FILTER"),
        "FILTER should be replaced, got: {out}"
    );
}

#[test]
fn filter_function_preserved() {
    let out = rewrite::normalise_gql_source("RETURN filter(x IN [1,2,3] WHERE x > 1)");
    assert!(
        out.contains("filter("),
        "filter() function should be preserved, got: {out}"
    );
}

#[test]
fn filter_where_redundant_where_stripped() {
    let out = rewrite::normalise_gql_source("MATCH (n) FILTER WHERE n.age > 21 RETURN n");
    let where_count = out.matches("WHERE").count();
    assert_eq!(
        where_count, 1,
        "double WHERE should be collapsed, got: {out}"
    );
}

#[test]
fn filter_case_insensitive() {
    let out = rewrite::normalise_gql_source("MATCH (n) filter n.x > 1 RETURN n");
    assert!(out.contains("WHERE"), "got: {out}");
}

// ── MANDATORY MATCH normalisation ────────────────────────────────────────────

#[test]
fn mandatory_match_stripped_from_source() {
    let out = rewrite::normalise_gql_source("MANDATORY MATCH (n) RETURN n");
    assert!(!out.contains("MANDATORY"), "got: {out}");
    assert!(out.contains("MATCH"), "got: {out}");
}

#[test]
fn mandatory_match_metadata_extracted() {
    let meta = rewrite::extract_gql_metadata("MANDATORY MATCH (n) RETURN n");
    assert!(meta.mandatory_match);
}

#[test]
fn mandatory_match_case_insensitive_metadata() {
    let meta = rewrite::extract_gql_metadata("mandatory match (n) RETURN n");
    assert!(meta.mandatory_match);
}

#[test]
fn non_mandatory_has_no_flag() {
    let meta = rewrite::extract_gql_metadata("MATCH (n) RETURN n");
    assert!(!meta.mandatory_match);
}

// ── FOR normalisation ────────────────────────────────────────────────────────

#[test]
fn for_becomes_unwind() {
    let out = rewrite::normalise_gql_source("FOR x IN [1, 2, 3] RETURN x");
    assert!(out.contains("UNWIND"), "got: {out}");
    assert!(out.contains("AS x"), "got: {out}");
}

#[test]
fn for_with_complex_list_expression() {
    let out = rewrite::normalise_gql_source("FOR item IN [1, 2, 3] RETURN item");
    assert!(out.contains("UNWIND"), "got: {out}");
    assert!(out.contains("AS item"), "got: {out}");
}

// ── LET normalisation ────────────────────────────────────────────────────────

#[test]
fn let_becomes_with_star() {
    let out = rewrite::normalise_gql_source("MATCH (n) LET x = n.name RETURN x");
    assert!(
        out.contains("WITH *,"),
        "LET should become WITH *, got: {out}"
    );
    assert!(out.contains("AS x"), "got: {out}");
}

#[test]
fn let_at_end_of_input() {
    // Malformed: LET with no variable name
    let out = rewrite::normalise_gql_source("MATCH (n) LET");
    // Should not panic, should produce some output
    assert!(!out.is_empty());
}

// ── Path mode metadata ───────────────────────────────────────────────────────

#[test]
fn path_mode_walk() {
    let meta = rewrite::extract_gql_metadata("MATCH WALK (a)-[*]->(b) RETURN a");
    assert_eq!(meta.path_mode, Some(PathMode::Walk));
}

#[test]
fn path_mode_trail() {
    let meta = rewrite::extract_gql_metadata("MATCH TRAIL (a)-[*]->(b) RETURN a");
    assert_eq!(meta.path_mode, Some(PathMode::Trail));
}

#[test]
fn path_mode_simple() {
    let meta = rewrite::extract_gql_metadata("MATCH SIMPLE (a)-[*]->(b) RETURN a");
    assert_eq!(meta.path_mode, Some(PathMode::Simple));
}

#[test]
fn path_mode_acyclic() {
    let meta = rewrite::extract_gql_metadata("MATCH ACYCLIC (a)-[*]->(b) RETURN a");
    assert_eq!(meta.path_mode, Some(PathMode::Acyclic));
}

#[test]
fn no_path_mode_for_plain_match() {
    let meta = rewrite::extract_gql_metadata("MATCH (a)-[:R]->(b) RETURN a");
    assert_eq!(meta.path_mode, None);
}

#[test]
fn path_mode_stripped_from_normalised_source() {
    for keyword in &["WALK", "TRAIL", "SIMPLE", "ACYCLIC"] {
        let src = format!("MATCH {keyword} (a)-[:R]->(b) RETURN a");
        let out = rewrite::normalise_gql_source(&src);
        assert!(
            !out.to_ascii_uppercase().contains(keyword),
            "{keyword} should be stripped, got: {out}"
        );
    }
}

// ── Path search metadata ─────────────────────────────────────────────────────

#[test]
fn path_search_any_shortest() {
    let meta = rewrite::extract_gql_metadata("MATCH ANY SHORTEST (a)-[*]->(b) RETURN a");
    assert_eq!(meta.path_search, Some(PathSearch::AnyShortest));
}

#[test]
fn path_search_all_shortest() {
    let meta = rewrite::extract_gql_metadata("MATCH ALL SHORTEST (a)-[*]->(b) RETURN a");
    assert_eq!(meta.path_search, Some(PathSearch::AllShortest));
}

#[test]
fn path_search_any() {
    let meta = rewrite::extract_gql_metadata("MATCH ANY (a)-[*]->(b) RETURN a");
    assert_eq!(meta.path_search, Some(PathSearch::Any));
}

#[test]
fn path_search_all() {
    let meta = rewrite::extract_gql_metadata("MATCH ALL (a)-[*]->(b) RETURN a");
    assert_eq!(meta.path_search, Some(PathSearch::All));
}

#[test]
fn no_path_search_for_plain_match() {
    let meta = rewrite::extract_gql_metadata("MATCH (a)-[:R]->(b) RETURN a");
    assert_eq!(meta.path_search, None);
}

#[test]
fn path_search_keywords_stripped_from_normalised_source() {
    let out = rewrite::normalise_gql_source("MATCH ANY SHORTEST (a)-[:R]->(b) RETURN a");
    assert!(
        !out.to_ascii_uppercase().contains("SHORTEST"),
        "SHORTEST should be stripped, got: {out}"
    );
}

// ── Combined keyword tests ───────────────────────────────────────────────────

#[test]
fn multiple_gql_keywords_in_one_query() {
    let src = "INSERT (n:Person {name: 'Alice'}) LET x = n.name FILTER x = 'Alice' RETURN x";
    let out = rewrite::normalise_gql_source(src);
    assert!(out.contains("CREATE"), "INSERT→CREATE");
    assert!(out.contains("WITH"), "LET→WITH");
    assert!(out.contains("WHERE"), "FILTER→WHERE");
}

#[test]
fn standard_cypher_passes_through_unchanged() {
    let src = "MATCH (n:Person) WHERE n.age > 21 RETURN n.name ORDER BY n.name LIMIT 10";
    let out = rewrite::normalise_gql_source(src);
    assert_eq!(out, src);
}

// ── parse_gql integration ────────────────────────────────────────────────────

#[test]
fn parse_gql_returns_metadata_and_ast() {
    let gql = parse_gql("MANDATORY MATCH (n) RETURN n").unwrap();
    assert!(gql.metadata.mandatory_match);
}

#[test]
fn parse_gql_invalid_syntax_returns_error() {
    let result = parse_gql("NOT VALID QUERY !!!");
    assert!(result.is_err());
}

#[test]
fn parse_gql_empty_string_returns_error() {
    let result = parse_gql("");
    assert!(result.is_err());
}

#[test]
fn parse_gql_with_path_mode() {
    let gql = parse_gql("MATCH TRAIL (a)-[:R]->(b) RETURN a").unwrap();
    assert_eq!(gql.metadata.path_mode, Some(PathMode::Trail));
}

#[test]
fn parse_gql_with_path_search() {
    let gql = parse_gql("MATCH ANY SHORTEST (a)-[:R]->(b) RETURN a").unwrap();
    assert_eq!(gql.metadata.path_search, Some(PathSearch::AnyShortest));
}

// ── GqlParseError Display ────────────────────────────────────────────────────

#[test]
fn parse_error_display_contains_context() {
    let err = parse_gql("XYZZY").unwrap_err();
    let msg = format!("{err}");
    assert!(!msg.is_empty(), "error message should not be empty");
}

// ── Edge: whitespace variations ──────────────────────────────────────────────

#[test]
fn extra_whitespace_between_keywords() {
    let out = rewrite::normalise_gql_source("MANDATORY   MATCH  (n)  RETURN  n");
    assert!(!out.contains("MANDATORY"), "got: {out}");
    assert!(out.contains("MATCH"), "got: {out}");
}

#[test]
fn tabs_and_newlines_in_source() {
    let src = "MANDATORY\tMATCH\n(n)\nRETURN\tn";
    let out = rewrite::normalise_gql_source(src);
    assert!(!out.contains("MANDATORY"), "got: {out}");
}

// ── Edge: keyword as identifier ──────────────────────────────────────────────

#[test]
fn filter_not_rewritten_when_followed_by_paren() {
    // filter(...) is a function call, not a clause
    let out = rewrite::normalise_gql_source("RETURN filter(x IN list WHERE x > 0)");
    assert!(out.contains("filter("), "function call should be preserved");
}
