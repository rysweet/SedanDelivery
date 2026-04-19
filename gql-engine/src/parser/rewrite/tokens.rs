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
///
/// This walks the source by character boundaries (not raw bytes) so that
/// multi-byte UTF-8 input cannot produce a slice that splits a code point —
/// such a slice would panic at runtime. Non-ASCII characters are treated as
/// non-word, single-character tokens.
pub(super) fn tokenise_for_rewrite(src: &str) -> Vec<RewriteToken<'_>> {
    // Average token length ~4 chars gives a reasonable initial capacity.
    let mut tokens = Vec::with_capacity(src.len() / 4 + 1);
    let bytes = src.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        let b = bytes[i];
        if b.is_ascii_alphanumeric() || b == b'_' {
            // Word token (ASCII identifier chars only).
            let start = i;
            while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            tokens.push(RewriteToken {
                text: &src[start..i],
            });
        } else if b.is_ascii() {
            // Non-word ASCII token (whitespace, punctuation, etc.)
            let start = i;
            i += 1;
            if b.is_ascii_whitespace() {
                while i < len && bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
            }
            tokens.push(RewriteToken {
                text: &src[start..i],
            });
        } else {
            // Non-ASCII: advance by the full UTF-8 character width so we
            // never slice through a multi-byte code point.
            let start = i;
            // Safety: `i` is on a UTF-8 char boundary (we only ever advance
            // by full chars or single ASCII bytes), so `src[i..]` is valid.
            let ch = src[i..]
                .chars()
                .next()
                .expect("non-empty remainder must have at least one char");
            i += ch.len_utf8();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenise_handles_multibyte_utf8_without_panic() {
        // Regression test for issue #1674: this input previously panicked
        // with "byte index N is not a char boundary" because the tokeniser
        // walked raw bytes and sliced &str at non-boundary offsets.
        let src = "MATľ(n) RETURN n";
        let tokens = tokenise_for_rewrite(src);
        let joined: String = tokens.iter().map(|t| t.text).collect();
        assert_eq!(joined, src);
    }

    #[test]
    fn tokenise_preserves_arbitrary_unicode() {
        let src = "RETURN \"日本語\" AS x";
        let tokens = tokenise_for_rewrite(src);
        let joined: String = tokens.iter().map(|t| t.text).collect();
        assert_eq!(joined, src);
    }

    #[test]
    fn parse_gql_does_not_panic_on_multibyte_input() {
        // End-to-end: the public parser must never panic on UTF-8 input,
        // regardless of whether the input parses successfully.
        let _ = crate::parser::parse_gql("MATľ(n) RETURN n");
        let _ = crate::parser::parse_gql("MATCH (n) RETURN \"日本語\"");
    }
}
