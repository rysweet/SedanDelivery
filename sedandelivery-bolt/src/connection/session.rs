use std::net::IpAddr;
use std::sync::Arc;

use crate::bookmark::BookmarkCounter;
use std::time::Duration;

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
    /// The connection was closed because it exceeded the idle timeout
    /// (no messages received within the configured window).
    #[error("idle timeout ({0:?})")]
    IdleTimeout(Duration),
    /// The connection was closed because it exceeded the maximum connection
    /// lifetime, which forces periodic reconnection for L4 load-balancer
    /// rebalancing after scale-out events.
    #[error("max connection lifetime exceeded ({0:?})")]
    MaxLifetimeExceeded(Duration),
}

/// Configuration for a single Bolt session.
///
/// Groups the parameters needed by [`run()`] into a single struct so that
/// adding new options does not require changing every call site.
pub struct SessionConfig {
    // -- Identity --
    /// Shared graph store backing this session.
    pub store: Arc<dyn GraphStore>,
    /// Unique connection identifier for logging and metrics.
    pub conn_id: u64,
    /// Peer IP address used for auth rate limiting and logging.
    pub peer_ip: Option<IpAddr>,

    // -- Shared state --
    /// Global bookmark counter shared across all sessions connected to the
    /// same store so bookmarks produced by one session are meaningful when
    /// passed to another.
    pub global_bookmark: Arc<BookmarkCounter>,
    /// Write concurrency semaphore for transaction timeout contention.
    pub write_semaphore: Arc<tokio::sync::Semaphore>,

    // -- Timeouts --
    /// Maximum time to wait for the next client message before closing the
    /// connection.  The timer resets after every received message.
    pub idle_timeout: Duration,
    /// Hard wall-clock cap on the entire connection measured from session
    /// start (after handshake).  `None` disables the limit.
    pub max_connection_lifetime: Option<Duration>,
}

/// Run a complete Bolt session on the given stream.
///
/// Performs the version-negotiation handshake, then enters the message loop:
/// reads chunked messages from the client, dispatches them through the
/// protocol handler, and writes back the encoded responses.  The loop
/// terminates on GOODBYE, connection close, a Defunct state transition,
/// idle timeout, or max connection lifetime expiry.
///
/// ## Timeout behaviour
///
/// * **`idle_timeout`** -- maximum time to wait for the *next* client message.
///   The timer resets after every successfully received message so active
///   sessions are not killed.  Default 300 s.
///
/// * **`max_connection_lifetime`** -- hard wall-clock cap on the entire
///   connection measured from session start (after handshake).  When set, the
///   connection is closed regardless of activity.  This is critical for L4
///   load balancers (e.g. Envoy) which pin connections to a backend at
///   connect-time: without a lifetime cap, new replicas added during
///   scale-out never receive traffic from long-lived connections.
///
/// The stream is generic over any type implementing `AsyncRead + AsyncWrite`,
/// which allows it to work with both plaintext TCP streams and TLS-wrapped
/// connections.
pub async fn run<S>(stream: S, config: SessionConfig) -> Result<(), SessionError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let SessionConfig {
        store,
        conn_id,
        peer_ip,
        global_bookmark,
        write_semaphore,
        idle_timeout,
        max_connection_lifetime,
    } = config;
    let (read_half, write_half) = tokio::io::split(stream);
    let mut reader = BufReader::new(read_half);
    let mut writer = write_half;

    // Handshake (not subject to idle timeout -- handshake has its own timeout
    // semantics and we don't want to penalise slow TLS negotiation).
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
        tx_had_write: false,
        peer_ip,
        last_write_at: None,
    };

    // Absolute deadline for the connection lifetime (if configured).
    let lifetime_deadline = max_connection_lifetime.map(|d| tokio::time::Instant::now() + d);

    // Message loop -- each iteration waits for the next message with an idle
    // timeout, and also checks the absolute lifetime deadline.
    loop {
        if session.state == ServerState::Defunct {
            break;
        }

        // Build the future that reads the next chunked message.
        let read_fut = codec::chunk_decode(&mut reader);

        // Apply idle timeout, then lifetime deadline on top.
        let msg_bytes = if let Some(deadline) = lifetime_deadline {
            match tokio::time::timeout_at(deadline, tokio::time::timeout(idle_timeout, read_fut))
                .await
            {
                // Lifetime expired.
                Err(_) => {
                    let lifetime = max_connection_lifetime.unwrap();
                    tracing::info!(
                        conn_id,
                        lifetime_secs = lifetime.as_secs(),
                        "closing connection: max lifetime exceeded \
                         (forces reconnect for L4 load-balancer rebalancing)"
                    );
                    return Err(SessionError::MaxLifetimeExceeded(lifetime));
                }
                // Idle timeout expired (but lifetime not yet).
                Ok(Err(_)) => {
                    tracing::info!(
                        conn_id,
                        idle_secs = idle_timeout.as_secs(),
                        "closing connection: idle timeout"
                    );
                    return Err(SessionError::IdleTimeout(idle_timeout));
                }
                // Got bytes (or IO error).
                Ok(Ok(result)) => result,
            }
        } else {
            // No lifetime cap -- just apply idle timeout.
            match tokio::time::timeout(idle_timeout, read_fut).await {
                Err(_) => {
                    tracing::info!(
                        conn_id,
                        idle_secs = idle_timeout.as_secs(),
                        "closing connection: idle timeout"
                    );
                    return Err(SessionError::IdleTimeout(idle_timeout));
                }
                Ok(result) => result,
            }
        };

        let msg_bytes = match msg_bytes {
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
    use indexmap::IndexMap;
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
            global_bookmark: Arc::new(BookmarkCounter::from_env()),
            tx_metadata: Vec::new(),
            tx_timeout: None,
            tx_store: None,
            write_semaphore: Arc::new(tokio::sync::Semaphore::new(128)),
            write_permit: None,
            saved_graph: None,
            tx_had_write: false,
            peer_ip,
            last_write_at: None,
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
                    SessionConfig {
                        store: Arc::new(InMemoryGraphStore::new()),
                        conn_id: 7,
                        peer_ip: Some(ip),
                        global_bookmark: Arc::new(BookmarkCounter::from_env()),
                        write_semaphore: Arc::new(tokio::sync::Semaphore::new(128)),
                        idle_timeout: Duration::from_secs(300),
                        max_connection_lifetime: None,
                    },
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
