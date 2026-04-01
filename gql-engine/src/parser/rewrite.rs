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
        let tok_upper = tokens[*i].text.to_ascii_uppercase();
        if CLAUSE_KEYWORDS.contains(&tok_upper.as_str()) {
            break;
        }
        expr_tokens.push(tokens[*i].text);
        *i += 1;
    }
    let expr_str: String = expr_tokens.concat();
    expr_str.trim().to_owned()
}

/// Extract GQL-specific metadata from the source before normalisation.
pub fn extract_gql_metadata(src: &str) -> GqlMetadata {
    let upper = src.to_ascii_uppercase();
    let mut meta = GqlMetadata::default();

    // Check for MANDATORY MATCH
    if upper.contains("MANDATORY") && upper.contains("MATCH") {
        meta.mandatory_match = true;
    }

    // Check for path modes (e.g. MATCH TRAIL, MATCH WALK)
    for (keyword, mode) in &[
        ("WALK", PathMode::Walk),
        ("TRAIL", PathMode::Trail),
        ("SIMPLE", PathMode::Simple),
        ("ACYCLIC", PathMode::Acyclic),
    ] {
        if contains_word(&upper, keyword) {
            meta.path_mode = Some(*mode);
            break;
        }
    }

    // Check for path search modifiers (order matters: check compound first)
    if upper.contains("ANY SHORTEST") || upper.contains("ANY  SHORTEST") {
        meta.path_search = Some(PathSearch::AnyShortest);
    } else if upper.contains("ALL SHORTEST") || upper.contains("ALL  SHORTEST") {
        meta.path_search = Some(PathSearch::AllShortest);
    } else if contains_match_modifier(&upper, "ANY") {
        meta.path_search = Some(PathSearch::Any);
    } else if contains_match_modifier(&upper, "ALL") {
        meta.path_search = Some(PathSearch::All);
    }

    meta
}

/// Check if a keyword appears as a MATCH modifier (i.e. "MATCH ANY (" pattern).
fn contains_match_modifier(upper: &str, keyword: &str) -> bool {
    // Look for "MATCH <ws> keyword <ws> (" pattern
    if let Some(match_pos) = upper.find("MATCH") {
        let after_match = &upper[match_pos + 5..];
        let trimmed = after_match.trim_start();
        if let Some(after_kw) = trimmed.strip_prefix(keyword) {
            let next_char = after_kw.as_bytes().first();
            // Must be followed by whitespace or '(' (not alphanumeric)
            if next_char.is_none_or(|c| !c.is_ascii_alphanumeric()) {
                return true;
            }
        }
    }
    false
}

/// Simple word-boundary check: keyword appears surrounded by non-alphanumeric chars.
fn contains_word(haystack: &str, word: &str) -> bool {
    let bytes = haystack.as_bytes();
    let wbytes = word.as_bytes();
    if wbytes.len() > bytes.len() {
        return false;
    }
    for start in 0..=(bytes.len() - wbytes.len()) {
        if &bytes[start..start + wbytes.len()] == wbytes {
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

/// Minimal token for rewriting: preserves whitespace and punctuation as-is.
struct RewriteToken<'a> {
    text: &'a str,
}

/// Split source into word tokens and non-word tokens for rewriting.
fn tokenise_for_rewrite(src: &str) -> Vec<RewriteToken<'_>> {
    let mut tokens = Vec::new();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalise_insert_to_create() {
        let src = "INSERT (n:Person {name: 'Alice'})";
        let out = normalise_gql_source(src);
        assert!(out.starts_with("CREATE"));
        assert!(out.contains("Person"));
    }

    #[test]
    fn normalise_mandatory_match() {
        let src = "MANDATORY MATCH (n) RETURN n";
        let out = normalise_gql_source(src);
        assert!(!out.contains("MANDATORY"));
        assert!(out.contains("MATCH"));
        let meta = extract_gql_metadata(src);
        assert!(meta.mandatory_match);
    }

    #[test]
    fn normalise_filter_clause() {
        let src = "MATCH (n) FILTER n.age > 21 RETURN n";
        let out = normalise_gql_source(src);
        assert!(out.contains("WHERE"));
        assert!(!out.contains("FILTER"));
    }

    #[test]
    fn normalise_filter_function_preserved() {
        let src = "RETURN filter(x IN [1,2,3] WHERE x > 1)";
        let out = normalise_gql_source(src);
        assert!(out.contains("filter"));
    }

    #[test]
    fn extract_path_mode_trail() {
        let src = "MATCH TRAIL (a)-[*]->(b) RETURN a, b";
        let meta = extract_gql_metadata(src);
        assert_eq!(meta.path_mode, Some(PathMode::Trail));
    }

    #[test]
    fn extract_path_search_any_shortest() {
        let src = "MATCH ANY SHORTEST (a)-[*]->(b) RETURN a, b";
        let meta = extract_gql_metadata(src);
        assert_eq!(meta.path_search, Some(PathSearch::AnyShortest));
    }

    #[test]
    fn plain_cypher_unchanged() {
        let src = "MATCH (n:Person) WHERE n.age > 21 RETURN n";
        let out = normalise_gql_source(src);
        assert_eq!(out, src);
    }

    // --- TDD: New GQL keyword rewrite tests ---

    #[test]
    fn normalise_let_to_with() {
        // GQL uses LET for computed bindings; cypher-core uses WITH *, expr AS x
        let src = "MATCH (n:Person) LET fullName = n.firstName RETURN fullName";
        let out = normalise_gql_source(src);
        assert!(out.contains("WITH *,"), "expected LET→WITH *, got: {out}");
        assert!(out.contains("AS fullName"), "expected AS alias, got: {out}");
        assert!(!contains_word(&out.to_ascii_uppercase(), "LET"));
    }

    #[test]
    fn normalise_for_to_unwind() {
        // GQL uses FOR instead of UNWIND
        let src = "FOR x IN [1, 2, 3] RETURN x";
        let out = normalise_gql_source(src);
        assert!(out.contains("UNWIND"), "expected FOR→UNWIND, got: {out}");
        assert!(!contains_word(&out.to_ascii_uppercase(), "FOR"));
    }

    #[test]
    fn normalise_for_preserves_foreach() {
        // FOR inside FOREACH should not be double-rewritten
        let src = "MATCH (n) RETURN n FOR n IN nodes(p)";
        let out = normalise_gql_source(src);
        // The standalone FOR should become UNWIND
        assert!(out.contains("UNWIND"), "expected FOR→UNWIND, got: {out}");
    }

    #[test]
    fn normalise_insert_case_insensitive() {
        let src = "insert (n:Person {name: 'Alice'})";
        let out = normalise_gql_source(src);
        assert!(
            out.starts_with("CREATE"),
            "expected insert→CREATE, got: {out}"
        );
    }

    #[test]
    fn normalise_filter_case_insensitive() {
        let src = "MATCH (n) filter n.age > 21 RETURN n";
        let out = normalise_gql_source(src);
        assert!(out.contains("WHERE"), "expected filter→WHERE, got: {out}");
    }

    #[test]
    fn normalise_filter_where_variant() {
        // GQL allows "FILTER WHERE" as a synonym for "FILTER"
        let src = "MATCH (n:Person) FILTER WHERE n.age > 21 RETURN n";
        let out = normalise_gql_source(src);
        // Should produce "WHERE n.age > 21" (strip the redundant WHERE after FILTER)
        assert!(
            out.contains("WHERE"),
            "expected WHERE in output, got: {out}"
        );
        // Should not contain double WHERE
        let where_count = out.matches("WHERE").count();
        assert_eq!(
            where_count, 1,
            "expected single WHERE, got {where_count} in: {out}"
        );
    }

    #[test]
    fn normalise_multiple_gql_keywords() {
        // Query using multiple GQL-specific keywords at once
        let src = "INSERT (n:Person {name: 'Alice'}) LET x = n.name FILTER x = 'Alice' RETURN x";
        let out = normalise_gql_source(src);
        assert!(out.contains("CREATE"), "INSERT→CREATE");
        assert!(out.contains("WITH"), "LET→WITH");
        assert!(out.contains("WHERE"), "FILTER→WHERE");
    }

    // --- TDD: Path mode metadata extraction ---

    #[test]
    fn extract_path_mode_walk() {
        let src = "MATCH WALK (a)-[*]->(b) RETURN a";
        let meta = extract_gql_metadata(src);
        assert_eq!(meta.path_mode, Some(PathMode::Walk));
    }

    #[test]
    fn extract_path_mode_simple() {
        let src = "MATCH SIMPLE (a)-[*]->(b) RETURN a";
        let meta = extract_gql_metadata(src);
        assert_eq!(meta.path_mode, Some(PathMode::Simple));
    }

    #[test]
    fn extract_path_mode_acyclic() {
        let src = "MATCH ACYCLIC (a)-[*]->(b) RETURN a";
        let meta = extract_gql_metadata(src);
        assert_eq!(meta.path_mode, Some(PathMode::Acyclic));
    }

    #[test]
    fn extract_path_search_all_shortest() {
        let src = "MATCH ALL SHORTEST (a)-[*]->(b) RETURN a";
        let meta = extract_gql_metadata(src);
        assert_eq!(meta.path_search, Some(PathSearch::AllShortest));
    }

    #[test]
    fn extract_path_search_any() {
        let src = "MATCH ANY (a)-[*]->(b) RETURN a";
        let meta = extract_gql_metadata(src);
        assert_eq!(meta.path_search, Some(PathSearch::Any));
    }

    #[test]
    fn extract_path_search_all() {
        let src = "MATCH ALL (a)-[*]->(b) RETURN a";
        let meta = extract_gql_metadata(src);
        assert_eq!(meta.path_search, Some(PathSearch::All));
    }

    #[test]
    fn no_path_mode_for_plain_match() {
        let src = "MATCH (a)-[:KNOWS]->(b) RETURN a";
        let meta = extract_gql_metadata(src);
        assert_eq!(meta.path_mode, None);
        assert_eq!(meta.path_search, None);
    }

    #[test]
    fn mandatory_match_case_insensitive() {
        let src = "mandatory match (n) RETURN n";
        let meta = extract_gql_metadata(src);
        assert!(meta.mandatory_match);
    }

    #[test]
    fn normalise_path_mode_keywords_stripped() {
        // Path mode keywords should be stripped from the source before cypher-core parsing
        let src = "MATCH TRAIL (a)-[:KNOWS]->(b) RETURN a";
        let out = normalise_gql_source(src);
        assert!(
            !contains_word(&out.to_ascii_uppercase(), "TRAIL"),
            "TRAIL should be stripped from normalised source, got: {out}"
        );
    }

    #[test]
    fn normalise_path_search_keywords_stripped() {
        let src = "MATCH ANY SHORTEST (a)-[:KNOWS]->(b) RETURN a";
        let out = normalise_gql_source(src);
        assert!(
            !contains_word(&out.to_ascii_uppercase(), "SHORTEST"),
            "ANY SHORTEST should be stripped, got: {out}"
        );
    }
}
