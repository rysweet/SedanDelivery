use crate::packstream::decoder;

/// Errors that can occur when decoding a Bolt message.
#[derive(Debug, thiserror::Error)]
pub enum MessageError {
    /// The underlying PackStream data could not be decoded.
    #[error("packstream decode error: {0}")]
    Decode(#[from] decoder::DecodeError),

    /// The top-level PackStream value is not a Structure.
    #[error("expected Structure, got {0}")]
    NotAStructure(String),

    /// The structure tag byte does not match any known message type.
    #[error("unknown message tag: 0x{0:02X}")]
    UnknownTag(u8),

    /// The structure has the wrong number of fields for its tag.
    #[error("wrong field count for tag 0x{tag:02X}: expected {expected}, got {actual}")]
    WrongFieldCount {
        tag: u8,
        expected: usize,
        actual: usize,
    },

    /// A field has an unexpected PackStream type.
    #[error("expected {expected} in field {field}, got {actual}")]
    UnexpectedFieldType {
        field: usize,
        expected: &'static str,
        actual: String,
    },
}
