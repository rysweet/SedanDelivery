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
        let min_minor = if range > minor { 0 } else { minor - range };
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
