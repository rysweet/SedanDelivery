//! Query string pre-processing helpers for the RUN handler.
//!
//! Stateless string-level utilities for stripping Neo4j pre-parser
//! directives, detecting `EXPLAIN`/`PROFILE` prefixes, and parsing
//! `USE <database>` clauses.

/// Strip Neo4j pre-parser directives like `CYPHER runtime=interpreted ...`
/// from the beginning of a query string.
///
/// If the query starts with `"CYPHER "` (case-insensitive), scans forward for
/// the first recognized Cypher keyword and returns the substring starting at
/// that keyword. Otherwise returns the trimmed input unchanged.
pub(super) fn strip_cypher_prefix(query: &str) -> &str {
    let trimmed = query.trim();
    if trimmed
        .get(..7)
        .is_some_and(|s| s.eq_ignore_ascii_case("CYPHER "))
    {
        let cypher_keywords = [
            "MATCH ",
            "RETURN ",
            "CREATE ",
            "MERGE ",
            "WITH ",
            "UNWIND ",
            "CALL ",
            "OPTIONAL ",
            "DELETE ",
            "DETACH ",
            "SET ",
            "REMOVE ",
            "EXPLAIN ",
            "PROFILE ",
        ];
        let upper = trimmed.to_uppercase();
        let mut start = trimmed.len(); // fallback: keep entire string
        for kw in &cypher_keywords {
            if let Some(pos) = upper[7..].find(kw) {
                let candidate = 7 + pos;
                if candidate < start {
                    start = candidate;
                }
            }
        }
        &trimmed[start..]
    } else {
        trimmed
    }
}

/// Detect `EXPLAIN` or `PROFILE` prefix on a query (case-insensitive).
///
/// Returns `(is_explain, is_profile, actual_query)` where `actual_query` is
/// the query text with the prefix stripped.
pub(super) fn detect_explain_profile(query: &str) -> (bool, bool, String) {
    let is_explain = query
        .get(..8)
        .is_some_and(|s| s.eq_ignore_ascii_case("EXPLAIN "));
    let is_profile = !is_explain
        && query
            .get(..8)
            .is_some_and(|s| s.eq_ignore_ascii_case("PROFILE "));
    let actual_query = if is_explain || is_profile {
        query[8..].to_string()
    } else {
        query.to_string()
    };
    (is_explain, is_profile, actual_query)
}

/// Parse a `USE <database>` prefix from a query string.
///
/// Returns `(db_name, rest)` where `rest` is the remaining query after the
/// USE clause (trimmed), or an empty string if the USE is the entire query.
/// Returns `None` if the query does not start with `USE `.
pub(super) fn parse_use_prefix(query: &str) -> Option<(String, &str)> {
    let trimmed = query.trim();
    let upper = trimmed.to_uppercase();
    if !upper.starts_with("USE ") {
        return None;
    }
    let rest = trimmed["USE ".len()..].trim();
    if rest.is_empty() {
        return None;
    }
    // Extract the database name. Handle backtick-quoted and simple identifiers.
    let (name, name_end): (String, usize) = if rest.starts_with('`') {
        // Backtick-quoted: scan for closing backtick.
        let bytes = rest.as_bytes();
        let mut i = 1;
        let mut result = String::new();
        while i < bytes.len() {
            if bytes[i] == b'`' {
                if i + 1 < bytes.len() && bytes[i + 1] == b'`' {
                    result.push('`');
                    i += 2;
                } else {
                    return Some((result, rest[i + 1..].trim()));
                }
            } else {
                result.push(bytes[i] as char);
                i += 1;
            }
        }
        (result, rest.len())
    } else {
        // Simple identifier: alphanumeric + underscore + hyphen + dot
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '-' || *c == '.')
            .collect();
        let end = name.len();
        (name, end)
    };
    if name.is_empty() {
        None
    } else {
        let after_name = rest[name_end..].trim();
        Some((name, after_name))
    }
}

/// Parse `USE <database>` standalone command. Returns the database name if
/// the query is purely a USE switch (no trailing content), `None` otherwise.
pub(super) fn parse_use_database(query: &str) -> Option<String> {
    parse_use_prefix(query).and_then(
        |(name, rest)| {
            if rest.is_empty() {
                Some(name)
            } else {
                None
            }
        },
    )
}
