//! Token consumption, error recovery, and utility functions.

use crate::error::ParseError;
use crate::parser::lexer::{Token, TokenKind};

use super::Parser;

impl<'src> Parser<'src> {
    // ── Cursor helpers ────────────────────────────────────────────────────────

    /// Return the kind of the current token without advancing.
    pub(super) fn peek(&self) -> &TokenKind {
        &self.tokens[self.pos].kind
    }

    /// Return the kind of the token one position ahead without advancing.
    pub(super) fn peek2(&self) -> &TokenKind {
        let idx = (self.pos + 1).min(self.tokens.len() - 1);
        &self.tokens[idx].kind
    }

    /// Peek two positions ahead (pos + 2).
    pub(super) fn peek3(&self) -> &TokenKind {
        let idx = (self.pos + 2).min(self.tokens.len() - 1);
        &self.tokens[idx].kind
    }

    /// Byte offset of the current token.
    pub(super) fn offset(&self) -> usize {
        self.tokens[self.pos].offset
    }

    /// Consume and return the current token.
    pub(super) fn advance(&mut self) -> &Token<'src> {
        let tok = &self.tokens[self.pos];
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    /// Return `true` if the current token matches `kind`.
    pub(super) fn at(&self, kind: &TokenKind) -> bool {
        self.peek() == kind
    }

    /// Consume the current token and return it if it matches `kind`,
    /// otherwise return `false` without consuming.
    pub(super) fn eat(&mut self, kind: &TokenKind) -> bool {
        if self.peek() == kind {
            self.advance();
            true
        } else {
            false
        }
    }

    /// Consume the current token if it matches `kind`, or return a
    /// [`ParseError::Syntax`] error.
    pub(super) fn expect(&mut self, kind: &TokenKind) -> Result<&Token<'src>, ParseError> {
        if self.peek() == kind {
            // SAFETY: advance() returns a reference into self.tokens which
            // outlives this parser, but the borrow checker can't see through
            // the mutable borrow. We use an index to work around this.
            let idx = self.pos;
            self.pos += 1;
            Ok(&self.tokens[idx])
        } else {
            Err(self.syntax_err(format!("{:?}", kind)))
        }
    }

    /// Consume an identifier-shaped token (bare Identifier or any keyword
    /// that may be used as an identifier in property-access position).
    /// Returns the text of the token as an owned `String`.
    pub(super) fn expect_identifier(&mut self) -> Result<String, ParseError> {
        match self.peek().clone() {
            TokenKind::Identifier => {
                let tok = self.advance();
                // Strip backtick quotes if present.
                let text = tok.text;
                if text.starts_with('`') && text.ends_with('`') && text.len() >= 2 {
                    Ok(text[1..text.len() - 1].to_string())
                } else {
                    Ok(text.to_string())
                }
            }
            // Many keywords are valid as variable/property names in Cypher.
            kind if is_keyword_usable_as_ident(&kind) => {
                let tok = self.advance();
                Ok(tok.text.to_string())
            }
            _ => Err(self.syntax_err("identifier")),
        }
    }

    /// Accept an identifier or a keyword that can be used as a map key or property name.
    pub(super) fn expect_map_key(&mut self) -> Result<String, ParseError> {
        match self.peek().clone() {
            TokenKind::Identifier => {
                let tok = self.advance();
                let text = tok.text;
                if text.starts_with('`') && text.ends_with('`') && text.len() >= 2 {
                    Ok(text[1..text.len() - 1].to_string())
                } else {
                    Ok(text.to_string())
                }
            }
            kind if is_keyword_usable_as_property(&kind) => {
                let tok = self.advance();
                Ok(tok.text.to_string())
            }
            _ => Err(self.syntax_err("identifier or property name")),
        }
    }

    pub(super) fn syntax_err(&self, expected: impl Into<String>) -> ParseError {
        let found = format!("{:?}", self.peek());
        ParseError::Syntax {
            offset: self.offset(),
            expected: expected.into(),
            found,
        }
    }
}

/// Returns `true` for keyword tokens that are also valid Cypher identifiers
/// (property keys, variable names, function names, relationship type names, etc.).
pub(super) fn is_keyword_usable_as_ident(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Count
            | TokenKind::All
            | TokenKind::Starts
            | TokenKind::Ends
            | TokenKind::Contains
            | TokenKind::Skip
            | TokenKind::Limit
            | TokenKind::Order
            | TokenKind::By
            | TokenKind::Distinct
            | TokenKind::As
            | TokenKind::Call
            | TokenKind::Yield
            | TokenKind::Null
            | TokenKind::True
            | TokenKind::False
            | TokenKind::End
            | TokenKind::Foreach
            | TokenKind::In
    )
}

/// Returns `true` for keyword tokens that can be used as map keys or property names.
/// This is a superset of `is_keyword_usable_as_ident` that includes literal keywords
/// and other context-sensitive keywords that are valid as property names.
pub(super) fn is_keyword_usable_as_property(kind: &TokenKind) -> bool {
    is_keyword_usable_as_ident(kind)
        || matches!(
            kind,
            TokenKind::Null
                | TokenKind::True
                | TokenKind::False
                | TokenKind::Case
                | TokenKind::When
                | TokenKind::Then
                | TokenKind::Else
                | TokenKind::End
                | TokenKind::In
                | TokenKind::Is
                | TokenKind::Not
                | TokenKind::And
                | TokenKind::Or
                | TokenKind::Xor
                | TokenKind::Match
                | TokenKind::Where
                | TokenKind::Return
                | TokenKind::Create
                | TokenKind::Delete
                | TokenKind::Set
                | TokenKind::Remove
                | TokenKind::Merge
                | TokenKind::With
                | TokenKind::Unwind
                | TokenKind::Optional
                | TokenKind::Detach
                | TokenKind::Foreach
                | TokenKind::Union
                | TokenKind::Starts
                | TokenKind::Ends
                | TokenKind::Contains
        )
}
