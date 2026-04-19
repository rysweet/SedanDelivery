//! Identifier and keyword scanning.

use super::super::tokens::{keyword, TokenKind};
use super::Lexer;

impl Lexer<'_> {
    /// Lex an identifier or keyword starting at `start`.
    /// On entry, `self.pos` is at the first character (ASCII alphabetic or `_`).
    /// Advances `self.pos` past the identifier and returns the resulting `TokenKind`.
    pub(super) fn lex_identifier_or_keyword(&mut self, start: usize) -> TokenKind {
        while matches!(
            self.peek_byte(0),
            Some(b'a'..=b'z') | Some(b'A'..=b'Z') | Some(b'0'..=b'9') | Some(b'_')
        ) {
            self.pos += 1;
        }
        let text = &self.src[start..self.pos];
        let lower = text.to_ascii_lowercase();
        keyword(&lower).unwrap_or(TokenKind::Identifier)
    }
}
