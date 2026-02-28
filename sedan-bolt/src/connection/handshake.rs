use tokio::io::{AsyncReadExt, AsyncWriteExt};

const BOLT_MAGIC: [u8; 4] = [0x60, 0x60, 0xB0, 0x17];
/// The basic message flow (HELLO, RUN, PULL, RESET, GOODBYE) is identical
/// across all Bolt 4.x versions. We accept any 4.x version proposed by the
/// client, preferring the highest minor version offered.
const SUPPORTED_MAJOR: u8 = 4;
const SUPPORTED_MINOR_MAX: u8 = 4;

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
    // We accept ANY Bolt 4.x version (0..=SUPPORTED_MINOR_MAX).  When the
    // client proposes a range, we agree on the highest version we both
    // support: min(client_minor, SUPPORTED_MINOR_MAX).
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
        if major == SUPPORTED_MAJOR {
            let min_minor = minor.saturating_sub(range);
            // Accept if any version in the client's range overlaps with ours (0..=SUPPORTED_MINOR_MAX)
            if min_minor <= SUPPORTED_MINOR_MAX {
                let agreed_minor = minor.min(SUPPORTED_MINOR_MAX);
                agreed = Some((major, agreed_minor));
                break;
            }
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
