use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum ParseError {
    #[error("syntax error at byte {offset}: expected {expected}, found {found}")]
    Syntax {
        offset: usize,
        expected: String,
        found: String,
    },

    #[error("unterminated string literal starting at byte {offset}")]
    UnterminatedString { offset: usize },

    #[error("integer overflow in literal '{literal}' at byte {offset}")]
    IntegerOverflow { literal: String, offset: usize },

    #[error("invalid escape sequence '\\{char}' at byte {offset}")]
    InvalidEscape { char: char, offset: usize },

    #[error("query too large: {size} bytes, limit is {limit} bytes")]
    QueryTooLarge { size: usize, limit: usize },

    #[error("expression nesting depth exceeded limit of {limit}")]
    RecursionLimitExceeded { limit: u32 },

    #[error("empty query")]
    EmptyQuery,
}
