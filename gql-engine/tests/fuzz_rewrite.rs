//! Proptest-based fuzz tests for the GQL rewrite module.
//!
//! Verifies that the normaliser never panics on arbitrary input and that
//! key invariants hold across random query strings.

use gql_engine::parser::rewrite;
use proptest::prelude::*;

// ── Strategy: random ASCII strings that look vaguely like queries ─────────────

fn gql_keyword() -> impl Strategy<Value = &'static str> {
    prop_oneof![
        Just("INSERT"),
        Just("FILTER"),
        Just("MANDATORY"),
        Just("MATCH"),
        Just("RETURN"),
        Just("LET"),
        Just("FOR"),
        Just("IN"),
        Just("WALK"),
        Just("TRAIL"),
        Just("SIMPLE"),
        Just("ACYCLIC"),
        Just("ANY"),
        Just("ALL"),
        Just("SHORTEST"),
        Just("CREATE"),
        Just("WHERE"),
        Just("UNWIND"),
        Just("WITH"),
        Just("ORDER"),
        Just("LIMIT"),
        Just("SKIP"),
        Just("DELETE"),
        Just("SET"),
        Just("REMOVE"),
    ]
}

fn gql_fragment() -> impl Strategy<Value = String> {
    prop_oneof![
        gql_keyword().prop_map(|k| k.to_string()),
        "[a-zA-Z_][a-zA-Z0-9_]{0,8}".prop_map(|s| s),
        Just("(".to_string()),
        Just(")".to_string()),
        Just("[".to_string()),
        Just("]".to_string()),
        Just("{".to_string()),
        Just("}".to_string()),
        Just("'text'".to_string()),
        Just("123".to_string()),
        Just(">".to_string()),
        Just("<".to_string()),
        Just("=".to_string()),
        Just(",".to_string()),
        Just(".".to_string()),
        Just(":".to_string()),
        Just("-".to_string()),
        Just("*".to_string()),
        Just(" ".to_string()),
        Just("  ".to_string()),
        Just("\n".to_string()),
        Just("\t".to_string()),
    ]
}

fn random_gql_source() -> impl Strategy<Value = String> {
    prop::collection::vec(gql_fragment(), 1..30).prop_map(|fragments| fragments.join(""))
}

// ── Fuzz: normalise_gql_source never panics ──────────────────────────────────

proptest! {
    #[test]
    fn normalise_never_panics(src in random_gql_source()) {
        let _ = rewrite::normalise_gql_source(&src);
    }

    #[test]
    fn extract_metadata_never_panics(src in random_gql_source()) {
        let _ = rewrite::extract_gql_metadata(&src);
    }

    #[test]
    fn normalise_never_panics_on_arbitrary_ascii(src in "[\\x20-\\x7e]{0,200}") {
        let _ = rewrite::normalise_gql_source(&src);
    }

    #[test]
    fn extract_metadata_never_panics_on_arbitrary_ascii(src in "[\\x20-\\x7e]{0,200}") {
        let _ = rewrite::extract_gql_metadata(&src);
    }
}

// ── Invariant: INSERT always becomes CREATE ──────────────────────────────────

proptest! {
    #[test]
    fn insert_always_becomes_create(
        suffix in "[a-zA-Z0-9 (){}:',._]{0,50}"
    ) {
        let src = format!("INSERT {suffix}");
        let out = rewrite::normalise_gql_source(&src);
        prop_assert!(
            out.starts_with("CREATE"),
            "INSERT should become CREATE, got: {out}"
        );
    }
}

// ── Invariant: MANDATORY is stripped from normalised output ───────────────────

proptest! {
    #[test]
    fn mandatory_stripped_when_followed_by_match(
        suffix in "[a-zA-Z0-9 (){}:',._]{0,50}"
    ) {
        let src = format!("MANDATORY MATCH {suffix}");
        let out = rewrite::normalise_gql_source(&src);
        prop_assert!(
            !out.contains("MANDATORY"),
            "MANDATORY should be stripped, got: {out}"
        );
    }
}

// ── Invariant: FILTER clause becomes WHERE ───────────────────────────────────

proptest! {
    #[test]
    fn filter_clause_becomes_where(
        predicate in "[a-zA-Z0-9._> ]{1,30}"
    ) {
        let src = format!("MATCH (n) FILTER {predicate} RETURN n");
        let out = rewrite::normalise_gql_source(&src);
        prop_assert!(
            out.contains("WHERE"),
            "FILTER clause should become WHERE, got: {out}"
        );
    }
}

// ── Invariant: normalisation output length is bounded ────────────────────────

proptest! {
    #[test]
    fn normalised_output_bounded(src in random_gql_source()) {
        let out = rewrite::normalise_gql_source(&src);
        // Output may grow (e.g. LET→WITH *, expr AS x) but should be bounded
        // relative to input. 3x is a generous upper bound.
        prop_assert!(
            out.len() <= src.len() * 3 + 100,
            "output unexpectedly large: input {} bytes, output {} bytes",
            src.len(),
            out.len()
        );
    }
}

// ── Invariant: standard Cypher keywords preserved ────────────────────────────

proptest! {
    #[test]
    fn match_keyword_always_present_after_normalise(
        pattern in "\\([a-z]\\)"
    ) {
        let src = format!("MATCH {pattern} RETURN n");
        let out = rewrite::normalise_gql_source(&src);
        prop_assert!(
            out.contains("MATCH"),
            "MATCH should be preserved, got: {out}"
        );
    }
}
