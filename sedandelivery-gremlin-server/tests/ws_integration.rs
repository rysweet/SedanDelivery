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
///
/// When `auth_token` is `Some`, the server requires bearer authentication.
/// When `None`, auth is disabled (backward compatible).
/// Returns the port it bound to.
async fn start_test_server_with_auth(auth_token: Option<Arc<str>>) -> u16 {
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
            let auth_token = auth_token.clone();
            tokio::spawn(async move {
                let _ = sedandelivery_gremlin_server::handle_connection(
                    stream,
                    store,
                    Duration::from_secs(30),
                    "default",
                    auth_token,
                )
                .await;
            });
        }
    });

    port
}

/// Start a Gremlin WebSocket server with auth disabled (backward compatible).
async fn start_test_server() -> u16 {
    start_test_server_with_auth(None).await
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
    sink.send(Message::Text(request.to_string().into()))
        .await
        .expect("send failed");

    let msg = tokio::time::timeout(Duration::from_secs(10), source.next())
        .await
        .expect("response timed out")
        .expect("stream ended")
        .expect("ws error");

    match msg {
        Message::Text(text) => serde_json::from_str(text.as_str()).expect("invalid JSON response"),
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
    sink.send(Message::Text(request.to_string().into()))
        .await
        .unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(10), source.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let resp: serde_json::Value = match msg {
        Message::Text(t) => serde_json::from_str(t.as_str()).unwrap(),
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
    sink.send(Message::Text(request.to_string().into()))
        .await
        .unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(10), source.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let resp: serde_json::Value = match msg {
        Message::Text(t) => serde_json::from_str(t.as_str()).unwrap(),
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
    sink.send(Message::Text(request.to_string().into()))
        .await
        .unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(10), source.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let resp: serde_json::Value = match msg {
        Message::Text(t) => serde_json::from_str(t.as_str()).unwrap(),
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
        Message::Text(t) => serde_json::from_str(t.as_str()).unwrap(),
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

    sink.send(Message::Binary(frame.into())).await.unwrap();

    let msg = tokio::time::timeout(Duration::from_secs(10), source.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let resp: serde_json::Value = match msg {
        Message::Text(t) => serde_json::from_str(t.as_str()).unwrap(),
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

    sink.send(Message::Ping(vec![1, 2, 3].into()))
        .await
        .unwrap();

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
        sink.send(Message::Text(request.to_string().into()))
            .await
            .unwrap();

        let msg = tokio::time::timeout(Duration::from_secs(10), source.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let resp: serde_json::Value = match msg {
            Message::Text(t) => serde_json::from_str(t.as_str()).unwrap(),
            other => panic!("expected text, got {other:?}"),
        };
        assert_eq!(resp["status"]["code"], 200);
    }
}

// ── Auth integration tests ──────────────────────────────────────────────

/// Connect to the test server with a custom Authorization header.
///
/// Uses `http::Request::builder()` via `IntoClientRequest` (re-exported by tungstenite)
/// to set custom headers on the WebSocket upgrade request.
async fn connect_with_auth(
    port: u16,
    auth_header: Option<&str>,
) -> Result<
    (
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
    ),
    tokio_tungstenite::tungstenite::Error,
> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    let url = format!("ws://127.0.0.1:{port}/gremlin");
    let mut request = url.into_client_request().expect("valid request");
    if let Some(value) = auth_header {
        request
            .headers_mut()
            .insert("Authorization", value.parse().expect("valid header value"));
    }

    let (ws, _) = tokio_tungstenite::connect_async(request).await?;
    Ok(ws.split())
}

const TEST_AUTH_TOKEN: &str = "test-gremlin-secret-42";

// ── Test 1: Valid Authorization header → authenticated, queries work ────

#[tokio::test]
async fn auth_valid_header_allows_queries() {
    let token: Arc<str> = TEST_AUTH_TOKEN.into();
    let port = start_test_server_with_auth(Some(token)).await;

    let (mut sink, mut source) =
        connect_with_auth(port, Some(&format!("Bearer {TEST_AUTH_TOKEN}")))
            .await
            .expect("connection with valid auth should succeed");

    let resp = send_eval(&mut sink, &mut source, "g.V().count()").await;
    assert_eq!(
        resp["status"]["code"], 200,
        "query should succeed after valid header auth"
    );
}

// ── Test 2: Invalid Authorization header → HTTP 401 rejection ──────────

#[tokio::test]
async fn auth_invalid_header_rejected_with_401() {
    let token: Arc<str> = TEST_AUTH_TOKEN.into();
    let port = start_test_server_with_auth(Some(token)).await;

    // Connect with wrong token in Authorization header
    let result = connect_with_auth(port, Some("Bearer wrong-token-here")).await;

    // The server should reject the upgrade with HTTP 401.
    // tokio-tungstenite surfaces this as a connection error.
    assert!(
        result.is_err(),
        "connection with invalid auth header should be rejected"
    );

    let err = result.unwrap_err();
    let err_str = err.to_string();
    assert!(
        err_str.contains("401"),
        "error should mention 401 status, got: {err_str}"
    );
}

// ── Test 3: No header → first-message bearer auth succeeds ─────────────

#[tokio::test]
async fn auth_first_message_bearer_allows_queries() {
    let token: Arc<str> = TEST_AUTH_TOKEN.into();
    let port = start_test_server_with_auth(Some(token)).await;

    // Connect WITHOUT Authorization header (defers to first-message auth)
    let (mut sink, mut source) = connect_with_auth(port, None)
        .await
        .expect("upgrade without header should succeed for first-message auth");

    // Send first message with bearer field
    let request = serde_json::json!({
        "requestId": "00000000-0000-0000-0000-000000000001",
        "op": "eval",
        "args": {
            "gremlin": "g.V().count()"
        },
        "bearer": TEST_AUTH_TOKEN
    });
    sink.send(Message::Text(request.to_string().into()))
        .await
        .expect("send failed");

    let msg = tokio::time::timeout(Duration::from_secs(10), source.next())
        .await
        .expect("response timed out")
        .expect("stream ended")
        .expect("ws error");

    let resp: serde_json::Value = match msg {
        Message::Text(t) => serde_json::from_str(t.as_str()).expect("invalid JSON"),
        other => panic!("expected text response, got {other:?}"),
    };

    assert_eq!(
        resp["status"]["code"], 200,
        "query should succeed after first-message bearer auth"
    );
}

// ── Test 4: Auth required but no token provided → 401 error + close ────

#[tokio::test]
async fn auth_missing_when_required_returns_401() {
    let token: Arc<str> = TEST_AUTH_TOKEN.into();
    let port = start_test_server_with_auth(Some(token)).await;

    // Connect WITHOUT Authorization header
    let (mut sink, mut source) = connect_with_auth(port, None)
        .await
        .expect("upgrade without header should succeed");

    // Send first message WITHOUT bearer field
    let request = serde_json::json!({
        "requestId": "00000000-0000-0000-0000-000000000001",
        "op": "eval",
        "args": {
            "gremlin": "g.V().count()"
        }
    });
    sink.send(Message::Text(request.to_string().into()))
        .await
        .expect("send failed");

    let msg = tokio::time::timeout(Duration::from_secs(10), source.next())
        .await
        .expect("response timed out")
        .expect("stream ended")
        .expect("ws error");

    let resp: serde_json::Value = match msg {
        Message::Text(t) => serde_json::from_str(t.as_str()).expect("invalid JSON"),
        other => panic!("expected text response, got {other:?}"),
    };

    assert_eq!(
        resp["status"]["code"], 401,
        "should return 401 when auth required but no bearer provided"
    );

    // After auth failure, server should close the connection
    let next = tokio::time::timeout(Duration::from_secs(5), source.next()).await;
    match next {
        Ok(Some(Ok(Message::Close(_)))) | Ok(None) | Err(_) => {
            // Expected: close frame, stream end, or timeout (server closed)
        }
        Ok(Some(Ok(other))) => {
            panic!("expected connection close after auth failure, got: {other:?}")
        }
        Ok(Some(Err(_))) => {
            // Connection error also acceptable (server dropped)
        }
    }
}

// ── Test 5: Auth disabled (no token configured) → queries work without auth ─

#[tokio::test]
async fn auth_disabled_allows_queries_without_token() {
    // Start server with auth disabled (None token) — backward compatible
    let port = start_test_server_with_auth(None).await;

    let (mut sink, mut source) = connect_with_auth(port, None)
        .await
        .expect("connection should succeed when auth disabled");

    let resp = send_eval(&mut sink, &mut source, "g.V().count()").await;
    assert_eq!(
        resp["status"]["code"], 200,
        "queries should work without auth when token not configured"
    );
}

// ── Test 6: Invalid first-message bearer → 401 + close ─────────────────

#[tokio::test]
async fn auth_invalid_first_message_bearer_returns_401() {
    let token: Arc<str> = TEST_AUTH_TOKEN.into();
    let port = start_test_server_with_auth(Some(token)).await;

    // Connect without Authorization header
    let (mut sink, mut source) = connect_with_auth(port, None)
        .await
        .expect("upgrade without header should succeed");

    // Send first message with WRONG bearer token
    let request = serde_json::json!({
        "requestId": "00000000-0000-0000-0000-000000000001",
        "op": "eval",
        "args": {
            "gremlin": "g.V().count()"
        },
        "bearer": "wrong-token-entirely"
    });
    sink.send(Message::Text(request.to_string().into()))
        .await
        .expect("send failed");

    let msg = tokio::time::timeout(Duration::from_secs(10), source.next())
        .await
        .expect("response timed out")
        .expect("stream ended")
        .expect("ws error");

    let resp: serde_json::Value = match msg {
        Message::Text(t) => serde_json::from_str(t.as_str()).expect("invalid JSON"),
        other => panic!("expected text response, got {other:?}"),
    };

    assert_eq!(
        resp["status"]["code"], 401,
        "should return 401 for invalid bearer in first message"
    );
}
