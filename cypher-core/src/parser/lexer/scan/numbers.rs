//! Numeric literal scanning (integers, floats, hex, octal).

use crate::error::ParseError;

use super::super::tokens::TokenKind;
use super::Lexer;

impl Lexer<'_> {
    /// Lex a numeric literal (integer or float). The first digit is already past `start`.
    /// On entry `self.pos` is just after the first character (which is a digit).
    /// We receive `start` and `first_char` so we can form the full slice.
    pub(super) fn lex_number(&mut self, start: usize) -> Result<TokenKind, ParseError> {
        // Consume the rest of the integer part.
        while matches!(self.peek_byte(0), Some(b'0'..=b'9') | Some(b'_')) {
            self.pos += 1;
        }

        // Attempt hex literal detection: starts at position `start`, first char was '0'
        let is_hex = self.src.as_bytes().get(start) == Some(&b'0')
            && matches!(self.peek_byte(0), Some(b'x') | Some(b'X'));

        if is_hex {
            self.pos += 1; // consume 'x'/'X'
            let hex_start = self.pos;
            while matches!(
                self.peek_byte(0),
                Some(b'0'..=b'9') | Some(b'a'..=b'f') | Some(b'A'..=b'F') | Some(b'_')
            ) {
                self.pos += 1;
            }
            let hex_digits: String = self.src[hex_start..self.pos]
                .chars()
                .filter(|c| *c != '_')
                .collect();
            // If the hex value is exactly 2^63, emit IntMinMagnitude (valid only after unary -)
            return i64::from_str_radix(&hex_digits, 16)
                .map(TokenKind::IntLiteral)
                .or_else(|_| {
                    u64::from_str_radix(&hex_digits, 16)
                        .ok()
                        .filter(|&v| v == (i64::MAX as u64) + 1)
                        .map(|_| TokenKind::IntMinMagnitude)
                        .ok_or(())
                })
                .map_err(|_| ParseError::IntegerOverflow {
                    literal: self.src[start..self.pos].to_string(),
                    offset: start,
                });
        }

        // Octal literal detection: `0o...` or `0O...`
        let is_octal = self.src.as_bytes().get(start) == Some(&b'0')
            && matches!(self.peek_byte(0), Some(b'o') | Some(b'O'));

        if is_octal {
            self.pos += 1; // consume 'o'/'O'
            let oct_start = self.pos;
            while matches!(self.peek_byte(0), Some(b'0'..=b'7') | Some(b'_')) {
                self.pos += 1;
            }
            let oct_digits: String = self.src[oct_start..self.pos]
                .chars()
                .filter(|c| *c != '_')
                .collect();
            // If the octal value is exactly 2^63, emit IntMinMagnitude (valid only after unary -)
            return i64::from_str_radix(&oct_digits, 8)
                .map(TokenKind::IntLiteral)
                .or_else(|_| {
                    u64::from_str_radix(&oct_digits, 8)
                        .ok()
                        .filter(|&v| v == (i64::MAX as u64) + 1)
                        .map(|_| TokenKind::IntMinMagnitude)
                        .ok_or(())
                })
                .map_err(|_| ParseError::IntegerOverflow {
                    literal: self.src[start..self.pos].to_string(),
                    offset: start,
                });
        }

        let is_float = matches!(self.peek_byte(0), Some(b'.'))
            && !matches!(self.peek_byte(1), Some(b'.')) // avoid `..` being eaten
            || matches!(self.peek_byte(0), Some(b'e') | Some(b'E'));

        if is_float {
            // Consume fractional part.
            if self.peek_byte(0) == Some(b'.') {
                self.pos += 1;
                while matches!(self.peek_byte(0), Some(b'0'..=b'9') | Some(b'_')) {
                    self.pos += 1;
                }
            }
            // Consume exponent.
            if matches!(self.peek_byte(0), Some(b'e') | Some(b'E')) {
                self.pos += 1;
                if matches!(self.peek_byte(0), Some(b'+') | Some(b'-')) {
                    self.pos += 1;
                }
                while matches!(self.peek_byte(0), Some(b'0'..=b'9') | Some(b'_')) {
                    self.pos += 1;
                }
            }
            let text: String = self.src[start..self.pos]
                .chars()
                .filter(|c| *c != '_')
                .collect();
            return text
                .parse::<f64>()
                .map(TokenKind::FloatLiteral)
                .map_err(|_| ParseError::IntegerOverflow {
                    literal: text,
                    offset: start,
                });
        }

        // Plain integer.
        let text: String = self.src[start..self.pos]
            .chars()
            .filter(|c| *c != '_')
            .collect();
        // Try parsing as i64 first. If it overflows but the value is exactly 2^63
        // (9223372036854775808 = i64::MAX + 1), emit IntMinMagnitude — this token is
        // only valid after unary minus: -9223372036854775808 = i64::MIN.
        text.parse::<i64>()
            .map(TokenKind::IntLiteral)
            .or_else(|_| {
                text.parse::<u64>()
                    .ok()
                    .filter(|&v| v == (i64::MAX as u64) + 1)
                    .map(|_| TokenKind::IntMinMagnitude)
                    .ok_or(())
            })
            .map_err(|_| ParseError::IntegerOverflow {
                literal: self.src[start..self.pos].to_string(),
                offset: start,
            })
    }

    /// Lex a float literal that begins with `.` (e.g. `.1`, `.5e3`).
    /// On entry, `self.pos` points at the leading `.`.
    pub(super) fn lex_dot_float(&mut self, start: usize) -> Result<TokenKind, ParseError> {
        self.pos += 1; // consume the '.'
        while matches!(self.peek_byte(0), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
        if matches!(self.peek_byte(0), Some(b'e') | Some(b'E')) {
            self.pos += 1;
            if matches!(self.peek_byte(0), Some(b'+') | Some(b'-')) {
                self.pos += 1;
            }
            while matches!(self.peek_byte(0), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        let text = &self.src[start..self.pos];
        let val: f64 = text.parse().map_err(|_| ParseError::Syntax {
            offset: start,
            expected: "valid float literal".into(),
            found: text.into(),
        })?;
        Ok(TokenKind::FloatLiteral(val))
    }
}
