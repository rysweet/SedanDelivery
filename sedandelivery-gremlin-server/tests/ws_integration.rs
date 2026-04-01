//! WebSocket integration tests for the Gremlin server.
//!
//! Starts a real WebSocket server backed by `InMemoryGraphStore`, then
//! connects with a raw WebSocket client and exercises the Gremlin protocol.

use std::sync::Arc;
use std::time::Duration;

use cypher_core::storage::InMemoryGraphStore;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

/// Start a Gremlin WebSocket server on a random port with an in-memory store.
/// Returns the port it bound to.
async fn start_test_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind");
    let port = listener.local_addr().unwrap().port();

    let store: Arc<dyn cypher_core::storage::GraphStore> = Arc::new(InMemoryGraphStore::new());
    store
        .create_graph("default")
        .expect("failed to create default graph");

    tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(pair) => pair,
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

/// Connect a WebSocket client to the test server.
async fn connect(
    port: u16,
) -> (
    futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
) {
    let url = format!("ws://127.0.0.1:{port}/gremlin");
    let (ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("failed to connect");
    ws.split()
}

/// Send a text request and read the JSON response.
async fn send_eval(
    sink: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    source: &mut futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
    gremlin: &str,
) -> serde_json::Value {
    let request = serde_json::json!({
        "requestId": "00000000-0000-0000-0000-000000000001",
        "op": "eval",
        "args": {
            "gremlin": gremlin
        }
    });
    sink.send(Message::Text(request.to_string()))
        .await
        .expect("send failed");

    let msg = tokio::time::timeout(Duration::from_secs(10), source.next())
        .await
        .expect("response timed out")
        .expect("stream ended")
        .expect("ws error");

    match msg {
        Message::Text(text) => serde_json::from_str(&text).expect("invalid JSON response"),
        other => panic!("expected text response, got {other:?}"),
    }
}

#[tokio::test]
async fn eval_vertex_count_on_empty_graph() {
    let port = start_test_server().await;
    let (mut sink, mut source) = connect(port).await;

    let resp = send_eval(&mut sink, &mut source, "g.V().count()").await;
    assert_eq!(resp["status"]["code"], 200);
}

#[tokio::test]
async fn eval_add_vertex_and_query() {
    let port = start_test_server().await;
    let (mut sink, mut source) = connect(port).await;

    // Add a vertex
    let resp = send_eval(
        &mut sink,
        &mut source,
        r#"g.addV("person").property("name","alice")"#,
    )
    .await;
    assert_eq!(resp["status"]["code"], 200, "addV failed: {resp}");

    // Query the vertex
    let resp = send_eval(
        &mut sink,
        &mut source,
        r#"g.V().has("person","name","alice").count()"#,
    )
    .await;
    assert_eq!(resp["status"]["code"], 200);
}

#[tokio::test]
async fn eval_invalid_query_returns_error() {
    let port = start_test_server().await;
    let (mut sink, mut source) = connect(port).await;

    let resp = send_eval(&mut sink, &mut source, "g.NOSUCHSTEP()").await;
    // Should be a 597 evaluation error
    assert_eq!(resp["status"]["code"], 597);
    assert!(!resp["status"]["message"].as_str().unwrap_or("").is_empty());
}

#[tokio::test]
async fn eval_missing_gremlin_arg_returns_499() {
    let port = start_test_server().await;
    let (mut sink, mut source) = connect(port).await;

    let request = serde_json::json!({
        "requestId": "00000000-0000-0000-0000-000000000001",
        "op": "eval",
        "args": {}
    });
    sink.send(Message::Text(request.to_string())).await.unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(10), source.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let resp: serde_json::Value = match msg {
        Message::Text(t) => serde_json::from_str(&t).unwrap(),
        other => panic!("expected text, got {other:?}"),
    };
    assert_eq!(resp["status"]["code"], 499);
}

#[tokio::test]
async fn close_op_returns_204() {
    let port = start_test_server().await;
    let (mut sink, mut source) = connect(port).await;

    let request = serde_json::json!({
        "requestId": "00000000-0000-0000-0000-000000000002",
        "op": "close",
        "args": {}
    });
    sink.send(Message::Text(request.to_string())).await.unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(10), source.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let resp: serde_json::Value = match msg {
        Message::Text(t) => serde_json::from_str(&t).unwrap(),
        other => panic!("expected text, got {other:?}"),
    };
    assert_eq!(resp["status"]["code"], 204);
}

#[tokio::test]
async fn unsupported_op_returns_500() {
    let port = start_test_server().await;
    let (mut sink, mut source) = connect(port).await;

    let request = serde_json::json!({
        "requestId": "00000000-0000-0000-0000-000000000003",
        "op": "bytecode",
        "args": {}
    });
    sink.send(Message::Text(request.to_string())).await.unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(10), source.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let resp: serde_json::Value = match msg {
        Message::Text(t) => serde_json::from_str(&t).unwrap(),
        other => panic!("expected text, got {other:?}"),
    };
    assert_eq!(resp["status"]["code"], 500);
    assert!(resp["status"]["message"]
        .as_str()
        .unwrap()
        .contains("unsupported op"));
}

#[tokio::test]
async fn malformed_json_returns_498() {
    let port = start_test_server().await;
    let (mut sink, mut source) = connect(port).await;

    sink.send(Message::Text("{not valid json".into()))
        .await
        .unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(10), source.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let resp: serde_json::Value = match msg {
        Message::Text(t) => serde_json::from_str(&t).unwrap(),
        other => panic!("expected text, got {other:?}"),
    };
    assert_eq!(resp["status"]["code"], 498);
}

#[tokio::test]
async fn binary_frame_with_graphson_mime() {
    let port = start_test_server().await;
    let (mut sink, mut source) = connect(port).await;

    let mime = b"application/vnd.gremlin-v3.0+json";
    let payload = serde_json::json!({
        "requestId": "00000000-0000-0000-0000-000000000004",
        "op": "eval",
        "args": {"gremlin": "g.V().count()"}
    });
    let payload_bytes = payload.to_string().into_bytes();

    let mut frame = vec![mime.len() as u8];
    frame.extend_from_slice(mime);
    frame.extend_from_slice(&payload_bytes);

    sink.send(Message::Binary(frame)).await.unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(10), source.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let resp: serde_json::Value = match msg {
        Message::Text(t) => serde_json::from_str(&t).unwrap(),
        other => panic!("expected text, got {other:?}"),
    };
    assert_eq!(resp["status"]["code"], 200);
}

#[tokio::test]
async fn ping_gets_pong() {
    let port = start_test_server().await;

    let url = format!("ws://127.0.0.1:{port}/gremlin");
    let (ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let (mut sink, mut source) = ws.split();

    sink.send(Message::Ping(vec![1, 2, 3])).await.unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(5), source.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(msg, Message::Pong(_)));
}

#[tokio::test]
async fn multiple_requests_on_same_connection() {
    let port = start_test_server().await;
    let (mut sink, mut source) = connect(port).await;

    // Send multiple queries on the same connection
    for i in 0..3 {
        let request = serde_json::json!({
            "requestId": format!("00000000-0000-0000-0000-00000000000{i}"),
            "op": "eval",
            "args": {"gremlin": "g.V().count()"}
        });
        sink.send(Message::Text(request.to_string())).await.unwrap();

        let msg = tokio::time::timeout(Duration::from_secs(10), source.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let resp: serde_json::Value = match msg {
            Message::Text(t) => serde_json::from_str(&t).unwrap(),
            other => panic!("expected text, got {other:?}"),
        };
        assert_eq!(resp["status"]["code"], 200);
    }
}
