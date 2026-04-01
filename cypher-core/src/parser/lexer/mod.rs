//! Zero-copy lexer for openCypher / GQL.
//!
//! [`Token`] holds a `&'src str` slice into the original query string, so the
//! lexer allocates nothing except for two cases where owned data is unavoidable:
//! - String literals with backslash escapes produce an owned `String` stored in
//!   [`TokenKind::StringLiteral`].
//! - Float and integer literals store their parsed numeric values directly in
//!   the token kind.
//!
//! All keyword matching is case-insensitive, as required by the Cypher spec.

mod scan;
pub mod tokens;

pub use tokens::{Token, TokenKind};

use crate::error::ParseError;
use scan::Lexer;

// ── Public API ────────────────────────────────────────────────────────────────

/// Tokenize `src` into a [`Vec<Token>`], including a terminal [`TokenKind::Eof`].
///
/// This function does NOT return errors — any unknown characters are emitted as
/// [`TokenKind::Unknown`] and unterminated strings produce an [`TokenKind::Unknown`]
/// token at the opening quote position. Use [`lex_query`] if you need strict
/// error reporting.
pub fn tokenize(src: &str) -> Vec<Token<'_>> {
    let mut lexer = Lexer::new(src);
    let mut tokens = Vec::new();
    loop {
        match lexer.next_token() {
            Ok(Some(tok)) => {
                let is_eof = tok.kind.is_eof();
                tokens.push(tok);
                if is_eof {
                    break;
                }
            }
            Ok(None) => {
                // Push Eof sentinel.
                let pos = lexer.pos;
                tokens.push(Token::new(TokenKind::Eof, &src[pos..pos], pos));
                break;
            }
            Err(_) => {
                // On lexical error emit Unknown and try to keep going.
                let pos = lexer.pos;
                // pos may be at end-of-input (e.g. unterminated string consumed to EOF).
                match src[pos..].chars().next() {
                    Some(ch) => {
                        let end = pos + ch.len_utf8();
                        tokens.push(Token::new(TokenKind::Unknown(ch), &src[pos..end], pos));
                        lexer.pos = end;
                    }
                    None => {
                        // Already at EOF; push Eof sentinel and stop.
                        tokens.push(Token::new(TokenKind::Eof, &src[pos..pos], pos));
                        break;
                    }
                }
            }
        }
    }
    tokens
}

/// Tokenize `src` strictly: return all tokens on success, or the first
/// [`ParseError`] on failure. Also emits a terminal [`TokenKind::Eof`].
pub fn lex_query(src: &str) -> Result<Vec<Token<'_>>, ParseError> {
    let mut lexer = Lexer::new(src);
    let mut tokens = Vec::new();
    loop {
        match lexer.next_token()? {
            Some(tok) => {
                let is_eof = tok.kind.is_eof();
                tokens.push(tok);
                if is_eof {
                    break;
                }
            }
            None => {
                let pos = lexer.pos;
                tokens.push(Token::new(TokenKind::Eof, &src[pos..pos], pos));
                break;
            }
        }
    }
    Ok(tokens)
}
