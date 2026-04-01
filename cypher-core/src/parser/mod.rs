//! GQL / openCypher parser.
//!
//! The lexer lives in [`lexer`] and produces a [`Vec<Token>`] via [`lex_query`].
//! The recursive-descent parser lives in [`parser`] and consumes that token
//! stream to build a [`crate::ast::Query`] AST.
//!
//! See ADR-003: hand-written recursive descent chosen over lalrpop because the
//! `|` character is context-dependent in Cypher (label disjunction, rel-type
//! disjunction, list comprehension pipe) and cannot be resolved at the grammar
//! level without semantic context.

pub mod lexer;
#[allow(clippy::module_inception)]
pub mod parser;

pub use lexer::{Token, TokenKind};
pub use parser::{parse_query, parse_query_with_max_size};

/// Tokenise `src` into a vector of [`Token`]s, terminated by [`TokenKind::Eof`].
///
/// Strictly validates the input: returns [`ParseError::UnterminatedString`] for
/// unterminated string literals, [`ParseError::IntegerOverflow`] for oversized
/// integer literals, and [`ParseError::InvalidEscape`] for unrecognised
/// backslash escapes.
pub fn lex_query(src: &str) -> Result<Vec<Token<'_>>, crate::error::ParseError> {
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ParseError;

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
    fn parse_query_returns_ast() {
        let q = parse_query("MATCH (n) RETURN n").unwrap();
        assert_eq!(q.clauses.len(), 2);
    }

    #[test]
    fn parse_reduce_expression() {
        let q = parse_query("RETURN reduce(total = 0, x IN [1,2,3] | total + x)").unwrap();
        assert_eq!(q.clauses.len(), 1);
        match &q.clauses[0] {
            crate::ast::Clause::Return(ret) => {
                assert_eq!(ret.items.len(), 1);
                match &ret.items[0].expr {
                    crate::ast::Expr::Reduce {
                        accumulator,
                        initial,
                        variable,
                        list,
                        expression,
                    } => {
                        assert_eq!(accumulator.0, "total");
                        assert!(matches!(**initial, crate::ast::Expr::Integer(0)));
                        assert_eq!(variable.0, "x");
                        assert!(matches!(**list, crate::ast::Expr::List(_)));
                        assert!(matches!(**expression, crate::ast::Expr::BinaryOp { .. }));
                    }
                    other => panic!("expected Reduce, got {:?}", other),
                }
            }
            other => panic!("expected Return, got {:?}", other),
        }
    }

    #[test]
    fn parse_extract_as_list_comp() {
        let q = parse_query("RETURN extract(x IN [1,2,3] | x * 2)").unwrap();
        assert_eq!(q.clauses.len(), 1);
        match &q.clauses[0] {
            crate::ast::Clause::Return(ret) => {
                assert_eq!(ret.items.len(), 1);
                match &ret.items[0].expr {
                    crate::ast::Expr::ListComp {
                        variable, filter, ..
                    } => {
                        assert_eq!(variable.0, "x");
                        assert!(filter.is_none());
                    }
                    other => panic!("expected ListComp, got {:?}", other),
                }
            }
            other => panic!("expected Return, got {:?}", other),
        }
    }

    #[test]
    fn parse_filter_as_list_comp() {
        let q = parse_query("RETURN filter(x IN [1,2,3,4,5] WHERE x > 3)").unwrap();
        assert_eq!(q.clauses.len(), 1);
        match &q.clauses[0] {
            crate::ast::Clause::Return(ret) => {
                assert_eq!(ret.items.len(), 1);
                match &ret.items[0].expr {
                    crate::ast::Expr::ListComp {
                        variable,
                        filter,
                        map,
                        ..
                    } => {
                        assert_eq!(variable.0, "x");
                        assert!(filter.is_some());
                        // map should be just the variable (identity)
                        assert!(matches!(**map, crate::ast::Expr::Variable(_)));
                    }
                    other => panic!("expected ListComp, got {:?}", other),
                }
            }
            other => panic!("expected Return, got {:?}", other),
        }
    }
}
