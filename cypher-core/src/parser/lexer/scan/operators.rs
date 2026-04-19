//! Operator and punctuation scanning.

use super::super::tokens::TokenKind;
use super::Lexer;

impl Lexer<'_> {
    /// Try to consume a two-character operator starting at the current position.
    /// Returns `Some(kind)` and advances `self.pos` by 2 on a match, else `None`.
    pub(super) fn try_two_char_operator(&mut self, b0: u8, b1: Option<u8>) -> Option<TokenKind> {
        let kind = match (b0, b1) {
            (b'<', Some(b'>')) => TokenKind::Neq,
            (b'<', Some(b'=')) => TokenKind::Lte,
            (b'>', Some(b'=')) => TokenKind::Gte,
            (b'-', Some(b'>')) => TokenKind::Arrow,
            (b'<', Some(b'-')) => TokenKind::LeftArrow,
            (b'.', Some(b'.')) => TokenKind::DoubleDot,
            (b'=', Some(b'~')) => TokenKind::RegexEq,
            _ => return None,
        };
        self.pos += 2;
        Some(kind)
    }

    /// Consume a single-character operator or punctuation token.
    /// On entry, `self.pos` is at the character to consume; on exit, `self.pos`
    /// has advanced past it (handling multi-byte UTF-8 for `Unknown`).
    /// Returns the resulting `TokenKind`.
    pub(super) fn lex_single_char_operator(&mut self, start: usize, b0: u8) -> TokenKind {
        self.pos += 1;
        match b0 {
            b'(' => TokenKind::LParen,
            b')' => TokenKind::RParen,
            b'[' => TokenKind::LBracket,
            b']' => TokenKind::RBracket,
            b'{' => TokenKind::LBrace,
            b'}' => TokenKind::RBrace,
            b',' => TokenKind::Comma,
            b'.' => TokenKind::Dot,
            b':' => TokenKind::Colon,
            b';' => TokenKind::Semicolon,
            b'=' => TokenKind::Eq,
            b'<' => TokenKind::Lt,
            b'>' => TokenKind::Gt,
            b'+' => TokenKind::Plus,
            b'-' => TokenKind::Dash,
            b'*' => TokenKind::Star,
            b'/' => TokenKind::Slash,
            b'%' => TokenKind::Percent,
            b'^' => TokenKind::Caret,
            b'&' => TokenKind::Amp,
            b'|' => TokenKind::Pipe,
            b'~' => TokenKind::Tilde,
            _ => {
                // Non-ASCII or truly unknown.
                let c = self.src[start..].chars().next().unwrap_or('?');
                // If it was a multi-byte char we consumed too little above; fix pos.
                self.pos = start + c.len_utf8();
                TokenKind::Unknown(c)
            }
        }
    }
}
