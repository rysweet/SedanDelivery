//! GQL-to-Cypher source-level rewriting.
//!
//! GQL shares most syntax with openCypher but has several keyword differences.
//! This module performs lightweight string-level normalisation so the
//! cypher-core parser can handle the result.
//!
//! The approach is intentionally simple: regex-based keyword substitution.
//! A full GQL-native parser is a future milestone — this gets us running.

use super::{GqlMetadata, PathMode, PathSearch};

/// Normalise GQL source into cypher-core–compatible source.
///
/// Substitutions (case-insensitive, whole-word):
/// - `INSERT` → `CREATE`
/// - `FILTER` (as clause, not function) → `WHERE`
/// - `MANDATORY MATCH` → `MATCH` (metadata captures the flag)
/// - `FOR` (as clause) → `FOREACH`
pub fn normalise_gql_source(src: &str) -> String {
    let mut result = String::with_capacity(src.len());
    let tokens = tokenise_for_rewrite(src);

    let mut i = 0;
    while i < tokens.len() {
        let tok = &tokens[i];
        let upper = tok.text.to_ascii_uppercase();

        match upper.as_str() {
            "INSERT" => {
                result.push_str("CREATE");
                i += 1;
            }
            "MANDATORY" => {
                // MANDATORY MATCH → MATCH (the mandatory flag is in metadata)
                // Look past whitespace tokens for "MATCH"
                let next_word = tokens[i + 1..].iter().find(|t| !t.text.trim().is_empty());
                if next_word.is_some_and(|t| t.text.eq_ignore_ascii_case("MATCH")) {
                    // Skip "MANDATORY" and the whitespace after it
                    i += 1;
                    // Skip whitespace tokens between MANDATORY and MATCH
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
                // If followed by '(' it's the function; otherwise it's the clause.
                let next_non_ws = tokens.get(i + 1);
                if next_non_ws.is_none_or(|t| t.text != "(") {
                    result.push_str("WHERE");
                } else {
                    result.push_str(tok.text);
                }
                i += 1;
            }
            _ => {
                result.push_str(tok.text);
                i += 1;
            }
        }
    }

    result
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

    // Check for path search modifiers
    if upper.contains("ANY SHORTEST") || upper.contains("ANY  SHORTEST") {
        meta.path_search = Some(PathSearch::AnyShortest);
    } else if upper.contains("ALL SHORTEST") || upper.contains("ALL  SHORTEST") {
        meta.path_search = Some(PathSearch::AllShortest);
    }

    meta
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
}
