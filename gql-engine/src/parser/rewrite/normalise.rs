//! Token-stream rewriting from GQL clause syntax into Cypher equivalents.

use super::tokens::{collect_until_clause, tokenise_for_rewrite, RewriteToken};

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
