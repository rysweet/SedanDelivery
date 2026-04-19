//! GQL-to-Cypher source-level rewriting.
//!
//! GQL shares most syntax with openCypher but has several keyword differences.
//! This module performs lightweight string-level normalisation so the
//! cypher-core parser can handle the result.
//!
//! The approach is intentionally simple: regex-based keyword substitution.
//! A full GQL-native parser is a future milestone — this gets us running.

use super::{GqlMetadata, PathMode, PathSearch};

/// Keywords that start a new clause — used to delimit expression collection.
const CLAUSE_KEYWORDS: &[&str] = &[
    "MATCH",
    "RETURN",
    "WHERE",
    "FILTER",
    "CREATE",
    "INSERT",
    "DELETE",
    "SET",
    "REMOVE",
    "WITH",
    "UNWIND",
    "LET",
    "FOR",
    "ORDER",
    "LIMIT",
    "SKIP",
    "OFFSET",
    "MANDATORY",
    "OPTIONAL",
];

/// Normalise GQL source into cypher-core–compatible source.
///
/// Substitutions (case-insensitive, whole-word):
/// - `INSERT` → `CREATE`
/// - `FILTER` (as clause, not function) → `WHERE`
/// - `FILTER WHERE` → `WHERE` (strip redundant WHERE after FILTER)
/// - `MANDATORY MATCH` → `MATCH` (metadata captures the flag)
/// - `LET` → `WITH`
/// - `FOR` (as clause, not inside FOREACH) → `UNWIND`
/// - Path mode keywords (`WALK`, `TRAIL`, `SIMPLE`, `ACYCLIC`) → stripped
/// - Path search keywords (`ANY SHORTEST`, `ALL SHORTEST`, `ANY`, `ALL`
///   before a pattern) → stripped
pub fn normalise_gql_source(src: &str) -> String {
    let mut result = String::with_capacity(src.len());
    let tokens = tokenise_for_rewrite(src);

    /// Find the next non-whitespace token starting at index `start`.
    fn next_word_index(tokens: &[RewriteToken<'_>], start: usize) -> Option<usize> {
        tokens[start..]
            .iter()
            .position(|t| !t.text.trim().is_empty())
            .map(|off| start + off)
    }

    let mut i = 0;
    while i < tokens.len() {
        let tok = &tokens[i];
        let upper = tok.text.to_ascii_uppercase();

        match upper.as_str() {
            "INSERT" => {
                result.push_str("CREATE");
                i += 1;
            }
            "LET" => {
                // GQL: LET x = expr  →  Cypher: WITH expr AS x
                // Find variable name, '=', and expression up to next clause keyword
                i += 1;
                // Skip whitespace after LET
                while i < tokens.len() && tokens[i].text.trim().is_empty() {
                    i += 1;
                }
                // Capture variable name
                let var_name = if i < tokens.len() {
                    let name = tokens[i].text;
                    i += 1;
                    name
                } else {
                    // Malformed — fall back
                    result.push_str("WITH");
                    continue;
                };
                // Skip whitespace and '='
                while i < tokens.len() && tokens[i].text.trim().is_empty() {
                    i += 1;
                }
                if i < tokens.len() && tokens[i].text == "=" {
                    i += 1; // skip '='
                }
                // Skip whitespace after '='
                while i < tokens.len() && tokens[i].text.trim().is_empty() {
                    i += 1;
                }
                // Collect expression tokens until next clause keyword or end
                let expr_trimmed = collect_until_clause(&tokens, &mut i);
                // GQL LET adds to scope (doesn't replace), so use WITH *, ...
                result.push_str("WITH *, ");
                result.push_str(&expr_trimmed);
                result.push_str(" AS ");
                result.push_str(var_name);
                result.push(' ');
            }
            "FOR" => {
                // GQL: FOR x IN list  →  Cypher: UNWIND list AS x
                i += 1;
                // Skip whitespace after FOR
                while i < tokens.len() && tokens[i].text.trim().is_empty() {
                    i += 1;
                }
                // Capture variable name
                let var_name = if i < tokens.len() {
                    let name = tokens[i].text;
                    i += 1;
                    name
                } else {
                    result.push_str("UNWIND");
                    continue;
                };
                // Skip whitespace and 'IN'
                while i < tokens.len() && tokens[i].text.trim().is_empty() {
                    i += 1;
                }
                if i < tokens.len() && tokens[i].text.eq_ignore_ascii_case("IN") {
                    i += 1; // skip 'IN'
                }
                // Skip whitespace after 'IN'
                while i < tokens.len() && tokens[i].text.trim().is_empty() {
                    i += 1;
                }
                // Collect list expression tokens until next clause keyword
                let expr_trimmed = collect_until_clause(&tokens, &mut i);
                result.push_str("UNWIND ");
                result.push_str(&expr_trimmed);
                result.push_str(" AS ");
                result.push_str(var_name);
                result.push(' ');
            }
            "MANDATORY" => {
                // MANDATORY MATCH → MATCH (the mandatory flag is in metadata)
                let nw = next_word_index(&tokens, i + 1);
                if nw.is_some_and(|idx| tokens[idx].text.eq_ignore_ascii_case("MATCH")) {
                    // Skip "MANDATORY" and whitespace up to MATCH
                    i += 1;
                    while i < tokens.len() && tokens[i].text.trim().is_empty() {
                        i += 1;
                    }
                    continue;
                }
                result.push_str(tok.text);
                i += 1;
            }
            "FILTER" => {
                // Distinguish FILTER clause from filter() function.
                let next_non_ws = tokens.get(i + 1);
                if next_non_ws.is_none_or(|t| t.text != "(") {
                    result.push_str("WHERE");
                    i += 1;
                    // If "FILTER WHERE", skip the redundant WHERE
                    if let Some(nw_idx) = next_word_index(&tokens, i) {
                        if tokens[nw_idx].text.eq_ignore_ascii_case("WHERE") {
                            // Emit whitespace tokens up to WHERE, then skip WHERE itself
                            while i < nw_idx {
                                result.push_str(tokens[i].text);
                                i += 1;
                            }
                            i += 1; // skip WHERE
                        }
                    }
                } else {
                    result.push_str(tok.text);
                    i += 1;
                }
            }
            // Path mode keywords — strip them (metadata captures the info)
            "WALK" | "TRAIL" | "SIMPLE" | "ACYCLIC" => {
                // Only strip if preceded by MATCH (look backward for context)
                // Simple heuristic: if next non-ws token starts with '(' it's a path pattern
                let nw = next_word_index(&tokens, i + 1);
                let is_path_context = nw.is_some_and(|idx| tokens[idx].text.starts_with('('));
                if is_path_context {
                    // Skip the keyword and any trailing whitespace
                    i += 1;
                    while i < tokens.len() && tokens[i].text.trim().is_empty() {
                        i += 1;
                    }
                } else {
                    result.push_str(tok.text);
                    i += 1;
                }
            }
            // Path search keywords — ANY/ALL possibly followed by SHORTEST
            "ANY" | "ALL" => {
                let nw = next_word_index(&tokens, i + 1);
                let next_is_shortest =
                    nw.is_some_and(|idx| tokens[idx].text.eq_ignore_ascii_case("SHORTEST"));
                if next_is_shortest {
                    // "ANY SHORTEST" or "ALL SHORTEST" — skip both keywords
                    i += 1;
                    while i < tokens.len() && tokens[i].text.trim().is_empty() {
                        i += 1;
                    }
                    // Now i points at SHORTEST — skip it
                    i += 1;
                    // Skip trailing whitespace
                    while i < tokens.len() && tokens[i].text.trim().is_empty() {
                        i += 1;
                    }
                } else {
                    // Standalone ANY/ALL before a pattern — check if next is '('
                    let nw2 = next_word_index(&tokens, i + 1);
                    let is_path_context = nw2.is_some_and(|idx| tokens[idx].text.starts_with('('));
                    if is_path_context {
                        i += 1;
                        while i < tokens.len() && tokens[i].text.trim().is_empty() {
                            i += 1;
                        }
                    } else {
                        // Not a path search context — keep as-is (e.g. all() function)
                        result.push_str(tok.text);
                        i += 1;
                    }
                }
            }
            _ => {
                result.push_str(tok.text);
                i += 1;
            }
        }
    }

    result
}

/// Collect tokens until a clause keyword is encountered, returning the trimmed expression.
fn collect_until_clause(tokens: &[RewriteToken<'_>], i: &mut usize) -> String {
    let mut expr_tokens = Vec::new();
    while *i < tokens.len() {
        if is_clause_keyword(tokens[*i].text) {
            break;
        }
        expr_tokens.push(tokens[*i].text);
        *i += 1;
    }
    let expr_str: String = expr_tokens.concat();
    expr_str.trim().to_owned()
}

/// Check if a token is a clause keyword (case-insensitive) without allocating.
fn is_clause_keyword(text: &str) -> bool {
    CLAUSE_KEYWORDS
        .iter()
        .any(|kw| text.eq_ignore_ascii_case(kw))
}

/// Extract GQL-specific metadata from the source before normalisation.
///
/// Uses case-insensitive matching on the original source to avoid
/// allocating a full uppercase copy.
pub fn extract_gql_metadata(src: &str) -> GqlMetadata {
    let mut meta = GqlMetadata::default();

    // Check for MANDATORY MATCH
    if contains_word_ci(src, "MANDATORY") && contains_word_ci(src, "MATCH") {
        meta.mandatory_match = true;
    }

    // Check for path modes (e.g. MATCH TRAIL, MATCH WALK)
    for (keyword, mode) in &[
        ("WALK", PathMode::Walk),
        ("TRAIL", PathMode::Trail),
        ("SIMPLE", PathMode::Simple),
        ("ACYCLIC", PathMode::Acyclic),
    ] {
        if contains_word_ci(src, keyword) {
            meta.path_mode = Some(*mode);
            break;
        }
    }

    // Check for path search modifiers (order matters: check compound first)
    if contains_phrase_ci(src, "ANY SHORTEST") {
        meta.path_search = Some(PathSearch::AnyShortest);
    } else if contains_phrase_ci(src, "ALL SHORTEST") {
        meta.path_search = Some(PathSearch::AllShortest);
    } else if contains_match_modifier_ci(src, "ANY") {
        meta.path_search = Some(PathSearch::Any);
    } else if contains_match_modifier_ci(src, "ALL") {
        meta.path_search = Some(PathSearch::All);
    }

    meta
}

/// Case-insensitive check if a keyword appears as a MATCH modifier
/// (i.e. "MATCH ANY (" pattern). Does not allocate.
fn contains_match_modifier_ci(src: &str, keyword: &str) -> bool {
    // Find "MATCH" case-insensitively, then check if keyword follows
    let src_bytes = src.as_bytes();
    let match_kw = b"MATCH";
    if src_bytes.len() < match_kw.len() {
        return false;
    }
    for start in 0..=(src_bytes.len() - match_kw.len()) {
        if src_bytes[start..start + 5].eq_ignore_ascii_case(match_kw) {
            // Word boundary check before MATCH to avoid matching e.g. "REMATCH".
            let before_ok = start == 0 || !src_bytes[start - 1].is_ascii_alphanumeric();
            if !before_ok {
                continue;
            }
            // Word boundary check after MATCH to avoid matching e.g. "MATCHANY".
            let pos_after_match = start + 5;
            if pos_after_match < src_bytes.len()
                && src_bytes[pos_after_match].is_ascii_alphanumeric()
            {
                continue;
            }
            let after_match = &src[start + 5..];
            let trimmed = after_match.trim_start();
            if trimmed.len() >= keyword.len()
                && trimmed.as_bytes()[..keyword.len()].eq_ignore_ascii_case(keyword.as_bytes())
            {
                let after_kw = &trimmed[keyword.len()..];
                let next_char = after_kw.as_bytes().first();
                if next_char.is_none_or(|c| !c.is_ascii_alphanumeric()) {
                    return true;
                }
            }
        }
    }
    false
}

/// Case-insensitive word-boundary check without allocating.
fn contains_word_ci(haystack: &str, word: &str) -> bool {
    let bytes = haystack.as_bytes();
    let wbytes = word.as_bytes();
    if wbytes.len() > bytes.len() {
        return false;
    }
    for start in 0..=(bytes.len() - wbytes.len()) {
        if bytes[start..start + wbytes.len()].eq_ignore_ascii_case(wbytes) {
            let before_ok = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
            let after_ok = start + wbytes.len() == bytes.len()
                || !bytes[start + wbytes.len()].is_ascii_alphanumeric();
            if before_ok && after_ok {
                return true;
            }
        }
    }
    false
}

/// Case-insensitive phrase search (words separated by whitespace).
fn contains_phrase_ci(haystack: &str, phrase: &str) -> bool {
    // Split phrase into words and search for them in sequence
    let phrase_words: Vec<&str> = phrase.split_whitespace().collect();
    if phrase_words.is_empty() {
        return false;
    }
    // Simple approach: find first word, then check if second follows
    let bytes = haystack.as_bytes();
    let first = phrase_words[0].as_bytes();
    if first.len() > bytes.len() {
        return false;
    }
    for start in 0..=(bytes.len() - first.len()) {
        if bytes[start..start + first.len()].eq_ignore_ascii_case(first) {
            // Word boundary check before the first word so we don't match
            // e.g. "ANY SHORTEST" inside "MANY SHORTEST".
            let before_ok = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
            if !before_ok {
                continue;
            }
            // Check remaining words follow (with whitespace between)
            let mut pos = start + first.len();
            // The character immediately after the first word must also be a
            // word boundary so that the first phrase word is matched as a
            // whole token (e.g. "ANY" not "ANYTHING").
            if pos < bytes.len() && bytes[pos].is_ascii_alphanumeric() {
                continue;
            }
            let mut matched = true;
            for word in &phrase_words[1..] {
                // Skip whitespace
                while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
                    pos += 1;
                }
                let wb = word.as_bytes();
                if pos + wb.len() > bytes.len()
                    || !bytes[pos..pos + wb.len()].eq_ignore_ascii_case(wb)
                {
                    matched = false;
                    break;
                }
                pos += wb.len();
            }
            if matched {
                // Trailing word boundary check on the last phrase word so that
                // "ALL SHORTEST" doesn't match inside "ALL SHORTESTPATH".
                let after_ok = pos == bytes.len() || !bytes[pos].is_ascii_alphanumeric();
                if after_ok {
                    return true;
                }
            }
        }
    }
    false
}

/// Minimal token for rewriting: preserves whitespace and punctuation as-is.
struct RewriteToken<'a> {
    text: &'a str,
}

/// Split source into word tokens and non-word tokens for rewriting.
fn tokenise_for_rewrite(src: &str) -> Vec<RewriteToken<'_>> {
    // Average token length ~4 chars gives a reasonable initial capacity.
    let mut tokens = Vec::with_capacity(src.len() / 4 + 1);
    let bytes = src.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' {
            // Word token
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            tokens.push(RewriteToken {
                text: &src[start..i],
            });
        } else {
            // Non-word token (whitespace, punctuation, etc.)
            let start = i;
            // Group consecutive non-word, non-alphanumeric chars — but keep
            // single chars for punctuation accuracy
            i += 1;
            // For whitespace, group it together
            if bytes[start].is_ascii_whitespace() {
                while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
            }
            tokens.push(RewriteToken {
                text: &src[start..i],
            });
        }
    }

    tokens
}

// Full unit tests for this module are in tests/unit_parser.rs and tests/fuzz_rewrite.rs.

#[cfg(test)]
mod word_boundary_regression {
    use super::{contains_match_modifier_ci, contains_phrase_ci};

    // Regression for the word-boundary bug in `contains_phrase_ci`: matching a
    // phrase like "ANY SHORTEST" must not succeed inside a longer identifier
    // such as "MANY SHORTEST" or "ALL SHORTESTPATH".
    #[test]
    fn phrase_ci_requires_leading_word_boundary() {
        assert!(!contains_phrase_ci("MATCH MANY SHORTEST", "ANY SHORTEST"));
        assert!(!contains_phrase_ci("MATCH BALL SHORTEST", "ALL SHORTEST"));
        assert!(contains_phrase_ci("MATCH ANY SHORTEST", "ANY SHORTEST"));
        assert!(contains_phrase_ci("match any shortest", "ANY SHORTEST"));
    }

    #[test]
    fn phrase_ci_requires_trailing_word_boundary() {
        assert!(!contains_phrase_ci(
            "MATCH ALL SHORTESTPATH",
            "ALL SHORTEST"
        ));
        assert!(contains_phrase_ci("MATCH ALL SHORTEST (p)", "ALL SHORTEST"));
    }

    // Regression for the word-boundary bug in `contains_match_modifier_ci`:
    // an identifier like "REMATCH" must not satisfy the "MATCH ..." prefix.
    #[test]
    fn match_modifier_ci_requires_leading_word_boundary() {
        assert!(!contains_match_modifier_ci("REMATCH ANY (p)", "ANY"));
        assert!(!contains_match_modifier_ci("PREMATCH ALL (p)", "ALL"));
        assert!(contains_match_modifier_ci("MATCH ANY (p)", "ANY"));
        assert!(contains_match_modifier_ci("match all (p)", "ALL"));
    }

    // Regression for cycle-2 finding: "MATCH" without trailing whitespace must
    // not match the modifier even if the next chars look like the keyword.
    #[test]
    fn match_modifier_ci_requires_trailing_word_boundary_on_match() {
        assert!(!contains_match_modifier_ci("MATCHANY (p)", "ANY"));
        assert!(!contains_match_modifier_ci("MATCHALL (p)", "ALL"));
        assert!(!contains_match_modifier_ci("MATCHED ANY (p)", "ANY"));
    }
}

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
