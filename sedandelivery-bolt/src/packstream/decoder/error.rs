/// Errors that can occur while decoding PackStream data.
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("unexpected end of input: needed {needed} bytes at offset {offset} but only {available} available")]
    UnexpectedEof {
        offset: usize,
        needed: usize,
        available: usize,
    },
    #[error("invalid marker byte 0x{0:02X}")]
    InvalidMarker(u8),
    #[error("UTF-8 error: {0}")]
    Utf8Error(#[from] std::string::FromUtf8Error),
    #[error("structure has too many fields: {0}")]
    TooManyFields(usize),
    #[error("map key is not a string at offset {0}")]
    NonStringMapKey(usize),
    #[error("nesting depth exceeds maximum ({0})")]
    MaxDepthExceeded(usize),
}

/// Check that `buf[offset..]` has at least `needed` bytes.
pub(super) fn need(buf: &[u8], offset: usize, needed: usize) -> Result<(), DecodeError> {
    let available = buf.len().saturating_sub(offset);
    if available < needed {
        Err(DecodeError::UnexpectedEof {
            offset,
            needed,
            available,
        })
    } else {
        Ok(())
    }
}
