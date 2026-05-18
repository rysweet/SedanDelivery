//! TDD contract tests for the tokio-tungstenite 0.28 migration (issue #1794).
//!
//! Encodes the post-migration API contract for `sedandelivery-gremlin-server`:
//!   * `Message::Text` carries `Utf8Bytes`.
//!   * `Message::Binary` / `Ping` / `Pong` carry `bytes::Bytes`.
//!   * The receive loop handles non-exhaustive `Message` variants gracefully
//!     (server stays alive on unknown variants — verified via Ping round-trip
//!     followed by a successful eval).
//!
//! Pre-migration (0.24): does not compile.
//! Post-migration (0.28): compiles and passes.

use std::sync::Arc;
use std::time::Duration;

use cypher_core::storage::InMemoryGraphStore;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::protocol::frame::Utf8Bytes;
use tokio_tungstenite::tungstenite::Message;

#[test]
fn text_frame_is_utf8_bytes() {
    let s = String::from("payload");
    let msg = Message::Text(s.into());
    if let Message::Text(t) = msg {
        // Confirm we got Utf8Bytes (has .as_str()), not String.
        let _: &str = t.as_str();
        // Static constructor available.
        let _: Utf8Bytes = Utf8Bytes::from_static("static");
    } else {
        panic!("expected Text");
    }
}

#[test]
fn binary_ping_pong_are_bytes() {
    // Vec<u8> -> bytes::Bytes via .into()
    let b = Message::Binary(vec![0xCA, 0xFE].into());
    let p = Message::Ping(vec![1u8, 2].into());
    let q = Message::Pong(vec![3u8, 4].into());
    assert!(matches!(b, Message::Binary(_)));
    assert!(matches!(p, Message::Ping(_)));
    assert!(matches!(q, Message::Pong(_)));
}

#[tokio::test]
async fn ping_then_eval_round_trip() {
    let port = start_server().await;
    let url = format!("ws://127.0.0.1:{port}/gremlin");
    let (ws, _) = tokio_tungstenite::connect_async(&url).await.expect("connect");
    let (mut sink, mut source) = ws.split();

    // Ping with bytes::Bytes payload (0.28 API).
    sink.send(Message::Ping(vec![9u8, 9, 9].into()))
        .await
        .expect("send ping");

    // Expect a Pong back; non-exhaustive match required.
    let pong = tokio::time::timeout(Duration::from_secs(5), source.next())
        .await
        .expect("pong timeout")
        .expect("stream closed")
        .expect("ws err");
    assert!(matches!(pong, Message::Pong(_)));

    // Then send a real eval to prove the server's match-arm catch-all
    // didn't break the protocol loop.
    let req = serde_json::json!({
        "requestId": "00000000-0000-0000-0000-000000000002",
        "op": "eval",
        "args": { "gremlin": "g.V().count()" }
    });
    sink.send(Message::Text(req.to_string().into()))
        .await
        .expect("send eval");

    let resp = tokio::time::timeout(Duration::from_secs(10), source.next())
        .await
        .expect("resp timeout")
        .expect("stream closed")
        .expect("ws err");

    match resp {
        Message::Text(text) => {
            // .as_str() is the post-migration accessor (0.28 Utf8Bytes).
            let v: serde_json::Value =
                serde_json::from_str(text.as_str()).expect("non-JSON text");
            assert!(v.get("requestId").is_some(), "missing requestId");
        }
        other => panic!("expected Text response, got {other:?}"),
    }
}

async fn start_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().unwrap().port();
    let store: Arc<dyn cypher_core::storage::GraphStore> = Arc::new(InMemoryGraphStore::new());
    store.create_graph("default").expect("create graph");
    tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(p) => p,
                Err(_) => break,
            };
            let store = Arc::clone(&store);
            tokio::spawn(async move {
                let _ = sedandelivery_gremlin_server::handle_connection(
                    stream,
                    store,
                    Duration::from_secs(30),
                    "default",
                )
                .await;
            });
        }
    });
    port
}
