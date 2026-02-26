//! GQL / openCypher parser — Phase 0 skeleton.
//!
//! Phase 0 ships a complete, tested lexer and the public API stubs that callers
//! depend on. The full recursive-descent parser is implemented in Phase 1.
//!
//! See ADR-003: hand-written recursive descent chosen over lalrpop because the
//! `|` character is context-dependent in Cypher (label disjunction, rel-type
//! disjunction, list comprehension pipe) and cannot be resolved at the grammar
//! level without semantic context.

pub mod lexer;

pub use lexer::{Token, TokenKind};

use crate::error::ParseError;

/// Tokenise `src` into a vector of [`Token`]s, terminated by [`TokenKind::Eof`].
///
/// Strictly validates the input: returns [`ParseError::UnterminatedString`] for
/// unterminated string literals, [`ParseError::IntegerOverflow`] for oversized
/// integer literals, and [`ParseError::InvalidEscape`] for unrecognised
/// backslash escapes.
///
/// This function is the canonical entry point for the lexer; use [`tokenize`]
/// only in contexts where you want lenient, best-effort tokenisation.
pub fn lex_query(src: &str) -> Result<Vec<Token<'_>>, ParseError> {
    lexer::lex_query(src)
}

/// Lenient tokeniser that never returns an error.
///
/// Unknown characters are emitted as [`TokenKind::Unknown`] tokens; lexical
/// errors (e.g. unterminated strings) emit an [`TokenKind::Unknown`] at the
/// problem position and then continue tokenising.
///
/// Primarily used for syntax highlighting and IDE tooling where partial input
/// is common.
pub fn tokenize(src: &str) -> Vec<Token<'_>> {
    lexer::tokenize(src)
}

/// Parse a GQL / Cypher query string into an AST.
///
/// Phase 0 placeholder — always returns `Ok(())`. The full parser is
/// implemented in Phase 1 and will return a `Query` AST node.
///
/// # Errors
///
/// Returns [`ParseError::EmptyQuery`] if `src` is entirely whitespace or empty
/// (even the Phase 0 stub validates this, since downstream code assumes at
/// least one clause).
pub fn parse_query(src: &str) -> Result<(), ParseError> {
    // placeholder — full parser implemented in Phase 1
    if src.trim().is_empty() {
        return Err(ParseError::EmptyQuery);
    }
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lex_query_simple() {
        let toks = lex_query("MATCH (n) RETURN n").unwrap();
        assert_eq!(toks[0].kind, TokenKind::Match);
        assert!(toks.last().unwrap().kind.is_eof());
    }

    #[test]
    fn lex_query_unterminated_string() {
        let result = lex_query("'oops");
        assert!(matches!(result, Err(ParseError::UnterminatedString { .. })));
    }

    #[test]
    fn tokenize_is_lenient() {
        // tokenize() does not return errors; unknowns become Unknown tokens.
        let toks = tokenize("'unterminated");
        assert!(!toks.is_empty());
    }

    #[test]
    fn parse_query_empty_returns_error() {
        assert!(matches!(parse_query(""), Err(ParseError::EmptyQuery)));
        assert!(matches!(
            parse_query("   \n  "),
            Err(ParseError::EmptyQuery)
        ));
    }

    #[test]
    fn parse_query_non_empty_returns_ok() {
        assert!(parse_query("MATCH (n) RETURN n").is_ok());
    }
}
