use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use tokio::io::{AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

use gql_core::storage::InMemoryGraphStore;

use crate::connection::handshake;
use crate::message::types::ClientMessage;
use crate::message::{codec, types};
use crate::protocol::handler::{self, Session};
use crate::protocol::state::ServerState;

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("handshake error: {0}")]
    Handshake(#[from] handshake::HandshakeError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("message decode error: {0}")]
    MessageDecode(#[from] types::MessageError),
    #[error("protocol error: {0}")]
    Protocol(#[from] handler::ProtocolError),
}

pub async fn run(
    stream: TcpStream,
    store: Arc<InMemoryGraphStore>,
    conn_id: u64,
    global_bookmark: Arc<AtomicU64>,
    write_lock: Arc<tokio::sync::Mutex<()>>,
) -> Result<(), SessionError> {
    let (read_half, write_half) = stream.into_split();
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
