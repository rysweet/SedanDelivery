//! Recursive descent parser for openCypher / GQL.
//!
//! Entry point: [`parse_query`].
//!
//! Design notes:
//! - The `|` character is handled contextually:
//!   - Inside a label or rel-type list it is label/type disjunction: `(n:A|B)`.
//!   - Inside a list comprehension `[x IN list | expr]` it separates the
//!     filter variable from the projection expression.
//! - All error paths carry a byte offset from the original source.

mod clause;
mod expression;
mod helpers;
mod pattern;

use crate::ast::{Clause, Query};
use crate::error::ParseError;
use crate::parser::lexer::{Token, TokenKind};

// ── Parser struct ─────────────────────────────────────────────────────────────

/// Cursor-based recursive descent parser.
///
/// Tokens are pre-collected from the lexer before parsing begins, so every
/// lookahead is an O(1) slice index and there is no buffering complexity.
pub struct Parser<'src> {
    tokens: Vec<Token<'src>>,
    pos: usize,
    /// Current expression nesting depth; guards against deeply nested DoS inputs.
    depth: u32,
    /// Maximum allowed expression nesting depth (injected from deployment config).
    max_depth: u32,
}

impl<'src> Parser<'src> {
    /// Create a parser from a pre-lexed token stream.
    ///
    /// The token stream must end with a [`TokenKind::Eof`] sentinel.
    pub fn new(tokens: Vec<Token<'src>>) -> Self {
        let max_depth = crate::config::global().parser_max_depth;
        Self {
            tokens,
            pos: 0,
            depth: 0,
            max_depth,
        }
    }
}

// ── Top-level entry point ─────────────────────────────────────────────────────

/// Parse `src` into a [`Query`] AST.
///
/// Returns [`ParseError::EmptyQuery`] for blank/empty input.
/// Returns [`ParseError::Syntax`] for any structural grammar error.
pub fn parse_query(src: &str) -> Result<Query, ParseError> {
    parse_query_with_max_size(src, crate::config::global().max_query_size)
}

/// Like [`parse_query`] but with an explicit byte-length limit.
///
/// Useful for testing the size guard without relying on the global config.
pub fn parse_query_with_max_size(src: &str, max_query_size: usize) -> Result<Query, ParseError> {
    if src.len() > max_query_size {
        return Err(ParseError::QueryTooLarge {
            size: src.len(),
            limit: max_query_size,
        });
    }
    if src.trim().is_empty() {
        return Err(ParseError::EmptyQuery);
    }
    let tokens = crate::parser::lexer::lex_query(src)?;
    let mut p = Parser::new(tokens);
    p.parse_query_inner()
}

impl<'src> Parser<'src> {
    fn parse_query_inner(&mut self) -> Result<Query, ParseError> {
        let clauses = self.parse_clause_sequence()?;

        if clauses.is_empty() {
            return Err(ParseError::EmptyQuery);
        }

        // Parse optional UNION / UNION ALL parts.
        let mut unions = Vec::new();
        while self.at(&TokenKind::Union) {
            self.advance(); // consume UNION
            let all = self.eat(&TokenKind::All);
            let part = self.parse_clause_sequence()?;
            if part.is_empty() {
                return Err(self.syntax_err("clause after UNION"));
            }
            unions.push((all, part));
        }

        Ok(Query { clauses, unions })
    }

    /// Parse a sequence of clauses (separated by optional semicolons),
    /// stopping at UNION, EOF, or when no more clauses can be parsed.
    fn parse_clause_sequence(&mut self) -> Result<Vec<Clause>, ParseError> {
        let mut clauses = Vec::new();
        // Skip leading semicolons.
        while self.eat(&TokenKind::Semicolon) {}

        loop {
            if self.at(&TokenKind::Eof) || self.at(&TokenKind::Union) {
                break;
            }
            match self.parse_clause()? {
                Some(clause) => clauses.push(clause),
                None => break,
            }
            // Optional clause separator.
            while self.eat(&TokenKind::Semicolon) {}
        }

        Ok(clauses)
    }
}
// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests_basic;
#[cfg(test)]
mod tests_clauses;
#[cfg(test)]
mod tests_expressions;
#[cfg(test)]
mod tests_literals;
#[cfg(test)]
mod tests_patterns;
