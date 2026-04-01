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
    Load,
    Csv,
    Headers,
    From,

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
    /// The value 9223372036854775808 (2^63), which is only valid as the operand
    /// of unary minus: `-9223372036854775808 = i64::MIN`. Alone it overflows.
    IntMinMagnitude,

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
    pub(crate) fn new(kind: TokenKind, text: &'src str, offset: usize) -> Self {
        Self { kind, text, offset }
    }
}

// ── Keyword table ─────────────────────────────────────────────────────────────

/// Map an ASCII-lowercased identifier text to a keyword [`TokenKind`], or
/// return `None` if it is not a reserved word.
pub(crate) fn keyword(lower: &str) -> Option<TokenKind> {
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
        "load" => Some(TokenKind::Load),
        "csv" => Some(TokenKind::Csv),
        "headers" => Some(TokenKind::Headers),
        // "from" is NOT a reserved keyword — it's only contextual in LOAD CSV.
        // Reserving it breaks $from as a parameter name (TCK tests use this).
        // The LOAD CSV parser matches it as an Identifier instead.
        _ => None,
    }
}
