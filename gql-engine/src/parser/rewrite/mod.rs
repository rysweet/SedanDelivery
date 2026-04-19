//! GQL-to-Cypher source-level rewriting.
//!
//! GQL shares most syntax with openCypher but has several keyword differences.
//! This module performs lightweight string-level normalisation so the
//! cypher-core parser can handle the result.
//!
//! The approach is intentionally simple: regex-free keyword substitution
//! over a tokenised view of the source. A full GQL-native parser is a
//! future milestone — this gets us running.
//!
//! Submodules:
//! - [`tokens`]: the rewriter's token model and tokeniser.
//! - [`normalise`]: the keyword-substitution pass producing Cypher source.
//! - [`metadata`]: pre-rewrite metadata extraction (mandatory match,
//!   path mode, path search).
//! - [`text_search`]: case-insensitive word/phrase search helpers.

mod metadata;
mod normalise;
mod text_search;
mod tokens;

pub use metadata::extract_gql_metadata;
pub use normalise::normalise_gql_source;

// Full unit tests for this module are in tests/unit_parser.rs and tests/fuzz_rewrite.rs.

#[cfg(test)]
mod rewrite_coverage {
    use super::{extract_gql_metadata, normalise_gql_source};
    use crate::parser::{PathMode, PathSearch};

    // ── normalise_gql_source: branch coverage ─────────────────────────────

    #[test]
    fn insert_keyword_lowercase_still_rewrites_to_create() {
        let out = normalise_gql_source("insert (n:Foo)");
        assert!(out.to_uppercase().contains("CREATE"), "got: {out}");
    }

    #[test]
    fn filter_clause_becomes_where() {
        let out = normalise_gql_source("MATCH (n) FILTER n.x > 1 RETURN n");
        assert!(out.contains("WHERE"));
        assert!(!out.split_whitespace().any(|t| t == "FILTER"));
    }

    #[test]
    fn filter_function_call_is_preserved() {
        // FILTER followed immediately by '(' must NOT be rewritten.
        let out = normalise_gql_source("RETURN filter(x IN [1,2] WHERE x > 0)");
        assert!(
            out.contains("filter("),
            "FILTER( should be preserved: {out}"
        );
    }

    #[test]
    fn filter_where_redundancy_is_collapsed() {
        let out = normalise_gql_source("MATCH (n) FILTER WHERE n.x > 1 RETURN n");
        let upper = out.to_uppercase();
        assert_eq!(upper.matches("WHERE").count(), 1, "got: {out}");
    }

    #[test]
    fn mandatory_without_match_is_kept_as_word() {
        let out = normalise_gql_source("RETURN MANDATORY");
        assert!(out.to_uppercase().contains("MANDATORY"), "got: {out}");
    }

    #[test]
    fn mandatory_match_keyword_is_stripped() {
        let out = normalise_gql_source("MANDATORY MATCH (n) RETURN n");
        let upper = out.to_uppercase();
        assert!(!upper.contains("MANDATORY"), "got: {out}");
        assert!(upper.contains("MATCH"));
    }

    #[test]
    fn for_keyword_rewrites_to_unwind() {
        let out = normalise_gql_source("FOR x IN [1,2,3] RETURN x");
        let upper = out.to_uppercase();
        assert!(upper.contains("UNWIND"), "got: {out}");
        assert!(upper.contains(" AS X"));
    }

    #[test]
    fn for_keyword_at_eof_falls_back_to_unwind_keyword() {
        let out = normalise_gql_source("FOR");
        assert!(out.to_uppercase().contains("UNWIND"), "got: {out}");
    }

    #[test]
    fn let_keyword_rewrites_to_with() {
        let out = normalise_gql_source("MATCH (n) LET y = n.age RETURN n");
        let upper = out.to_uppercase();
        assert!(upper.contains("WITH"), "got: {out}");
        assert!(upper.contains(" AS Y"));
    }

    #[test]
    fn let_keyword_at_eof_falls_back_to_with_keyword() {
        let out = normalise_gql_source("LET");
        assert!(out.to_uppercase().contains("WITH"), "got: {out}");
    }

    #[test]
    fn walk_path_mode_is_stripped_in_pattern_context() {
        let out = normalise_gql_source("MATCH WALK (a)-[:KNOWS]->(b) RETURN a");
        assert!(!out.to_uppercase().contains("WALK"), "got: {out}");
    }

    #[test]
    fn trail_path_mode_is_stripped_in_pattern_context() {
        let out = normalise_gql_source("MATCH TRAIL (a)-[:R*]->(b) RETURN a");
        assert!(!out.to_uppercase().contains("TRAIL"), "got: {out}");
    }

    #[test]
    fn simple_path_mode_is_stripped_in_pattern_context() {
        let out = normalise_gql_source("MATCH SIMPLE (a)-->(b) RETURN a");
        assert!(!out.to_uppercase().contains("SIMPLE"), "got: {out}");
    }

    #[test]
    fn acyclic_path_mode_is_stripped_in_pattern_context() {
        let out = normalise_gql_source("MATCH ACYCLIC (a)-->(b) RETURN a");
        assert!(!out.to_uppercase().contains("ACYCLIC"), "got: {out}");
    }

    #[test]
    fn walk_keyword_outside_path_context_is_kept() {
        let out = normalise_gql_source("RETURN WALK");
        assert!(out.to_uppercase().contains("WALK"), "got: {out}");
    }

    #[test]
    fn any_shortest_modifier_is_stripped() {
        let out = normalise_gql_source("MATCH ANY SHORTEST (a)-[:R*]->(b) RETURN a");
        assert!(!out.to_uppercase().contains("SHORTEST"), "got: {out}");
    }

    #[test]
    fn all_shortest_modifier_is_stripped() {
        let out = normalise_gql_source("MATCH ALL SHORTEST (a)-[:R*]->(b) RETURN a");
        assert!(!out.to_uppercase().contains("SHORTEST"), "got: {out}");
    }

    #[test]
    fn standalone_any_before_pattern_is_stripped() {
        let out = normalise_gql_source("MATCH ANY (a)-->(b) RETURN a");
        let upper = out.to_uppercase();
        let has_standalone_any = upper
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|w| w == "ANY");
        assert!(!has_standalone_any, "got: {out}");
    }

    #[test]
    fn standalone_all_before_pattern_is_stripped() {
        let out = normalise_gql_source("MATCH ALL (a)-->(b) RETURN a");
        let upper = out.to_uppercase();
        let has_standalone_all = upper
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|w| w == "ALL");
        assert!(!has_standalone_all, "got: {out}");
    }

    #[test]
    fn any_followed_by_non_pattern_word_is_preserved() {
        let out = normalise_gql_source("RETURN ANY thing");
        assert!(out.to_uppercase().contains("ANY"), "got: {out}");
    }

    #[test]
    fn all_followed_by_non_pattern_word_is_preserved() {
        let out = normalise_gql_source("RETURN ALL things");
        assert!(out.to_uppercase().contains("ALL"), "got: {out}");
    }

    #[test]
    fn empty_source_returns_empty_string() {
        assert_eq!(normalise_gql_source(""), "");
    }

    #[test]
    fn whitespace_only_source_is_preserved() {
        let out = normalise_gql_source("   \n\t  ");
        assert_eq!(out, "   \n\t  ");
    }

    #[test]
    fn punctuation_only_source_is_preserved() {
        let out = normalise_gql_source("(){}[];,");
        assert_eq!(out, "(){}[];,");
    }

    #[test]
    fn unrelated_keywords_pass_through_unchanged() {
        let out = normalise_gql_source("MATCH (n) RETURN n.name ORDER BY n.name LIMIT 10");
        assert!(out.contains("ORDER"));
        assert!(out.contains("LIMIT"));
        assert!(out.contains("RETURN"));
    }

    // ── extract_gql_metadata branches ─────────────────────────────────────

    #[test]
    fn metadata_default_is_empty() {
        let m = extract_gql_metadata("MATCH (n) RETURN n");
        assert!(!m.mandatory_match);
        assert!(m.path_mode.is_none());
        assert!(m.path_search.is_none());
    }

    #[test]
    fn metadata_detects_mandatory_match() {
        let m = extract_gql_metadata("MANDATORY MATCH (n) RETURN n");
        assert!(m.mandatory_match);
    }

    #[test]
    fn metadata_detects_walk_path_mode() {
        let m = extract_gql_metadata("MATCH WALK (a)-->(b) RETURN a");
        assert_eq!(m.path_mode, Some(PathMode::Walk));
    }

    #[test]
    fn metadata_detects_trail_path_mode() {
        let m = extract_gql_metadata("MATCH TRAIL (a)-->(b) RETURN a");
        assert_eq!(m.path_mode, Some(PathMode::Trail));
    }

    #[test]
    fn metadata_detects_simple_path_mode() {
        let m = extract_gql_metadata("MATCH SIMPLE (a)-->(b) RETURN a");
        assert_eq!(m.path_mode, Some(PathMode::Simple));
    }

    #[test]
    fn metadata_detects_acyclic_path_mode() {
        let m = extract_gql_metadata("MATCH ACYCLIC (a)-->(b) RETURN a");
        assert_eq!(m.path_mode, Some(PathMode::Acyclic));
    }

    #[test]
    fn metadata_detects_any_shortest_search() {
        let m = extract_gql_metadata("MATCH ANY SHORTEST (a)-->(b) RETURN a");
        assert_eq!(m.path_search, Some(PathSearch::AnyShortest));
    }

    #[test]
    fn metadata_detects_all_shortest_search() {
        let m = extract_gql_metadata("MATCH ALL SHORTEST (a)-->(b) RETURN a");
        assert_eq!(m.path_search, Some(PathSearch::AllShortest));
    }

    #[test]
    fn metadata_detects_any_modifier() {
        let m = extract_gql_metadata("MATCH ANY (a)-->(b) RETURN a");
        assert_eq!(m.path_search, Some(PathSearch::Any));
    }

    #[test]
    fn metadata_detects_all_modifier() {
        let m = extract_gql_metadata("MATCH ALL (a)-->(b) RETURN a");
        assert_eq!(m.path_search, Some(PathSearch::All));
    }

    #[test]
    fn metadata_word_boundary_avoids_false_positive_walk_in_identifier() {
        let m = extract_gql_metadata("MATCH (n) WHERE n.style = 'WALKING' RETURN n");
        assert!(m.path_mode.is_none(), "false positive: {:?}", m.path_mode);
    }

    #[test]
    fn metadata_word_boundary_avoids_false_positive_simple_in_identifier() {
        let m = extract_gql_metadata("MATCH (n) WHERE n.kind = 'SIMPLEX' RETURN n");
        assert!(m.path_mode.is_none(), "false positive: {:?}", m.path_mode);
    }

    #[test]
    fn metadata_short_source_does_not_panic() {
        assert!(!extract_gql_metadata("").mandatory_match);
        assert!(!extract_gql_metadata("X").mandatory_match);
    }

    // ── enum traits ───────────────────────────────────────────────────────

    #[test]
    fn path_mode_variants_are_distinct_and_copy() {
        assert_ne!(PathMode::Walk, PathMode::Trail);
        assert_ne!(PathMode::Simple, PathMode::Acyclic);
        let _ = format!("{:?}", PathMode::Walk);
        let cloned = PathMode::Trail;
        #[allow(clippy::clone_on_copy)]
        let c2 = cloned.clone();
        assert_eq!(c2, PathMode::Trail);
    }

    #[test]
    fn path_search_variants_are_distinct_and_copy() {
        assert_ne!(PathSearch::Any, PathSearch::All);
        assert_ne!(PathSearch::AnyShortest, PathSearch::AllShortest);
        let _ = format!("{:?}", PathSearch::AnyShortest);
        let cloned = PathSearch::All;
        #[allow(clippy::clone_on_copy)]
        let c2 = cloned.clone();
        assert_eq!(c2, PathSearch::All);
    }

    // ── parse_gql + GqlParseError display ─────────────────────────────────

    #[test]
    fn parse_gql_returns_metadata_alongside_query() {
        let q = crate::parser::parse_gql("MANDATORY MATCH (n) RETURN n").unwrap();
        assert!(q.metadata.mandatory_match);
    }

    #[test]
    fn parse_gql_error_display_does_not_panic() {
        let err = crate::parser::parse_gql("@@@").unwrap_err();
        assert!(!format!("{err}").is_empty());
        let _ = format!("{err:?}");
    }

    #[test]
    fn parse_gql_unsupported_error_display() {
        let err = crate::parser::GqlParseError::Unsupported("MATCH ... QUANTIFY ...".into());
        let s = format!("{err}");
        assert!(s.contains("unsupported"));
        assert!(s.contains("QUANTIFY"));
    }

    #[test]
    fn parse_gql_semantic_error_display() {
        let err = crate::parser::GqlParseError::Semantic("isomorphism violation".into());
        let s = format!("{err}");
        assert!(s.contains("semantic"));
        assert!(s.contains("isomorphism"));
    }
}
