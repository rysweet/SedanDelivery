//! TDD contract tests for the tokio-tungstenite 0.28 migration (issue #1794).
//!
//! These tests encode the expected post-migration API surface:
//!   * `Message::Text` carries a `Utf8Bytes`, not a `String`.
//!   * `Message::Binary` / `Ping` / `Pong` carry `bytes::Bytes`, not `Vec<u8>`.
//!   * `Message` is `#[non_exhaustive]` — any `match` over inbound frames
//!     in this crate must compile without an exhaustive list (verified by
//!     `cargo build`, but we also assert key construction sites here).
//!
//! Pre-migration (0.24) these tests fail to compile.
//! Post-migration (0.28) they compile and pass.

use std::sync::Arc;
use std::time::Duration;

use cypher_core::storage::{GraphStore, InMemoryGraphStore};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::protocol::frame::Utf8Bytes;
use tokio_tungstenite::tungstenite::Message;

/// Compile-time contract: `Message::Text` accepts a `Utf8Bytes`, which is
/// constructible from `String` and `&str` via `Into`.
#[test]
fn message_text_constructs_from_utf8_bytes() {
    // From &str
    let m1 = Message::Text(Utf8Bytes::from_static("hello"));
    // From String via .into()
    let owned: String = String::from("world");
    let m2 = Message::Text(owned.into());
    // From &str via .into()
    let m3 = Message::Text("plain".into());

    for m in [m1, m2, m3] {
        match m {
            Message::Text(payload) => {
                // Utf8Bytes derefs to &str
                let _: &str = payload.as_str();
                assert!(!payload.as_str().is_empty());
            }
            _ => panic!("expected Text variant"),
        }
    }
}

/// Compile-time contract: `Message::Binary` / `Ping` / `Pong` accept
/// `bytes::Bytes`, constructible from `Vec<u8>` via `.into()`.
#[test]
fn message_binary_ping_pong_use_bytes() {
    let bin = Message::Binary(vec![1u8, 2, 3].into());
    let ping = Message::Ping(vec![4u8].into());
    let pong = Message::Pong(vec![5u8].into());

    match bin {
        Message::Binary(b) => assert_eq!(&b[..], &[1, 2, 3]),
        _ => panic!("expected Binary"),
    }
    match ping {
        Message::Ping(b) => assert_eq!(&b[..], &[4]),
        _ => panic!("expected Ping"),
    }
    match pong {
        Message::Pong(b) => assert_eq!(&b[..], &[5]),
        _ => panic!("expected Pong"),
    }
}

/// Behavioral contract: a real round-trip over the migrated server using
/// the 0.28 API. This exercises both inbound (server reading `Utf8Bytes`)
/// and outbound (server writing `Utf8Bytes`) text frame handling.
#[tokio::test]
async fn round_trip_text_frame_uses_utf8_bytes() {
    let (port, _handle) = start_server().await;
    let url = format!("ws://127.0.0.1:{port}");
    let (mut ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect");

    // Send a request as Text built from a String via .into() — this is the
    // 0.28 idiom (would not compile under 0.24's `Text(String)`).
    let req = serde_json::json!({
        "requestId": "00000000-0000-0000-0000-000000000001",
        "op": "bytecode",
        "processor": "traversal",
        "args": {
            "gremlin": {
                "@type": "g:Bytecode",
                "@value": { "step": [["V"], ["count"]] }
            },
            "aliases": { "g": "g" }
        }
    });
    let payload: String = req.to_string();
    ws.send(Message::Text(payload.into())).await.expect("send");

    // Receive response: pattern-match Text and consume via .as_str().
    let msg = tokio::time::timeout(Duration::from_secs(10), ws.next())
        .await
        .expect("timeout")
        .expect("stream closed")
        .expect("ws error");

    match msg {
        Message::Text(text) => {
            // Utf8Bytes -> &str without allocation
            let s: &str = text.as_str();
            let _: serde_json::Value =
                serde_json::from_str(s).expect("server sent non-JSON text frame");
        }
        // 0.28: Message is #[non_exhaustive] — exhaustive match no longer possible.
        // Any non-Text frame here is a test failure.
        other => panic!("expected Text response, got {other:?}"),
    }
}

async fn start_server() -> (u16, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().unwrap().port();
    let store: Arc<dyn GraphStore> = Arc::new(InMemoryGraphStore::new());
    store.create_graph("default").expect("create graph");
    let handle = tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(p) => p,
                Err(_) => break,
            };
            let store = Arc::clone(&store);
            tokio::spawn(async move {
                let _ = sedandelivery_gremlin::connection::handle_connection(
                    stream,
                    store,
                    Duration::from_secs(30),
                    Duration::from_secs(300),
                )
                .await;
            });
        }
    });
    (port, handle)
}
