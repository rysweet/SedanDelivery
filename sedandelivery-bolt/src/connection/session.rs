use std::net::IpAddr;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

use gql_core::storage::GraphStore;

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
    store: Arc<dyn GraphStore>,
    conn_id: u64,
    global_bookmark: Arc<AtomicU64>,
    write_semaphore: Arc<tokio::sync::Semaphore>,
    peer_ip: Option<IpAddr>,
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
        write_semaphore,
        write_permit: None,
        saved_graph: None,
        peer_ip,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::net::{IpAddr, Ipv4Addr};

    use gql_core::storage::InMemoryGraphStore;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    use crate::message::{codec, types};
    use crate::packstream::decoder;
    use crate::packstream::encoder;
    use crate::packstream::value::BoltValue;
    use crate::protocol::handler;
    use crate::protocol::state::ServerState;
    use crate::protocol::tags::{TAG_FAILURE, TAG_HELLO, TAG_SUCCESS};

    const BOLT_MAGIC: [u8; 4] = [0x60, 0x60, 0xB0, 0x17];

    async fn tcp_pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let client = TcpStream::connect(addr).await.unwrap();
        let (server, _) = listener.accept().await.unwrap();
        (client, server)
    }

    fn build_handshake() -> Vec<u8> {
        let mut buf = Vec::with_capacity(20);
        buf.extend_from_slice(&BOLT_MAGIC);
        buf.extend_from_slice(&[0, 0, 4, 4]);
        buf.extend_from_slice(&[0u8; 12]);
        buf
    }

    fn encode_struct(tag: u8, fields: &[BoltValue]) -> Vec<u8> {
        let mut buf = Vec::new();
        encoder::encode(
            &BoltValue::Structure {
                tag,
                fields: fields.to_vec(),
            },
            &mut buf,
        );
        buf
    }

    fn hello_message() -> Vec<u8> {
        encode_struct(
            TAG_HELLO,
            &[BoltValue::Map(vec![
                (
                    "user_agent".to_string(),
                    BoltValue::String("test/1.0".to_string()),
                ),
                ("scheme".to_string(), BoltValue::String("basic".to_string())),
                (
                    "credentials".to_string(),
                    BoltValue::String("wrong".to_string()),
                ),
            ])],
        )
    }

    fn auth_session(peer_ip: Option<IpAddr>) -> Session {
        Session {
            state: ServerState::Authentication,
            store: Arc::new(InMemoryGraphStore::new()),
            conn_id: 1,
            current_graph: "neo4j".to_string(),
            pending_results: HashMap::new(),
            next_qid: 0,
            global_bookmark: Arc::new(AtomicU64::new(0)),
            tx_metadata: Vec::new(),
            tx_timeout: None,
            tx_store: None,
            write_semaphore: Arc::new(tokio::sync::Semaphore::new(128)),
            write_permit: None,
            saved_graph: None,
            peer_ip,
        }
    }

    async fn saturate_auth_limiter(ip: IpAddr) {
        for _ in 0..16 {
            let mut session = auth_session(Some(ip));
            let _ = handler::with_required_password_override_for_tests(Some("secret"), async {
                handler::handle_message(
                    &mut session,
                    types::ClientMessage::Hello(vec![
                        ("scheme".to_string(), BoltValue::String("basic".to_string())),
                        (
                            "credentials".to_string(),
                            BoltValue::String("wrong".to_string()),
                        ),
                    ]),
                )
                .await
                .unwrap()
            })
            .await;
        }
    }

    fn response_code(msg_bytes: &[u8]) -> Option<String> {
        let (value, _) = decoder::decode(msg_bytes).unwrap();
        match value {
            BoltValue::Structure { tag, fields } => match tag {
                TAG_FAILURE => match fields.first() {
                    Some(BoltValue::Map(meta)) => meta
                        .iter()
                        .find(|(key, _)| key == "code")
                        .and_then(|(_, value)| match value {
                            BoltValue::String(code) => Some(code.clone()),
                            _ => None,
                        }),
                    _ => None,
                },
                TAG_SUCCESS => Some("SUCCESS".to_string()),
                other => Some(format!("tag=0x{other:02X}")),
            },
            other => Some(format!("{other:?}")),
        }
    }

    #[tokio::test]
    async fn tcp_peer_ip_reaches_hello_auth_rate_limiter() {
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        saturate_auth_limiter(ip).await;

        let (mut client, server) = tcp_pair().await;
        let session_task = tokio::spawn(async move {
            handler::with_required_password_override_for_tests(Some("secret"), async move {
                run(
                    server,
                    Arc::new(InMemoryGraphStore::new()),
                    7,
                    Arc::new(AtomicU64::new(0)),
                    Arc::new(tokio::sync::Semaphore::new(128)),
                    Some(ip),
                )
                .await
            })
            .await
        });

        client.write_all(&build_handshake()).await.unwrap();
        let mut agreed_version = [0u8; 4];
        client.read_exact(&mut agreed_version).await.unwrap();
        assert_eq!(agreed_version, [0, 0, 4, 4]);

        let mut wire = Vec::new();
        codec::chunk_encode(&hello_message(), &mut wire);
        client.write_all(&wire).await.unwrap();

        let response = codec::chunk_decode(&mut client).await.unwrap();
        assert_eq!(
            response_code(&response).as_deref(),
            Some("Neo.ClientError.Security.AuthenticationRateLimit"),
            "real TCP peers should carry their IP into HELLO auth throttling"
        );

        drop(client);
        session_task.await.unwrap().unwrap();
    }
}
