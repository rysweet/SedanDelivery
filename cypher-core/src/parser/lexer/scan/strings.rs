//! String and backtick-identifier scanning.

use crate::error::ParseError;

use super::Lexer;

impl<'src> Lexer<'src> {
    /// Lex a string literal (opening quote already consumed).
    /// `quote` is either `b'\''` or `b'"'`.
    /// Returns the unescaped string content, or a `ParseError`.
    pub(super) fn lex_string(&mut self, quote: u8, start: usize) -> Result<String, ParseError> {
        let mut buf = String::new();
        loop {
            match self.peek_byte(0) {
                None => {
                    return Err(ParseError::UnterminatedString { offset: start });
                }
                Some(b) if b == quote => {
                    self.pos += 1;
                    return Ok(buf);
                }
                Some(b'\\') => {
                    self.pos += 1; // consume backslash
                    let escape_offset = self.pos;
                    match self.peek_byte(0) {
                        None => {
                            return Err(ParseError::UnterminatedString { offset: start });
                        }
                        Some(b'n') => {
                            self.pos += 1;
                            buf.push('\n');
                        }
                        Some(b't') => {
                            self.pos += 1;
                            buf.push('\t');
                        }
                        Some(b'r') => {
                            self.pos += 1;
                            buf.push('\r');
                        }
                        Some(b'\\') => {
                            self.pos += 1;
                            buf.push('\\');
                        }
                        Some(b'\'') => {
                            self.pos += 1;
                            buf.push('\'');
                        }
                        Some(b'"') => {
                            self.pos += 1;
                            buf.push('"');
                        }
                        Some(b'b') => {
                            self.pos += 1;
                            buf.push('\x08');
                        }
                        Some(b'f') => {
                            self.pos += 1;
                            buf.push('\x0C');
                        }
                        Some(b'u') => {
                            // \uXXXX Unicode escape
                            self.pos += 1;
                            let hex_start = self.pos;
                            let mut code: u32 = 0;
                            let mut digits = 0;
                            while digits < 4 {
                                match self.peek_byte(0) {
                                    Some(b) if (b as char).is_ascii_hexdigit() => {
                                        // safe: is_ascii_hexdigit guard ensures to_digit(16) succeeds
                                        code = code * 16 + (b as char).to_digit(16).unwrap();
                                        self.pos += 1;
                                        digits += 1;
                                    }
                                    _ => break,
                                }
                            }
                            if digits < 4 {
                                return Err(ParseError::InvalidEscape {
                                    char: 'u',
                                    offset: hex_start - 1,
                                });
                            }
                            match char::from_u32(code) {
                                Some(c) => buf.push(c),
                                None => {
                                    return Err(ParseError::InvalidEscape {
                                        char: 'u',
                                        offset: hex_start - 1,
                                    });
                                }
                            }
                        }
                        Some(b) => {
                            let c = b as char;
                            self.pos += 1;
                            return Err(ParseError::InvalidEscape {
                                char: c,
                                offset: escape_offset - 1,
                            });
                        }
                    }
                }
                Some(_) => {
                    // Regular character — may be multi-byte UTF-8.
                    // safe: peek_byte returned Some, so rest() is non-empty
                    let c = self.rest().chars().next().unwrap();
                    self.pos += c.len_utf8();
                    buf.push(c);
                }
            }
        }
    }

    /// Lex a backtick-quoted identifier (opening backtick already consumed).
    pub(super) fn lex_backtick_identifier(
        &mut self,
        start: usize,
    ) -> Result<&'src str, ParseError> {
        let content_start = self.pos;
        loop {
            match self.peek_byte(0) {
                None => {
                    return Err(ParseError::UnterminatedString { offset: start });
                }
                Some(b'`') => {
                    let end = self.pos;
                    self.pos += 1;
                    return Ok(&self.src[content_start..end]);
                }
                Some(b) => {
                    self.pos += if b.is_ascii() {
                        1
                    } else {
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
