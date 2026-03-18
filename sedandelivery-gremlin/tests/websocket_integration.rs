//! WebSocket end-to-end integration tests for the Gremlin server.
//!
//! Each test starts the server in-process with an `InMemoryGraphStore`,
//! connects via `tokio-tungstenite`, sends a GraphSON bytecode request,
//! and verifies the response.

use std::collections::HashMap;
use std::sync::Arc;

use gql_core::storage::{GraphStore, InMemoryGraphStore};
use gql_core::types::Value;
use serde_json::json;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Test infrastructure
// ---------------------------------------------------------------------------

/// Start the Gremlin WebSocket server on a random port with the TinkerPop
/// "modern" graph pre-loaded in the `"default"` graph.
///
/// Returns the server task handle and the port.
async fn start_test_server() -> (JoinHandle<()>, u16) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind test listener");
    let port = listener.local_addr().unwrap().port();

    let store: Arc<dyn GraphStore> = Arc::new(create_modern_graph_store());

    let handle = tokio::spawn(async move {
        loop {
            let (stream, _peer) = match listener.accept().await {
                Ok(pair) => pair,
                Err(_) => break,
            };
            let store = Arc::clone(&store);
            tokio::spawn(async move {
                let _ = sedandelivery_gremlin::connection::handle_connection(stream, store).await;
            });
        }
    });

    (handle, port)
}

/// Connect a WebSocket client to the test server.
async fn connect(
    port: u16,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let url = format!("ws://127.0.0.1:{port}");
    let (ws, _response) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("failed to connect to test server");
    ws
}

/// Build a GraphSON bytecode request JSON string.
fn bytecode_request(steps: Vec<(&str, Vec<serde_json::Value>)>) -> String {
    let request_id = Uuid::new_v4().to_string();
    let step_instructions: Vec<serde_json::Value> = steps
        .iter()
        .map(|(name, args)| {
            let mut arr = vec![json!(name)];
            arr.extend(args.iter().cloned());
            json!(arr)
        })
        .collect();

    json!({
        "requestId": request_id,
        "op": "bytecode",
        "processor": "traversal",
        "args": {
            "gremlin": {
                "@type": "g:Bytecode",
                "@value": {
                    "step": step_instructions
                }
            },
            "aliases": {"g": "g"}
        }
    })
    .to_string()
}

/// Build a raw Gremlin request with a custom op.
fn raw_request(op: &str, args: serde_json::Value) -> String {
    let request_id = Uuid::new_v4().to_string();
    json!({
        "requestId": request_id,
        "op": op,
        "processor": "",
        "args": args
    })
    .to_string()
}

/// Send a text message and receive the response, returning parsed JSON.
async fn send_and_receive(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    msg: &str,
) -> serde_json::Value {
    use futures_util::{SinkExt, StreamExt};

    ws.send(Message::Text(msg.to_string())).await.unwrap();

    let response = ws.next().await.expect("no response").expect("ws error");
    match response {
        Message::Text(text) => serde_json::from_str(&text).expect("invalid JSON response"),
        other => panic!("expected text message, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Modern graph fixture (in "default" graph for GremlinSession compatibility)
// ---------------------------------------------------------------------------

fn create_modern_graph_store() -> InMemoryGraphStore {
    let store = InMemoryGraphStore::new();
    store.create_graph("default").unwrap();

    // Vertices
    let marko = store
        .create_node("default", vec!["person".into()], person_props("marko", 29))
        .unwrap();
    let vadas = store
        .create_node("default", vec!["person".into()], person_props("vadas", 27))
        .unwrap();
    let _lop = store
        .create_node(
            "default",
            vec!["software".into()],
            software_props("lop", "java"),
        )
        .unwrap();
    let josh = store
        .create_node("default", vec!["person".into()], person_props("josh", 32))
        .unwrap();
    let _ripple = store
        .create_node(
            "default",
            vec!["software".into()],
            software_props("ripple", "java"),
        )
        .unwrap();
    let _peter = store
        .create_node("default", vec!["person".into()], person_props("peter", 35))
        .unwrap();

    // Edges
    store
        .create_edge("default", marko, vadas, "knows".into(), weight_props(0.5))
        .unwrap();
    store
        .create_edge("default", marko, josh, "knows".into(), weight_props(1.0))
        .unwrap();
    store
        .create_edge("default", marko, _lop, "created".into(), weight_props(0.4))
        .unwrap();
    store
        .create_edge(
            "default",
            josh,
            _ripple,
            "created".into(),
            weight_props(1.0),
        )
        .unwrap();
    store
        .create_edge("default", josh, _lop, "created".into(), weight_props(0.4))
        .unwrap();
    store
        .create_edge("default", _peter, _lop, "created".into(), weight_props(0.2))
        .unwrap();

    store
}

fn person_props(name: &str, age: i64) -> HashMap<String, Value> {
    let mut props = HashMap::new();
    props.insert("name".into(), Value::Str(name.into()));
    props.insert("age".into(), Value::Int(age));
    props
}

fn software_props(name: &str, lang: &str) -> HashMap<String, Value> {
    let mut props = HashMap::new();
    props.insert("name".into(), Value::Str(name.into()));
    props.insert("lang".into(), Value::Str(lang.into()));
    props
}

fn weight_props(weight: f64) -> HashMap<String, Value> {
    let mut props = HashMap::new();
    props.insert("weight".into(), Value::Float(weight));
    props
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_vertex_count_over_websocket() {
    let (_handle, port) = start_test_server().await;
    let mut ws = connect(port).await;

    // g.V().count()
    let msg = bytecode_request(vec![("V", vec![]), ("count", vec![])]);
    let resp = send_and_receive(&mut ws, &msg).await;

    assert_eq!(resp["status"]["code"], 200, "response: {resp}");

    let data = resp["result"]["data"]
        .as_array()
        .expect("data should be array");
    assert_eq!(data.len(), 1, "count should return a single value");

    // The count should be 6 (6 vertices in the modern graph).
    let count = data[0]["@value"].as_i64().unwrap_or_else(|| {
        // Fallback: count may be returned as a plain integer.
        data[0].as_i64().expect("count should be an integer")
    });
    assert_eq!(count, 6, "modern graph has 6 vertices");
}

#[tokio::test]
async fn test_has_label_person() {
    let (_handle, port) = start_test_server().await;
    let mut ws = connect(port).await;

    // g.V().hasLabel("person")
    let msg = bytecode_request(vec![("V", vec![]), ("hasLabel", vec![json!("person")])]);
    let resp = send_and_receive(&mut ws, &msg).await;

    assert_eq!(resp["status"]["code"], 200, "response: {resp}");

    let data = resp["result"]["data"]
        .as_array()
        .expect("data should be array");
    // 4 person vertices: marko, vadas, josh, peter
    assert_eq!(data.len(), 4, "modern graph has 4 person vertices");
}

#[tokio::test]
async fn test_has_label_software() {
    let (_handle, port) = start_test_server().await;
    let mut ws = connect(port).await;

    // g.V().hasLabel("software")
    let msg = bytecode_request(vec![("V", vec![]), ("hasLabel", vec![json!("software")])]);
    let resp = send_and_receive(&mut ws, &msg).await;

    assert_eq!(resp["status"]["code"], 200, "response: {resp}");

    let data = resp["result"]["data"]
        .as_array()
        .expect("data should be array");
    // 2 software vertices: lop, ripple
    assert_eq!(data.len(), 2, "modern graph has 2 software vertices");
}

#[tokio::test]
async fn test_traversal_out_knows_values() {
    let (_handle, port) = start_test_server().await;
    let mut ws = connect(port).await;

    // g.V().has("name","marko").out("knows").values("name")
    let msg = bytecode_request(vec![
        ("V", vec![]),
        ("has", vec![json!("name"), json!("marko")]),
        ("out", vec![json!("knows")]),
        ("values", vec![json!("name")]),
    ]);
    let resp = send_and_receive(&mut ws, &msg).await;

    assert_eq!(resp["status"]["code"], 200, "response: {resp}");

    let data = resp["result"]["data"]
        .as_array()
        .expect("data should be array");
    assert_eq!(data.len(), 2, "marko knows 2 people");

    let mut names: Vec<String> = data
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .collect();
    names.sort();
    assert_eq!(names, vec!["josh", "vadas"]);
}

#[tokio::test]
async fn test_eval_op_returns_error() {
    let (_handle, port) = start_test_server().await;
    let mut ws = connect(port).await;

    // Send an "eval" op — should be rejected with 597 SCRIPT_EVALUATION_ERROR.
    let msg = raw_request("eval", json!({"gremlin": "g.V().count()"}));
    let resp = send_and_receive(&mut ws, &msg).await;

    assert_eq!(
        resp["status"]["code"], 597,
        "eval should return 597: {resp}"
    );
    let message = resp["status"]["message"].as_str().unwrap_or("");
    assert!(
        message.contains("not supported"),
        "error message should mention 'not supported': {message}"
    );
}

#[tokio::test]
async fn test_empty_traversal() {
    let (_handle, port) = start_test_server().await;
    let mut ws = connect(port).await;

    // g.V().has("name","nonexistent")
    let msg = bytecode_request(vec![
        ("V", vec![]),
        ("has", vec![json!("name"), json!("nonexistent")]),
    ]);
    let resp = send_and_receive(&mut ws, &msg).await;

    // Expect either 200 with empty data or 204 NO_CONTENT.
    let code = resp["status"]["code"].as_u64().unwrap();
    assert!(
        code == 200 || code == 204,
        "empty result should be 200 or 204: {resp}"
    );

    if code == 200 {
        let data = resp["result"]["data"]
            .as_array()
            .expect("data should be array");
        assert!(data.is_empty(), "no vertices should match: {data:?}");
    }
}

#[tokio::test]
async fn test_unsupported_op_returns_server_error() {
    let (_handle, port) = start_test_server().await;
    let mut ws = connect(port).await;

    // Send a completely unknown op.
    let msg = raw_request("bogus_op", json!({}));
    let resp = send_and_receive(&mut ws, &msg).await;

    assert_eq!(
        resp["status"]["code"], 500,
        "unknown op should return 500: {resp}"
    );
}

#[tokio::test]
async fn test_close_op_returns_no_content() {
    let (_handle, port) = start_test_server().await;
    let mut ws = connect(port).await;

    let msg = raw_request("close", json!({}));
    let resp = send_and_receive(&mut ws, &msg).await;

    assert_eq!(
        resp["status"]["code"], 204,
        "close should return 204: {resp}"
    );
}

#[tokio::test]
async fn test_malformed_json_returns_error() {
    let (_handle, port) = start_test_server().await;
    let mut ws = connect(port).await;

    // Send invalid JSON — the server should respond with an error (protocol
    // error is surfaced as a connection error, which closes the connection).
    use futures_util::{SinkExt, StreamExt};
    ws.send(Message::Text("not valid json {{{".to_string()))
        .await
        .unwrap();

    // The server may return an error response or close the connection.
    let response = ws.next().await;
    match response {
        Some(Ok(Message::Text(text))) => {
            let resp: serde_json::Value = serde_json::from_str(&text).unwrap();
            // Any error code is acceptable (400, 500, etc.)
            let code = resp["status"]["code"].as_u64().unwrap_or(0);
            assert!(code >= 400, "malformed JSON should return error: {resp}");
        }
        Some(Ok(Message::Close(_))) | None => {
            // Connection closed — also acceptable for a malformed request.
        }
        other => {
            // Connection error is also fine — the server dropped us.
            if let Some(Err(_)) = other {
                // Expected: connection reset / closed.
            } else {
                panic!("unexpected response: {other:?}");
            }
        }
    }
}

#[tokio::test]
async fn test_vertex_limit() {
    let (_handle, port) = start_test_server().await;
    let mut ws = connect(port).await;

    // g.V().limit(2)
    let msg = bytecode_request(vec![("V", vec![]), ("limit", vec![json!(2)])]);
    let resp = send_and_receive(&mut ws, &msg).await;

    assert_eq!(resp["status"]["code"], 200, "response: {resp}");

    let data = resp["result"]["data"]
        .as_array()
        .expect("data should be array");
    assert_eq!(data.len(), 2, "limit(2) should return exactly 2 vertices");
}

#[tokio::test]
async fn test_multiple_requests_on_same_connection() {
    let (_handle, port) = start_test_server().await;
    let mut ws = connect(port).await;

    // First request: g.V().count()
    let msg1 = bytecode_request(vec![("V", vec![]), ("count", vec![])]);
    let resp1 = send_and_receive(&mut ws, &msg1).await;
    assert_eq!(resp1["status"]["code"], 200, "first request: {resp1}");

    // Second request: g.V().hasLabel("software")
    let msg2 = bytecode_request(vec![("V", vec![]), ("hasLabel", vec![json!("software")])]);
    let resp2 = send_and_receive(&mut ws, &msg2).await;
    assert_eq!(resp2["status"]["code"], 200, "second request: {resp2}");

    let data = resp2["result"]["data"]
        .as_array()
        .expect("data should be array");
    assert_eq!(data.len(), 2, "second request should still work");
}
