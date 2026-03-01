use tokio::io::{AsyncRead, AsyncReadExt};

/// Maximum chunk data size. The Bolt spec allows up to 65535; we use 16384
/// to keep individual TCP writes manageable.
const MAX_CHUNK_SIZE: usize = 16384;

/// Encode a serialized message into chunked wire format.
///
/// The message bytes are split into chunks of at most `MAX_CHUNK_SIZE` bytes.
/// Each chunk is prefixed with a 2-byte big-endian length header. The message
/// is terminated by a zero-length chunk (`0x00 0x00`).
///
/// If `msg_bytes` is empty, only the terminator is written.
pub fn chunk_encode(msg_bytes: &[u8], out: &mut Vec<u8>) {
    if msg_bytes.is_empty() {
        out.extend_from_slice(&[0x00, 0x00]);
        return;
    }
    for chunk in msg_bytes.chunks(MAX_CHUNK_SIZE) {
        let len = chunk.len() as u16;
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(chunk);
    }
    out.extend_from_slice(&[0x00, 0x00]); // terminator
}

/// Decode a chunked message from an async reader.
///
/// Reads 2-byte big-endian chunk length headers followed by chunk data,
/// concatenating all chunks until a zero-length chunk (`0x00 0x00`) is
/// encountered. Returns the reassembled message bytes.
pub async fn chunk_decode<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Vec<u8>, std::io::Error> {
    let mut message = Vec::new();
    loop {
        let mut len_buf = [0u8; 2];
        reader.read_exact(&mut len_buf).await?;
        let chunk_len = u16::from_be_bytes(len_buf) as usize;
        if chunk_len == 0 {
            break;
        }
        let mut chunk = vec![0u8; chunk_len];
        reader.read_exact(&mut chunk).await?;
        message.extend_from_slice(&chunk);
    }
    Ok(message)
}

/// Synchronous chunk decode, primarily for testing.
///
/// Parses the wire-format chunk framing from a byte slice. Returns the
/// reassembled message bytes or an I/O error on truncation.
pub fn chunk_decode_sync(wire: &[u8]) -> Result<Vec<u8>, std::io::Error> {
    let mut message = Vec::new();
    let mut pos = 0;
    loop {
        if pos + 2 > wire.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "truncated chunk header",
            ));
        }
        let chunk_len = u16::from_be_bytes([wire[pos], wire[pos + 1]]) as usize;
        pos += 2;
        if chunk_len == 0 {
            break;
        }
        if pos + chunk_len > wire.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "truncated chunk data",
            ));
        }
        message.extend_from_slice(&wire[pos..pos + chunk_len]);
        pos += chunk_len;
    }
    Ok(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_chunk_roundtrip() {
        let msg = b"hello bolt";
        let mut wire = Vec::new();
        chunk_encode(msg, &mut wire);

        // Wire format: [00 0A] [hello bolt] [00 00]
        assert_eq!(wire.len(), 2 + msg.len() + 2);
        // First two bytes are the length header (10 = 0x000A)
        assert_eq!(&wire[0..2], &[0x00, 0x0A]);
        // Last two bytes are the terminator
        assert_eq!(&wire[wire.len() - 2..], &[0x00, 0x00]);

        let decoded = chunk_decode_sync(&wire).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn multi_chunk_roundtrip() {
        // Create a message larger than MAX_CHUNK_SIZE so it spans multiple chunks.
        let msg: Vec<u8> = (0..MAX_CHUNK_SIZE + 100).map(|i| (i % 256) as u8).collect();
        let mut wire = Vec::new();
        chunk_encode(&msg, &mut wire);

        // Should have 2 chunks: one of MAX_CHUNK_SIZE and one of 100.
        // Wire size: (2 + MAX_CHUNK_SIZE) + (2 + 100) + 2 = MAX_CHUNK_SIZE + 106
        assert_eq!(wire.len(), MAX_CHUNK_SIZE + 106);

        // First chunk header should be MAX_CHUNK_SIZE
        let first_len = u16::from_be_bytes([wire[0], wire[1]]) as usize;
        assert_eq!(first_len, MAX_CHUNK_SIZE);

        // Second chunk header should be 100
        let second_header_pos = 2 + MAX_CHUNK_SIZE;
        let second_len =
            u16::from_be_bytes([wire[second_header_pos], wire[second_header_pos + 1]]) as usize;
        assert_eq!(second_len, 100);

        let decoded = chunk_decode_sync(&wire).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn empty_message() {
        let msg = b"";
        let mut wire = Vec::new();
        chunk_encode(msg, &mut wire);

        // Only the terminator
        assert_eq!(wire, vec![0x00, 0x00]);

        let decoded = chunk_decode_sync(&wire).unwrap();
        assert!(decoded.is_empty());
    }

    #[test]
    fn chunk_decode_sync_truncated_header() {
        // Only 1 byte -- not enough for a 2-byte header.
        let wire = vec![0x00];
        let err = chunk_decode_sync(&wire).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn chunk_decode_sync_truncated_data() {
        // Header says 10 bytes but only 3 follow.
        let wire = vec![0x00, 0x0A, 0x01, 0x02, 0x03];
        let err = chunk_decode_sync(&wire).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
    }

    #[tokio::test]
    async fn async_single_chunk_roundtrip() {
        let msg = b"async bolt message";
        let mut wire = Vec::new();
        chunk_encode(msg, &mut wire);

        let mut cursor = std::io::Cursor::new(wire);
        let decoded = chunk_decode(&mut cursor).await.unwrap();
        assert_eq!(decoded, msg);
    }

    #[tokio::test]
    async fn async_multi_chunk_roundtrip() {
        let msg: Vec<u8> = (0..MAX_CHUNK_SIZE * 2 + 50)
            .map(|i| (i % 256) as u8)
            .collect();
        let mut wire = Vec::new();
        chunk_encode(&msg, &mut wire);

        let mut cursor = std::io::Cursor::new(wire);
        let decoded = chunk_decode(&mut cursor).await.unwrap();
        assert_eq!(decoded, msg);
    }

    #[tokio::test]
    async fn async_empty_message() {
        let mut wire = Vec::new();
        chunk_encode(b"", &mut wire);

        let mut cursor = std::io::Cursor::new(wire);
        let decoded = chunk_decode(&mut cursor).await.unwrap();
        assert!(decoded.is_empty());
    }

    #[test]
    fn exact_chunk_boundary() {
        // Message that is exactly MAX_CHUNK_SIZE should produce one data chunk + terminator.
        let msg: Vec<u8> = vec![0xAB; MAX_CHUNK_SIZE];
        let mut wire = Vec::new();
        chunk_encode(&msg, &mut wire);

        // Wire: (2 + MAX_CHUNK_SIZE) + 2 terminator
        assert_eq!(wire.len(), 2 + MAX_CHUNK_SIZE + 2);

        let decoded = chunk_decode_sync(&wire).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn known_wire_bytes() {
        // Verify against spec example: RECORD [1] -> B1 71 91 01
        // Chunked: 00 04 B1 71 91 01 00 00
        let wire = vec![0x00, 0x04, 0xB1, 0x71, 0x91, 0x01, 0x00, 0x00];
        let decoded = chunk_decode_sync(&wire).unwrap();
        assert_eq!(decoded, vec![0xB1, 0x71, 0x91, 0x01]);
    }

    /// Test that a 100K-byte message roundtrips through chunking correctly.
    #[test]
    fn large_message_100k_roundtrip() {
        let msg: Vec<u8> = (0..100_000).map(|i| (i % 256) as u8).collect();
        let mut wire = Vec::new();
        chunk_encode(&msg, &mut wire);

        // Verify chunk count: ceil(100_000 / 16384) = 7 chunks.
        let expected_chunks = 100_000_usize.div_ceil(MAX_CHUNK_SIZE);
        assert_eq!(expected_chunks, 7);

        // Wire size: sum of (2 + chunk_data_len) for each chunk + 2 terminator.
        let expected_wire_len = expected_chunks * 2 + 100_000 + 2;
        assert_eq!(wire.len(), expected_wire_len);

        let decoded = chunk_decode_sync(&wire).unwrap();
        assert_eq!(decoded.len(), 100_000);
        assert_eq!(decoded, msg);
    }

    /// Test that a 500K-byte message roundtrips through chunking correctly.
    #[test]
    fn large_message_500k_roundtrip() {
        let msg: Vec<u8> = (0..500_000).map(|i| (i % 256) as u8).collect();
        let mut wire = Vec::new();
        chunk_encode(&msg, &mut wire);

        let decoded = chunk_decode_sync(&wire).unwrap();
        assert_eq!(decoded.len(), 500_000);
        assert_eq!(decoded, msg);
    }

    /// Test async decode of a 100K-byte message (simulates server-to-client path).
    #[tokio::test]
    async fn async_large_message_100k_roundtrip() {
        let msg: Vec<u8> = (0..100_000).map(|i| (i % 256) as u8).collect();
        let mut wire = Vec::new();
        chunk_encode(&msg, &mut wire);

        let mut cursor = std::io::Cursor::new(wire);
        let decoded = chunk_decode(&mut cursor).await.unwrap();
        assert_eq!(decoded.len(), 100_000);
        assert_eq!(decoded, msg);
    }

    /// Test async decode of a 500K-byte message.
    #[tokio::test]
    async fn async_large_message_500k_roundtrip() {
        let msg: Vec<u8> = (0..500_000).map(|i| (i % 256) as u8).collect();
        let mut wire = Vec::new();
        chunk_encode(&msg, &mut wire);

        let mut cursor = std::io::Cursor::new(wire);
        let decoded = chunk_decode(&mut cursor).await.unwrap();
        assert_eq!(decoded.len(), 500_000);
        assert_eq!(decoded, msg);
    }
}
