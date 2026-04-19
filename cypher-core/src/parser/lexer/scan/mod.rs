//! Cursor-based lexer state and the main `next_token` dispatch loop.
//!
//! Token-category-specific scanning logic lives in submodules:
//! - [`strings`]: string literals and backtick-quoted identifiers
//! - [`numbers`]: integer, float, hex, and octal literals
//! - [`identifiers`]: identifiers and keywords
//! - [`operators`]: single- and multi-character operators and punctuation
//! - [`trivia`]: whitespace and comment skipping

mod identifiers;
mod numbers;
mod operators;
mod strings;
mod trivia;

#[cfg(test)]
mod tests;

use crate::error::ParseError;

use super::tokens::{Token, TokenKind};

// ── Lexer ─────────────────────────────────────────────────────────────────────

/// Internal cursor-based lexer state.
pub(crate) struct Lexer<'src> {
    src: &'src str,
    /// Current byte position in `src`.
    pub(crate) pos: usize,
}

impl<'src> Lexer<'src> {
    pub(crate) fn new(src: &'src str) -> Self {
        Self { src, pos: 0 }
    }

    /// Remaining source from current position.
    pub(super) fn rest(&self) -> &'src str {
        &self.src[self.pos..]
    }

    /// Peek at the byte at `self.pos + offset` without advancing.
    pub(super) fn peek_byte(&self, offset: usize) -> Option<u8> {
        self.src.as_bytes().get(self.pos + offset).copied()
    }

    /// Produce the next token, skipping whitespace and comments.
    /// Returns `Ok(None)` at end of input, `Ok(Some(token))` otherwise,
    /// and `Err` on lexical errors (unterminated strings, bad escapes, etc.).
    pub(crate) fn next_token(&mut self) -> Result<Option<Token<'src>>, ParseError> {
        // Skip whitespace and comments in a loop (comments may be adjacent).
        loop {
            self.skip_whitespace();
            if self.rest().is_empty() {
                return Ok(None);
            }
            // safe: rest().is_empty() returned false above, so byte 0 exists
            let b0 = self.peek_byte(0).unwrap();
            let b1 = self.peek_byte(1);
            if b0 == b'/' && b1 == Some(b'/') {
                self.pos += 2;
                self.skip_line_comment();
                continue;
            }
            if b0 == b'/' && b1 == Some(b'*') {
                self.pos += 2;
                if !self.skip_block_comment() {
                    // Block comment never terminated — report at the `/*` location.
                    return Err(ParseError::UnterminatedString { offset: self.pos });
                }
                continue;
            }
            break;
        }

        let start = self.pos;
        // safe: loop above only breaks when rest() is non-empty
        let b0 = self.peek_byte(0).unwrap();
        let b1 = self.peek_byte(1);

        // ── Single/double-quoted string literals ──────────────────────────────
        if b0 == b'\'' || b0 == b'"' {
            self.pos += 1; // consume opening quote
            let s = self.lex_string(b0, start)?;
            let text = &self.src[start..self.pos];
            return Ok(Some(Token::new(TokenKind::StringLiteral(s), text, start)));
        }

        // ── Backtick-quoted identifier ────────────────────────────────────────
        if b0 == b'`' {
            self.pos += 1;
            let _inner = self.lex_backtick_identifier(start)?;
            let text = &self.src[start..self.pos];
            return Ok(Some(Token::new(TokenKind::Identifier, text, start)));
        }

        // ── Numeric literals ──────────────────────────────────────────────────
        if b0.is_ascii_digit() {
            self.pos += 1;
            let kind = self.lex_number(start)?;
            let text = &self.src[start..self.pos];
            return Ok(Some(Token::new(kind, text, start)));
        }

        // ── Identifiers and keywords ──────────────────────────────────────────
        if b0.is_ascii_alphabetic() || b0 == b'_' {
            let kind = self.lex_identifier_or_keyword(start);
            let text = &self.src[start..self.pos];
            return Ok(Some(Token::new(kind, text, start)));
        }

        // ── Parameter: $name ──────────────────────────────────────────────────
        if b0 == b'$' {
            self.pos += 1;
            // $ alone emits DollarSign; $ident emits a parameter token implicitly
            // (the parser resolves $name). We emit DollarSign and let the parser
            // combine it with the following identifier.
            let text = &self.src[start..self.pos];
            return Ok(Some(Token::new(TokenKind::DollarSign, text, start)));
        }

        // ── Float literal starting with '.' (e.g. .1, .5e3) ─────────────────
        if b0 == b'.' && matches!(b1, Some(b'0'..=b'9')) {
            let kind = self.lex_dot_float(start)?;
            let text = &self.src[start..self.pos];
            return Ok(Some(Token::new(kind, text, start)));
        }

        // ── Two-character operators ────────────────────────────────────────────
        if let Some(kind) = self.try_two_char_operator(b0, b1) {
            let text = &self.src[start..self.pos];
            return Ok(Some(Token::new(kind, text, start)));
        }

        // ── Single-character operators and punctuation ────────────────────────
        let kind = self.lex_single_char_operator(start, b0);
        // text slice must be computed AFTER pos is fixed (handles multi-byte chars)
        let text = &self.src[start..self.pos];
        Ok(Some(Token::new(kind, text, start)))
    }
}
