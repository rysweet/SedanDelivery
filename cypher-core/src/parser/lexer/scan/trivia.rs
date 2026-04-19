//! Whitespace and comment skipping (lexical "trivia").

use super::Lexer;

impl Lexer<'_> {
    /// Skip whitespace (space, tab, CR, LF).
    pub(super) fn skip_whitespace(&mut self) {
        while let Some(b) = self.peek_byte(0) {
            if b == b' ' || b == b'\t' || b == b'\r' || b == b'\n' {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    /// Skip a `//`-style line comment (the `//` has already been consumed).
    pub(super) fn skip_line_comment(&mut self) {
        while let Some(b) = self.peek_byte(0) {
            self.pos += 1;
            if b == b'\n' {
                break;
            }
        }
    }

    /// Skip a `/* ... */` block comment (the `/*` has already been consumed).
    /// Returns `false` if the comment is unterminated (EOF reached).
    pub(super) fn skip_block_comment(&mut self) -> bool {
        loop {
            match self.peek_byte(0) {
                None => return false, // unterminated
                Some(b'*') if self.peek_byte(1) == Some(b'/') => {
                    self.pos += 2;
                    return true;
                }
                Some(b) => {
                    self.pos += if b.is_ascii() {
                        1
                    } else {
                        // multi-byte UTF-8 char — advance properly
                        self.rest()
                            .chars()
                            .next()
                            .map(|c| c.len_utf8())
                            .unwrap_or(1)
                    };
                }
            }
        }
    }
}
