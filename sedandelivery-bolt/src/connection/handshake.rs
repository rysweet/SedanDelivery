use tokio::io::{AsyncReadExt, AsyncWriteExt};

const BOLT_MAGIC: [u8; 4] = [0x60, 0x60, 0xB0, 0x17];
/// The basic message flow (HELLO, RUN, PULL, RESET, GOODBYE) is identical
/// across all Bolt 4.x versions. We accept any 4.x version proposed by the
/// client, preferring the highest minor version offered.
const SUPPORTED_VERSIONS: &[(u8, u8)] = &[
    (4, 4), // Bolt 4.0 through 4.4
];

#[derive(Debug, thiserror::Error)]
pub enum HandshakeError {
    #[error("bad magic preamble: {0:?}")]
    BadMagic([u8; 4]),
    #[error("no supported protocol version")]
    NoSupportedVersion,
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

pub async fn perform<R, W>(reader: &mut R, writer: &mut W) -> Result<(u8, u8), HandshakeError>
where
    R: AsyncReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    // Read 4-byte magic preamble
    let mut preamble = [0u8; 4];
    reader.read_exact(&mut preamble).await?;
    if preamble != BOLT_MAGIC {
        return Err(HandshakeError::BadMagic(preamble));
    }

    // Read 4 version proposals (16 bytes)
    let mut versions = [0u8; 16];
    reader.read_exact(&mut versions).await?;

    // Find a supported version.
    // Each 4-byte slot is: [padding, range, minor, major].
    // The range field indicates the client supports minor versions
    // from (minor - range) through minor (inclusive).
    //
    // We try each client proposal against our supported versions,
    // preferring the first match (client proposals are highest-first).
    let mut agreed: Option<(u8, u8)> = None;
    for i in 0..4 {
        let offset = i * 4;
        let range = versions[offset + 1];
        let minor = versions[offset + 2];
        let major = versions[offset + 3];
        tracing::debug!(slot = i, major, minor, range, "version proposal");
        if major == 0 && minor == 0 {
            continue;
        }
        let min_minor = minor.saturating_sub(range);
        // Check against each supported version
        for &(sup_major, sup_minor_max) in SUPPORTED_VERSIONS {
            if major == sup_major && min_minor <= sup_minor_max {
                let agreed_minor = minor.min(sup_minor_max);
                agreed = Some((major, agreed_minor));
                break;
            }
        }
        if agreed.is_some() {
            break;
        }
    }

    match agreed {
        Some((major, minor)) => {
            writer.write_all(&[0u8, 0u8, minor, major]).await?;
            writer.flush().await?;
            Ok((major, minor))
        }
        None => {
            writer.write_all(&[0u8; 4]).await?;
            writer.flush().await?;
            Err(HandshakeError::NoSupportedVersion)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a 20-byte handshake payload: 4-byte preamble + 4 version slots.
    fn build_handshake(slots: [[u8; 4]; 4]) -> Vec<u8> {
        let mut buf = Vec::with_capacity(20);
        buf.extend_from_slice(&BOLT_MAGIC);
        for slot in &slots {
            buf.extend_from_slice(slot);
        }
        buf
    }

    #[tokio::test]
    async fn handshake_bolt_4_4_exact() {
        // Client proposes exactly Bolt 4.4 in the first slot.
        // Slot format: [padding, range, minor, major]
        let input = build_handshake([
            [0, 0, 4, 4], // Bolt 4.4 (range=0 means only 4.4)
            [0, 0, 0, 0],
            [0, 0, 0, 0],
            [0, 0, 0, 0],
        ]);
        let mut reader = &input[..];
        let mut writer = Vec::new();
        let result = perform(&mut reader, &mut writer).await.unwrap();
        assert_eq!(result, (4, 4));
        // Server should reply with [0, 0, 4, 4].
        assert_eq!(writer, vec![0, 0, 4, 4]);
    }

    #[tokio::test]
    async fn handshake_bolt_4_range() {
        // Client proposes Bolt 4.4 with range=4 (supports 4.0 through 4.4).
        let input = build_handshake([[0, 4, 4, 4], [0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0]]);
        let mut reader = &input[..];
        let mut writer = Vec::new();
        let result = perform(&mut reader, &mut writer).await.unwrap();
        assert_eq!(result, (4, 4));
    }

    #[tokio::test]
    async fn handshake_bolt_4_3_only() {
        // Client proposes only 4.3 (range=0). Our server supports 4.0-4.4, so
        // should agree on 4.3 (min_minor=3, sup_minor_max=4 => agreed_minor = min(3,4) = 3).
        let input = build_handshake([[0, 0, 3, 4], [0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0]]);
        let mut reader = &input[..];
        let mut writer = Vec::new();
        let result = perform(&mut reader, &mut writer).await.unwrap();
        assert_eq!(result, (4, 3));
        assert_eq!(writer, vec![0, 0, 3, 4]);
    }

    #[tokio::test]
    async fn handshake_bolt_4_0_only() {
        let input = build_handshake([
            [0, 0, 0, 4], // Bolt 4.0 exact
            [0, 0, 0, 0],
            [0, 0, 0, 0],
            [0, 0, 0, 0],
        ]);
        let mut reader = &input[..];
        let mut writer = Vec::new();
        let result = perform(&mut reader, &mut writer).await.unwrap();
        assert_eq!(result, (4, 0));
        assert_eq!(writer, vec![0, 0, 0, 4]);
    }

    #[tokio::test]
    async fn handshake_no_supported_version() {
        // Client only proposes Bolt 5.0, which we don't support.
        let input = build_handshake([[0, 0, 0, 5], [0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0]]);
        let mut reader = &input[..];
        let mut writer = Vec::new();
        let result = perform(&mut reader, &mut writer).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            HandshakeError::NoSupportedVersion => {}
            other => panic!("expected NoSupportedVersion, got {:?}", other),
        }
        // Server should reply with all zeros.
        assert_eq!(writer, vec![0, 0, 0, 0]);
    }

    #[tokio::test]
    async fn handshake_bad_magic() {
        // Wrong preamble bytes.
        let mut input = vec![0x00, 0x00, 0x00, 0x00];
        input.extend_from_slice(&[0u8; 16]); // version slots
        let mut reader = &input[..];
        let mut writer = Vec::new();
        let result = perform(&mut reader, &mut writer).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            HandshakeError::BadMagic(preamble) => {
                assert_eq!(preamble, [0, 0, 0, 0]);
            }
            other => panic!("expected BadMagic, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn handshake_second_slot_fallback() {
        // First slot is unsupported (Bolt 5.0), second slot is Bolt 4.4.
        let input = build_handshake([
            [0, 0, 0, 5], // unsupported
            [0, 0, 4, 4], // supported
            [0, 0, 0, 0],
            [0, 0, 0, 0],
        ]);
        let mut reader = &input[..];
        let mut writer = Vec::new();
        let result = perform(&mut reader, &mut writer).await.unwrap();
        assert_eq!(result, (4, 4));
    }

    #[tokio::test]
    async fn handshake_all_slots_empty() {
        // All version slots are zeros.
        let input = build_handshake([[0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0]]);
        let mut reader = &input[..];
        let mut writer = Vec::new();
        let result = perform(&mut reader, &mut writer).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            HandshakeError::NoSupportedVersion => {}
            other => panic!("expected NoSupportedVersion, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn handshake_truncated_input() {
        // Only 4 bytes (magic) with no version slots.
        let input = BOLT_MAGIC.to_vec();
        let mut reader = &input[..];
        let mut writer = Vec::new();
        let result = perform(&mut reader, &mut writer).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            HandshakeError::Io(_) => {}
            other => panic!("expected Io error, got {:?}", other),
        }
    }

    #[test]
    fn handshake_error_display() {
        let err = HandshakeError::BadMagic([0xDE, 0xAD, 0xBE, 0xEF]);
        let msg = format!("{}", err);
        assert!(msg.contains("bad magic preamble"));

        let err = HandshakeError::NoSupportedVersion;
        let msg = format!("{}", err);
        assert!(msg.contains("no supported protocol version"));
    }
}
