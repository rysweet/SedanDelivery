use super::*;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};

use cypher_core::storage::InMemoryGraphStore;
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
                Some(BoltValue::Map(meta)) => {
                    meta.iter()
                        .find(|(key, _)| key == "code")
                        .and_then(|(_, value)| match value {
                            BoltValue::String(code) => Some(code.clone()),
                            _ => None,
                        })
                }
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
