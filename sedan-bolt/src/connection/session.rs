use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

use gql_core::storage::InMemoryGraphStore;

use crate::connection::handshake;
use crate::message::types::ClientMessage;
use crate::message::{codec, types};
use crate::protocol::handler::{self, Session};
use crate::protocol::state::ServerState;

/// Errors that can occur during a Bolt session.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// The version-negotiation handshake failed.
    #[error("handshake error: {0}")]
    Handshake(#[from] handshake::HandshakeError),
    /// A low-level I/O error on the TCP stream.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// A received message could not be decoded.
    #[error("message decode error: {0}")]
    MessageDecode(#[from] types::MessageError),
    /// The protocol handler rejected the message.
    #[error("protocol error: {0}")]
    Protocol(#[from] handler::ProtocolError),
}

/// Run a complete Bolt session on the given stream.
///
/// Performs the version-negotiation handshake, then enters the message loop:
/// reads chunked messages from the client, dispatches them through the
/// protocol handler, and writes back the encoded responses.  The loop
/// terminates on GOODBYE, connection close, or a Defunct state transition.
///
/// The stream is generic over any type implementing `AsyncRead + AsyncWrite`,
/// which allows it to work with both plaintext TCP streams and TLS-wrapped
/// connections.
pub async fn run<S>(
    stream: S,
    store: Arc<InMemoryGraphStore>,
    conn_id: u64,
    global_bookmark: Arc<AtomicU64>,
    write_lock: Arc<tokio::sync::Mutex<()>>,
) -> Result<(), SessionError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (read_half, write_half) = tokio::io::split(stream);
    let mut reader = BufReader::new(read_half);
    let mut writer = write_half;

    // Handshake
    let version = handshake::perform(&mut reader, &mut writer).await?;
    tracing::info!(conn_id, ?version, "handshake complete");

    let mut session = Session {
        state: ServerState::Authentication,
        store,
        conn_id,
        current_graph: "neo4j".to_string(),
        pending_results: std::collections::HashMap::new(),
        next_qid: 0,
        global_bookmark,
        tx_metadata: Vec::new(),
        tx_timeout: None,
        tx_store: None,
        write_lock,
        write_lock_guard: None,
    };

    // Message loop
    loop {
        if session.state == ServerState::Defunct {
            break;
        }

        let msg_bytes = match codec::chunk_decode(&mut reader).await {
            Ok(b) if b.is_empty() => break,
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(SessionError::Io(e)),
        };

        let client_msg = types::decode_client_message(&msg_bytes)?;

        if matches!(client_msg, ClientMessage::Goodbye) {
            tracing::debug!(conn_id, "received GOODBYE");
            break;
        }

        let responses = handler::handle_message(&mut session, client_msg).await?;
        for msg in responses {
            let encoded = types::encode_server_message(&msg);
            let mut wire = Vec::new();
            codec::chunk_encode(&encoded, &mut wire);
            writer.write_all(&wire).await?;
        }
        writer.flush().await?;
    }

    Ok(())
}
