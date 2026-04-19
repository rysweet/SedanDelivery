//! Lightweight token model used by the GQL→Cypher source rewriter.
//!
//! The rewriter does not need a full parser — it just needs to walk the
//! source one whitespace-or-word chunk at a time so it can substitute
//! keywords without disturbing punctuation, whitespace, or identifiers.

/// Keywords that start a new clause — used to delimit expression collection.
pub(super) const CLAUSE_KEYWORDS: &[&str] = &[
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

/// Minimal token for rewriting: preserves whitespace and punctuation as-is.
pub(super) struct RewriteToken<'a> {
    pub(super) text: &'a str,
}

/// Split source into word tokens and non-word tokens for rewriting.
pub(super) fn tokenise_for_rewrite(src: &str) -> Vec<RewriteToken<'_>> {
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

/// Collect tokens until a clause keyword is encountered, returning the trimmed expression.
pub(super) fn collect_until_clause(tokens: &[RewriteToken<'_>], i: &mut usize) -> String {
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
pub(super) fn is_clause_keyword(text: &str) -> bool {
    CLAUSE_KEYWORDS
        .iter()
        .any(|kw| text.eq_ignore_ascii_case(kw))
}
