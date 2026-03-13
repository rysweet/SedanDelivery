//! Full-stack Bolt integration tests.
//!
//! Starts a real Bolt TCP server backed by `InMemoryGraphStore`, then connects
//! with a raw `TcpStream` and exercises the handshake, HELLO, RUN+PULL, and
//! GOODBYE flows using the crate's own PackStream encoder/decoder.

use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

use gql_core::storage::InMemoryGraphStore;
use sedandelivery_bolt::bookmark::BookmarkCounter;
use sedandelivery_bolt::connection::session::SessionConfig;
use sedandelivery_bolt::packstream::decoder;
use sedandelivery_bolt::packstream::encoder;
use sedandelivery_bolt::packstream::value::{BoltMap, BoltMapExt, BoltValue};

// ---------------------------------------------------------------------------
// Bolt protocol constants
// ---------------------------------------------------------------------------

const BOLT_MAGIC: [u8; 4] = [0x60, 0x60, 0xB0, 0x17];

const TAG_HELLO: u8 = 0x01;
const TAG_GOODBYE: u8 = 0x02;
const TAG_RUN: u8 = 0x10;
const TAG_PULL: u8 = 0x3F;

const TAG_SUCCESS: u8 = 0x70;
const TAG_RECORD: u8 = 0x71;
const TAG_FAILURE: u8 = 0x7F;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Start a Bolt server on a random port with an in-memory store.
/// Returns the server task handle and the port it bound to.
async fn start_test_server() -> (JoinHandle<()>, u16) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind test listener");
    let port = listener.local_addr().unwrap().port();

    let store: Arc<dyn gql_core::storage::GraphStore> = Arc::new(InMemoryGraphStore::new());
    store
        .create_graph("neo4j")
        .expect("failed to create default database");

    let global_bookmark = Arc::new(BookmarkCounter::from_env());
    let write_semaphore = Arc::new(tokio::sync::Semaphore::new(128));

    let handle = tokio::spawn(async move {
        // Accept connections until the listener is dropped or an error occurs.
        loop {
            let (stream, peer_addr) = match listener.accept().await {
                Ok(pair) => pair,
                Err(_) => break,
            };
            let store = Arc::clone(&store);
            let bookmark = Arc::clone(&global_bookmark);
            let ws = Arc::clone(&write_semaphore);
            tokio::spawn(async move {
                let _ = sedandelivery_bolt::connection::session::run(
                    stream,
                    SessionConfig {
                        store,
                        conn_id: 0,
                        peer_ip: Some(peer_addr.ip()),
                        global_bookmark: bookmark,
                        write_semaphore: ws,
                        idle_timeout: std::time::Duration::from_secs(300),
                        max_connection_lifetime: None,
                    },
                )
                .await;
            });
        }
    });

    (handle, port)
}

/// Connect a raw TCP stream to the test server.
async fn connect(port: u16) -> TcpStream {
    TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .expect("failed to connect to test server")
}

/// Send the Bolt handshake proposing v4.4 and return the negotiated version.
async fn bolt_handshake(stream: &mut TcpStream) -> (u8, u8) {
    // Preamble
    stream.write_all(&BOLT_MAGIC).await.unwrap();
    // Version proposals: slot 0 = Bolt 4.4 with range 4 (supports 4.0-4.4)
    let versions: [u8; 16] = [
        0, 4, 4, 4, // Bolt 4.0-4.4
        0, 0, 0, 0, // empty
        0, 0, 0, 0, // empty
        0, 0, 0, 0, // empty
    ];
    stream.write_all(&versions).await.unwrap();
    stream.flush().await.unwrap();

    // Read 4-byte version response
    let mut resp = [0u8; 4];
    stream.read_exact(&mut resp).await.unwrap();
    (resp[3], resp[2]) // (major, minor)
}

/// Encode a PackStream structure into chunked wire format.
fn encode_message(tag: u8, fields: &[BoltValue]) -> Vec<u8> {
    let val = BoltValue::Structure {
        tag,
        fields: fields.to_vec(),
    };
    let mut ps_bytes = Vec::new();
    encoder::encode(&val, &mut ps_bytes);

    // Chunk-encode: single chunk header + data + terminator
    let mut wire = Vec::new();
    let len = ps_bytes.len() as u16;
    wire.extend_from_slice(&len.to_be_bytes());
    wire.extend_from_slice(&ps_bytes);
    wire.extend_from_slice(&[0x00, 0x00]); // terminator
    wire
}

/// Read one chunked message from the stream and decode it as a BoltValue structure.
/// Returns (tag, fields).
async fn read_message(stream: &mut TcpStream) -> (u8, Vec<BoltValue>) {
    // Reassemble chunks
    let mut message = Vec::new();
    loop {
        let mut len_buf = [0u8; 2];
        stream.read_exact(&mut len_buf).await.unwrap();
        let chunk_len = u16::from_be_bytes(len_buf) as usize;
        if chunk_len == 0 {
            break;
        }
        let mut chunk = vec![0u8; chunk_len];
        stream.read_exact(&mut chunk).await.unwrap();
        message.extend_from_slice(&chunk);
    }

    let (val, _) = decoder::decode(&message).expect("failed to decode server message");
    match val {
        BoltValue::Structure { tag, fields } => (tag, fields),
        other => panic!("expected Structure from server, got {:?}", other),
    }
}

/// Send HELLO with a basic user-agent and no auth.
async fn send_hello(stream: &mut TcpStream) {
    let extra = BoltValue::Map(vec![
        (
            "user_agent".to_string(),
            BoltValue::String("bolt-integration-test/1.0".to_string()),
        ),
        ("scheme".to_string(), BoltValue::String("none".to_string())),
    ]);
    let wire = encode_message(TAG_HELLO, &[extra]);
    stream.write_all(&wire).await.unwrap();
    stream.flush().await.unwrap();
}

/// Send RUN with a query, empty params, and empty extra.
async fn send_run(stream: &mut TcpStream, query: &str) {
    let wire = encode_message(
        TAG_RUN,
        &[
            BoltValue::String(query.to_string()),
            BoltValue::Map(vec![]), // params
            BoltValue::Map(vec![]), // extra
        ],
    );
    stream.write_all(&wire).await.unwrap();
    stream.flush().await.unwrap();
}

/// Send PULL with n=-1 (fetch all).
async fn send_pull_all(stream: &mut TcpStream) {
    let extra = BoltValue::Map(vec![("n".to_string(), BoltValue::Int(-1))]);
    let wire = encode_message(TAG_PULL, &[extra]);
    stream.write_all(&wire).await.unwrap();
    stream.flush().await.unwrap();
}

/// Send GOODBYE (no fields).
async fn send_goodbye(stream: &mut TcpStream) {
    let wire = encode_message(TAG_GOODBYE, &[]);
    stream.write_all(&wire).await.unwrap();
    stream.flush().await.unwrap();
}

/// Extract the BoltMap entries from a SUCCESS fields list.
fn extract_success_map(fields: &[BoltValue]) -> &BoltMap {
    match &fields[0] {
        BoltValue::Map(entries) => entries,
        other => panic!("expected Map in SUCCESS, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn handshake_negotiates_v4_4() {
    let (_handle, port) = start_test_server().await;
    let mut stream = connect(port).await;

    let (major, minor) = bolt_handshake(&mut stream).await;
    assert_eq!(major, 4, "expected Bolt major version 4");
    assert_eq!(minor, 4, "expected Bolt minor version 4");
}

#[tokio::test]
async fn hello_succeeds_without_auth() {
    let (_handle, port) = start_test_server().await;
    let mut stream = connect(port).await;

    bolt_handshake(&mut stream).await;
    send_hello(&mut stream).await;

    let (tag, fields) = read_message(&mut stream).await;
    assert_eq!(tag, TAG_SUCCESS, "expected SUCCESS after HELLO");

    let meta = extract_success_map(&fields);
    assert!(
        meta.get_str("server").is_some(),
        "SUCCESS metadata should contain 'server' key"
    );
    assert!(
        meta.get_str("connection_id").is_some(),
        "SUCCESS metadata should contain 'connection_id' key"
    );
}

#[tokio::test]
async fn run_return_1_as_x() {
    let (_handle, port) = start_test_server().await;
    let mut stream = connect(port).await;

    bolt_handshake(&mut stream).await;
    send_hello(&mut stream).await;
    let _ = read_message(&mut stream).await; // consume HELLO SUCCESS

    // RUN "RETURN 1 AS x"
    send_run(&mut stream, "RETURN 1 AS x").await;
    let (tag, fields) = read_message(&mut stream).await;
    assert_eq!(tag, TAG_SUCCESS, "expected SUCCESS after RUN");

    // Verify field names in the RUN SUCCESS metadata
    let meta = extract_success_map(&fields);
    let field_names = meta.get_value("fields");
    assert!(field_names.is_some(), "RUN SUCCESS should contain 'fields'");
    match field_names.unwrap() {
        BoltValue::List(items) => {
            assert_eq!(items.len(), 1);
            assert_eq!(items[0], BoltValue::String("x".to_string()));
        }
        other => panic!("expected List for 'fields', got {:?}", other),
    }

    // PULL all
    send_pull_all(&mut stream).await;

    // Should get a RECORD followed by a SUCCESS
    let (tag, fields) = read_message(&mut stream).await;
    assert_eq!(tag, TAG_RECORD, "expected RECORD after PULL");
    // RECORD contains a list with one field: [data_list]
    match &fields[0] {
        BoltValue::List(items) => {
            assert_eq!(items.len(), 1, "expected 1 field in record");
            assert_eq!(items[0], BoltValue::Int(1), "expected x=1");
        }
        other => panic!("expected List in RECORD, got {:?}", other),
    }

    // Final SUCCESS after PULL
    let (tag, _) = read_message(&mut stream).await;
    assert_eq!(tag, TAG_SUCCESS, "expected SUCCESS after all records");
}

#[tokio::test]
async fn run_create_node_returns_node() {
    let (_handle, port) = start_test_server().await;
    let mut stream = connect(port).await;

    bolt_handshake(&mut stream).await;
    send_hello(&mut stream).await;
    let _ = read_message(&mut stream).await; // consume HELLO SUCCESS

    send_run(&mut stream, "CREATE (n:Test {name: 'hello'}) RETURN n").await;
    let (tag, _) = read_message(&mut stream).await;
    assert_eq!(tag, TAG_SUCCESS, "expected SUCCESS after RUN CREATE");

    send_pull_all(&mut stream).await;

    // First message should be a RECORD containing the node
    let (tag, fields) = read_message(&mut stream).await;
    assert_eq!(tag, TAG_RECORD, "expected RECORD with created node");
    match &fields[0] {
        BoltValue::List(items) => {
            assert_eq!(items.len(), 1, "expected 1 field in record");
            // The node should be a Structure with tag 0x4E (NODE)
            match &items[0] {
                BoltValue::Structure { tag, fields } => {
                    assert_eq!(*tag, 0x4E, "expected NODE structure tag");
                    // Node structure: (id, labels, properties)
                    assert!(fields.len() >= 3, "node should have at least 3 fields");
                    // Check labels contain "Test"
                    match &fields[1] {
                        BoltValue::List(labels) => {
                            assert!(
                                labels.contains(&BoltValue::String("Test".to_string())),
                                "node labels should contain 'Test'"
                            );
                        }
                        other => panic!("expected List for labels, got {:?}", other),
                    }
                    // Check properties contain name='hello'
                    match &fields[2] {
                        BoltValue::Map(props) => {
                            assert_eq!(
                                props.get_value("name"),
                                Some(&BoltValue::String("hello".to_string())),
                                "node should have name='hello'"
                            );
                        }
                        other => panic!("expected Map for properties, got {:?}", other),
                    }
                }
                other => panic!("expected Node Structure, got {:?}", other),
            }
        }
        other => panic!("expected List in RECORD, got {:?}", other),
    }

    // Final SUCCESS after PULL
    let (tag, _) = read_message(&mut stream).await;
    assert_eq!(tag, TAG_SUCCESS, "expected SUCCESS after all records");
}

#[tokio::test]
async fn invalid_query_returns_failure() {
    let (_handle, port) = start_test_server().await;
    let mut stream = connect(port).await;

    bolt_handshake(&mut stream).await;
    send_hello(&mut stream).await;
    let _ = read_message(&mut stream).await; // consume HELLO SUCCESS

    // Send a syntactically invalid query
    send_run(&mut stream, "THIS IS NOT VALID GQL").await;

    let (tag, fields) = read_message(&mut stream).await;
    assert_eq!(tag, TAG_FAILURE, "expected FAILURE for invalid query");

    // FAILURE should have a metadata map with 'code' and 'message'
    match &fields[0] {
        BoltValue::Map(entries) => {
            assert!(
                entries.get_str("code").is_some(),
                "FAILURE metadata should contain 'code'"
            );
            assert!(
                entries.get_str("message").is_some(),
                "FAILURE metadata should contain 'message'"
            );
        }
        other => panic!("expected Map in FAILURE, got {:?}", other),
    }
}

#[tokio::test]
async fn goodbye_closes_cleanly() {
    let (_handle, port) = start_test_server().await;
    let mut stream = connect(port).await;

    bolt_handshake(&mut stream).await;
    send_hello(&mut stream).await;
    let _ = read_message(&mut stream).await; // consume HELLO SUCCESS

    send_goodbye(&mut stream).await;

    // After GOODBYE the server should close the connection.
    // A subsequent read should return 0 bytes (EOF).
    let mut buf = [0u8; 1];
    let n = stream.read(&mut buf).await.unwrap();
    assert_eq!(n, 0, "expected EOF after GOODBYE");
}
