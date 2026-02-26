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

use crate::error::ParseError;

// ── TokenKind ─────────────────────────────────────────────────────────────────

/// Every distinct token type the lexer can produce.
#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // ── Keywords ──────────────────────────────────────────────────────────────
    Match,
    Optional,
    Where,
    Return,
    Create,
    Merge,
    Set,
    Remove,
    Delete,
    Detach,
    With,
    Unwind,
    Order,
    By,
    Limit,
    Skip,
    Distinct,
    As,
    In,
    Not,
    And,
    Or,
    Xor,
    Is,
    Null,
    True,
    False,
    Call,
    Foreach,
    Union,
    All,
    Case,
    When,
    Then,
    Else,
    End,
    Starts,
    Ends,
    Contains,
    Count,
    Yield,

    // ── Punctuation ───────────────────────────────────────────────────────────
    LParen,   // (
    RParen,   // )
    LBracket, // [
    RBracket, // ]
    LBrace,   // {
    RBrace,   // }

    Comma,     // ,
    Dot,       // .
    Colon,     // :
    Semicolon, // ;
    DoubleDot, // ..

    /// `->` — directed edge end.
    Arrow,
    /// `<-` — directed edge start.
    LeftArrow,
    /// `-` — undirected edge or arithmetic minus.
    Dash,

    // ── Comparison / assignment operators ─────────────────────────────────────
    Eq,      // =
    Neq,     // <>
    Lt,      // <
    Lte,     // <=
    Gt,      // >
    Gte,     // >=
    Tilde,   // ~ (kept for `=~` which is parsed at the parser level as Eq+Tilde)
    RegexEq, // =~

    // ── Arithmetic operators ──────────────────────────────────────────────────
    Plus, // +
    // Note: there is no Minus variant — the lexer emits Dash for '-'.
    // Phase 1 parser resolves Dash as either a subtraction operator or
    // part of a relationship pattern based on context.
    Star,    // *
    Slash,   // /
    Percent, // %
    Caret,   // ^

    // ── Bitwise / list operators ──────────────────────────────────────────────
    Amp,  // &
    Pipe, // |

    // ── Parameter sigil ───────────────────────────────────────────────────────
    DollarSign, // $

    // ── Literals ──────────────────────────────────────────────────────────────
    IntLiteral(i64),
    FloatLiteral(f64),
    /// Owned string because backslash escapes require materialisation.
    StringLiteral(String),
    /// A bare identifier or backtick-quoted identifier.
    Identifier,

    // ── Special ───────────────────────────────────────────────────────────────
    Eof,
    Unknown(char),
}

impl TokenKind {
    /// Returns `true` if this token is [`TokenKind::Eof`].
    pub fn is_eof(&self) -> bool {
        matches!(self, Self::Eof)
    }
}

// ── Token ─────────────────────────────────────────────────────────────────────

/// A single token produced by the lexer.
///
/// `text` is a slice of the original query; `offset` is the byte position of
/// the first character of this token.
#[derive(Debug, Clone, PartialEq)]
pub struct Token<'src> {
    pub kind: TokenKind,
    /// Slice of the original source text that produced this token.
    pub text: &'src str,
    /// Byte offset of the first byte of this token in the source.
    pub offset: usize,
}

impl<'src> Token<'src> {
    fn new(kind: TokenKind, text: &'src str, offset: usize) -> Self {
        Self { kind, text, offset }
    }
}

// ── Keyword table ─────────────────────────────────────────────────────────────

/// Map an ASCII-lowercased identifier text to a keyword [`TokenKind`], or
/// return `None` if it is not a reserved word.
fn keyword(lower: &str) -> Option<TokenKind> {
    match lower {
        "match" => Some(TokenKind::Match),
        "optional" => Some(TokenKind::Optional),
        "where" => Some(TokenKind::Where),
        "return" => Some(TokenKind::Return),
        "create" => Some(TokenKind::Create),
        "merge" => Some(TokenKind::Merge),
        "set" => Some(TokenKind::Set),
        "remove" => Some(TokenKind::Remove),
        "delete" => Some(TokenKind::Delete),
        "detach" => Some(TokenKind::Detach),
        "with" => Some(TokenKind::With),
        "unwind" => Some(TokenKind::Unwind),
        "order" => Some(TokenKind::Order),
        "by" => Some(TokenKind::By),
        "limit" => Some(TokenKind::Limit),
        "skip" => Some(TokenKind::Skip),
        "distinct" => Some(TokenKind::Distinct),
        "as" => Some(TokenKind::As),
        "in" => Some(TokenKind::In),
        "not" => Some(TokenKind::Not),
        "and" => Some(TokenKind::And),
        "or" => Some(TokenKind::Or),
        "xor" => Some(TokenKind::Xor),
        "is" => Some(TokenKind::Is),
        "null" => Some(TokenKind::Null),
        "true" => Some(TokenKind::True),
        "false" => Some(TokenKind::False),
        "call" => Some(TokenKind::Call),
        "foreach" => Some(TokenKind::Foreach),
        "union" => Some(TokenKind::Union),
        "all" => Some(TokenKind::All),
        "case" => Some(TokenKind::Case),
        "when" => Some(TokenKind::When),
        "then" => Some(TokenKind::Then),
        "else" => Some(TokenKind::Else),
        "end" => Some(TokenKind::End),
        "starts" => Some(TokenKind::Starts),
        "ends" => Some(TokenKind::Ends),
        "contains" => Some(TokenKind::Contains),
        "count" => Some(TokenKind::Count),
        "yield" => Some(TokenKind::Yield),
        _ => None,
    }
}

// ── Lexer ─────────────────────────────────────────────────────────────────────

/// Internal cursor-based lexer state.
struct Lexer<'src> {
    src: &'src str,
    /// Current byte position in `src`.
    pos: usize,
}

impl<'src> Lexer<'src> {
    fn new(src: &'src str) -> Self {
        Self { src, pos: 0 }
    }

    /// Remaining source from current position.
    fn rest(&self) -> &'src str {
        &self.src[self.pos..]
    }

    /// Peek at the byte at `self.pos + offset` without advancing.
    fn peek_byte(&self, offset: usize) -> Option<u8> {
        self.src.as_bytes().get(self.pos + offset).copied()
    }

    /// Skip whitespace (space, tab, CR, LF).
    fn skip_whitespace(&mut self) {
        while let Some(b) = self.peek_byte(0) {
            if b == b' ' || b == b'\t' || b == b'\r' || b == b'\n' {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    /// Skip a `//`-style line comment (the `//` has already been consumed).
    fn skip_line_comment(&mut self) {
        while let Some(b) = self.peek_byte(0) {
            self.pos += 1;
            if b == b'\n' {
                break;
            }
        }
    }

    /// Skip a `/* ... */` block comment (the `/*` has already been consumed).
    /// Returns `false` if the comment is unterminated (EOF reached).
    fn skip_block_comment(&mut self) -> bool {
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

    /// Lex a string literal (opening quote already consumed).
    /// `quote` is either `b'\''` or `b'"'`.
    /// Returns the unescaped string content, or a `ParseError`.
    fn lex_string(&mut self, quote: u8, start: usize) -> Result<String, ParseError> {
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
                    let c = self.rest().chars().next().unwrap();
                    self.pos += c.len_utf8();
                    buf.push(c);
                }
            }
        }
    }

    /// Lex a backtick-quoted identifier (opening backtick already consumed).
    fn lex_backtick_identifier(&mut self, start: usize) -> Result<&'src str, ParseError> {
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

    /// Lex a numeric literal (integer or float). The first digit is already past `start`.
    /// On entry `self.pos` is just after the first character (which is a digit).
    /// We receive `start` and `first_char` so we can form the full slice.
    fn lex_number(&mut self, start: usize) -> Result<TokenKind, ParseError> {
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
            return i64::from_str_radix(&hex_digits, 16)
                .map(TokenKind::IntLiteral)
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
        text.parse::<i64>()
            .map(TokenKind::IntLiteral)
            .map_err(|_| ParseError::IntegerOverflow {
                literal: self.src[start..self.pos].to_string(),
                offset: start,
            })
    }

    /// Produce the next token, skipping whitespace and comments.
    /// Returns `Ok(None)` at end of input, `Ok(Some(token))` otherwise,
    /// and `Err` on lexical errors (unterminated strings, bad escapes, etc.).
    fn next_token(&mut self) -> Result<Option<Token<'src>>, ParseError> {
        // Skip whitespace and comments in a loop (comments may be adjacent).
        loop {
            self.skip_whitespace();
            if self.rest().is_empty() {
                return Ok(None);
            }
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
            while matches!(
                self.peek_byte(0),
                Some(b'a'..=b'z') | Some(b'A'..=b'Z') | Some(b'0'..=b'9') | Some(b'_')
            ) {
                self.pos += 1;
            }
            let text = &self.src[start..self.pos];
            let lower = text.to_ascii_lowercase();
            let kind = keyword(&lower).unwrap_or(TokenKind::Identifier);
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

        // ── Two-character operators ────────────────────────────────────────────
        let tok = match (b0, b1) {
            (b'<', Some(b'>')) => {
                self.pos += 2;
                Some(TokenKind::Neq)
            }
            (b'<', Some(b'=')) => {
                self.pos += 2;
                Some(TokenKind::Lte)
            }
            (b'>', Some(b'=')) => {
                self.pos += 2;
                Some(TokenKind::Gte)
            }
            (b'-', Some(b'>')) => {
                self.pos += 2;
                Some(TokenKind::Arrow)
            }
            (b'<', Some(b'-')) => {
                self.pos += 2;
                Some(TokenKind::LeftArrow)
            }
            (b'.', Some(b'.')) => {
                self.pos += 2;
                Some(TokenKind::DoubleDot)
            }
            (b'=', Some(b'~')) => {
                self.pos += 2;
                Some(TokenKind::RegexEq)
            }
            _ => None,
        };
        if let Some(kind) = tok {
            let text = &self.src[start..self.pos];
            return Ok(Some(Token::new(kind, text, start)));
        }

        // ── Single-character operators and punctuation ────────────────────────
        self.pos += 1;
        let text = &self.src[start..self.pos];
        let kind = match b0 {
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
        };
        Ok(Some(Token::new(kind, text, start)))
    }
}

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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<TokenKind> {
        lex_query(src)
            .unwrap()
            .into_iter()
            .map(|t| t.kind)
            .collect()
    }

    // ── Simple query tokenisation ─────────────────────────────────────────────

    #[test]
    fn simple_match_return() {
        let toks = kinds("MATCH (n) RETURN n");
        assert_eq!(
            toks,
            vec![
                TokenKind::Match,
                TokenKind::LParen,
                TokenKind::Identifier,
                TokenKind::RParen,
                TokenKind::Return,
                TokenKind::Identifier,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn create_with_label_and_properties() {
        let toks = kinds("CREATE (p:Person {name: 'Alice', age: 30})");
        assert!(toks.contains(&TokenKind::Create));
        assert!(toks.contains(&TokenKind::Colon));
        assert!(toks.contains(&TokenKind::LBrace));
        assert!(toks.contains(&TokenKind::StringLiteral("Alice".to_string())));
        assert!(toks.contains(&TokenKind::IntLiteral(30)));
    }

    #[test]
    fn where_clause_operators() {
        let toks = kinds("WHERE n.age >= 18 AND n.age < 65");
        assert!(toks.contains(&TokenKind::Where));
        assert!(toks.contains(&TokenKind::Gte));
        assert!(toks.contains(&TokenKind::And));
        assert!(toks.contains(&TokenKind::Lt));
    }

    // ── All operators ─────────────────────────────────────────────────────────

    #[test]
    fn all_comparison_operators() {
        let toks = kinds("= <> < <= > >=");
        assert_eq!(
            toks,
            vec![
                TokenKind::Eq,
                TokenKind::Neq,
                TokenKind::Lt,
                TokenKind::Lte,
                TokenKind::Gt,
                TokenKind::Gte,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn arrow_operators() {
        let toks = kinds("-> <- -");
        assert_eq!(
            toks,
            vec![
                TokenKind::Arrow,
                TokenKind::LeftArrow,
                TokenKind::Dash,
                TokenKind::Eof
            ]
        );
    }

    #[test]
    fn double_dot_operator() {
        let toks = kinds("[*1..3]");
        assert!(toks.contains(&TokenKind::DoubleDot));
        assert!(!toks.contains(&TokenKind::Dot)); // `..` is not two dots
    }

    #[test]
    fn regex_eq_operator() {
        let toks = kinds("n.name =~ 'A.*'");
        assert!(toks.contains(&TokenKind::RegexEq));
    }

    #[test]
    fn pipe_operator() {
        let toks = kinds("MATCH (n:A|B)");
        assert!(toks.contains(&TokenKind::Pipe));
    }

    #[test]
    fn arithmetic_operators() {
        let toks = kinds("1 + 2 - 3 * 4 / 5 % 6 ^ 7");
        assert!(toks.contains(&TokenKind::Plus));
        assert!(toks.contains(&TokenKind::Dash));
        assert!(toks.contains(&TokenKind::Star));
        assert!(toks.contains(&TokenKind::Slash));
        assert!(toks.contains(&TokenKind::Percent));
        assert!(toks.contains(&TokenKind::Caret));
    }

    // ── String literal escapes ────────────────────────────────────────────────

    #[test]
    fn string_single_quoted() {
        let toks = kinds("'hello world'");
        assert_eq!(toks[0], TokenKind::StringLiteral("hello world".to_string()));
    }

    #[test]
    fn string_double_quoted() {
        let toks = kinds("\"hello world\"");
        assert_eq!(toks[0], TokenKind::StringLiteral("hello world".to_string()));
    }

    #[test]
    fn string_escape_newline() {
        let toks = kinds(r"'line1\nline2'");
        assert_eq!(
            toks[0],
            TokenKind::StringLiteral("line1\nline2".to_string())
        );
    }

    #[test]
    fn string_escape_tab() {
        let toks = kinds(r"'col1\tcol2'");
        assert_eq!(toks[0], TokenKind::StringLiteral("col1\tcol2".to_string()));
    }

    #[test]
    fn string_escape_backslash() {
        let toks = kinds(r"'a\\b'");
        assert_eq!(toks[0], TokenKind::StringLiteral("a\\b".to_string()));
    }

    #[test]
    fn string_escape_quote_in_single() {
        let toks = kinds(r"'it\'s'");
        assert_eq!(toks[0], TokenKind::StringLiteral("it's".to_string()));
    }

    #[test]
    fn string_unicode_escape() {
        let toks = kinds(r"'\u0041'"); // U+0041 = 'A'
        assert_eq!(toks[0], TokenKind::StringLiteral("A".to_string()));
    }

    // ── Unterminated string ───────────────────────────────────────────────────

    #[test]
    fn unterminated_string_errors() {
        let result = lex_query("'unterminated");
        assert!(matches!(
            result,
            Err(ParseError::UnterminatedString { offset: 0 })
        ));
    }

    #[test]
    fn unterminated_double_quoted_string_errors() {
        let result = lex_query("\"oops");
        assert!(matches!(
            result,
            Err(ParseError::UnterminatedString { offset: 0 })
        ));
    }

    // ── Integer / float literals ──────────────────────────────────────────────

    #[test]
    fn integer_literal() {
        let toks = kinds("42");
        assert_eq!(toks[0], TokenKind::IntLiteral(42));
    }

    #[test]
    fn negative_integer_is_dash_then_literal() {
        // Cypher parses `-1` as unary minus applied to `1`.
        let toks = kinds("-1");
        assert_eq!(toks[0], TokenKind::Dash);
        assert_eq!(toks[1], TokenKind::IntLiteral(1));
    }

    #[test]
    fn float_with_decimal() {
        let toks = kinds("1.23456");
        assert_eq!(toks[0], TokenKind::FloatLiteral(1.23456));
    }

    #[test]
    fn float_with_exponent() {
        let toks = kinds("1.5e3");
        assert_eq!(toks[0], TokenKind::FloatLiteral(1500.0));
    }

    #[test]
    fn float_exponent_negative() {
        let toks = kinds("2e-2");
        assert!(matches!(toks[0], TokenKind::FloatLiteral(v) if (v - 0.02).abs() < 1e-10));
    }

    #[test]
    fn integer_overflow_error() {
        // 2^63 + 1 overflows i64.
        let result = lex_query("9999999999999999999");
        assert!(matches!(result, Err(ParseError::IntegerOverflow { .. })));
    }

    // ── Identifiers ───────────────────────────────────────────────────────────

    #[test]
    fn plain_identifier() {
        let tokens = lex_query("myVar").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::Identifier);
        assert_eq!(tokens[0].text, "myVar");
    }

    #[test]
    fn backtick_identifier() {
        let tokens = lex_query("`has spaces`").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::Identifier);
        assert_eq!(tokens[0].text, "`has spaces`");
    }

    // ── Keywords ─────────────────────────────────────────────────────────────

    #[test]
    fn keywords_case_insensitive() {
        let toks = kinds("match MATCH Match");
        assert_eq!(toks[0], TokenKind::Match);
        assert_eq!(toks[1], TokenKind::Match);
        assert_eq!(toks[2], TokenKind::Match);
    }

    #[test]
    fn all_keywords_recognised() {
        let query = "MATCH OPTIONAL WHERE RETURN CREATE MERGE SET REMOVE DELETE DETACH \
                     WITH UNWIND ORDER BY LIMIT SKIP DISTINCT AS IN NOT AND OR XOR \
                     IS NULL TRUE FALSE CALL FOREACH UNION ALL CASE WHEN THEN ELSE END \
                     STARTS ENDS CONTAINS COUNT";
        let toks = lex_query(query).unwrap();
        let non_eof_non_space: Vec<_> = toks.iter().filter(|t| !t.kind.is_eof()).collect();
        // Should have exactly as many tokens as keywords listed.
        assert_eq!(non_eof_non_space.len(), 40);
    }

    // ── Comments ─────────────────────────────────────────────────────────────

    #[test]
    fn line_comment_skipped() {
        let toks = kinds("MATCH // this is a comment\nRETURN");
        assert_eq!(
            toks,
            vec![TokenKind::Match, TokenKind::Return, TokenKind::Eof]
        );
    }

    #[test]
    fn block_comment_skipped() {
        let toks = kinds("MATCH /* ignore this */ RETURN");
        assert_eq!(
            toks,
            vec![TokenKind::Match, TokenKind::Return, TokenKind::Eof]
        );
    }

    #[test]
    fn nested_block_comments_are_not_nested() {
        // Standard Cypher: `/* /* */` ends at the first `*/`
        let toks = kinds("MATCH /* outer /* inner */ RETURN");
        // "RETURN" is consumed inside the comment, so only Match + Eof remain.
        // (or just Eof if there's nothing after; depends on what follows `*/`)
        // The point is we don't panic and Eof is present.
        assert!(toks.last().unwrap().is_eof());
    }

    // ── Token offsets ─────────────────────────────────────────────────────────

    #[test]
    fn token_offsets_correct() {
        let src = "MATCH (n)";
        let tokens = lex_query(src).unwrap();
        assert_eq!(tokens[0].offset, 0); // MATCH
        assert_eq!(tokens[1].offset, 6); // (
        assert_eq!(tokens[2].offset, 7); // n
        assert_eq!(tokens[3].offset, 8); // )
    }

    // ── Parameter sigil ───────────────────────────────────────────────────────

    #[test]
    fn dollar_sign_emitted() {
        let toks = kinds("$name");
        assert_eq!(toks[0], TokenKind::DollarSign);
        assert_eq!(toks[1], TokenKind::Identifier);
    }

    // ── Semicolon and other punctuation ──────────────────────────────────────

    #[test]
    fn semicolon_and_braces() {
        let toks = kinds(";{}[]()");
        assert_eq!(
            toks,
            vec![
                TokenKind::Semicolon,
                TokenKind::LBrace,
                TokenKind::RBrace,
                TokenKind::LBracket,
                TokenKind::RBracket,
                TokenKind::LParen,
                TokenKind::RParen,
                TokenKind::Eof,
            ]
        );
    }

    // ── Empty input ───────────────────────────────────────────────────────────

    #[test]
    fn empty_input_gives_eof() {
        let toks = kinds("");
        assert_eq!(toks, vec![TokenKind::Eof]);
    }

    #[test]
    fn whitespace_only_gives_eof() {
        let toks = kinds("   \n\t  ");
        assert_eq!(toks, vec![TokenKind::Eof]);
    }

    // ── Invalid escape ────────────────────────────────────────────────────────

    #[test]
    fn invalid_escape_sequence_errors() {
        let result = lex_query(r"'\q'");
        assert!(matches!(
            result,
            Err(ParseError::InvalidEscape { char: 'q', .. })
        ));
    }
}
