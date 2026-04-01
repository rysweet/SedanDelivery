//! Gremlin WebSocket server library for SedanDelivery.
//!
//! Accepts Gremlin text queries (`eval` op) over WebSocket and routes them
//! through gremlin-core's text parser → compiler → cypher-core executor.
//! Returns results in GraphSON v3 format.

pub mod executor;
pub mod protocol;

use std::sync::Arc;
use std::time::Duration;

use cypher_core::storage::GraphStore;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;

/// Handle a single WebSocket connection.
///
/// Reads Gremlin requests (text or binary frames), executes them, and sends
/// back GraphSON v3 responses. Closes on idle timeout or client disconnect.
pub async fn handle_connection(
    stream: TcpStream,
    store: Arc<dyn GraphStore>,
    idle_timeout: Duration,
    graph: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig {
        max_frame_size: Some(64 * 1024),
        ..Default::default()
    };
    let ws_stream = tokio_tungstenite::accept_async_with_config(stream, Some(config)).await?;
    let (mut sink, mut source) = ws_stream.split();

    loop {
        let msg = match tokio::time::timeout(idle_timeout, source.next()).await {
            Ok(Some(msg)) => msg?,
            Ok(None) => break,
            Err(_) => {
                tracing::debug!("closing connection after idle timeout");
                break;
            }
        };

        let json_text = match &msg {
            Message::Text(text) => text.clone(),
            Message::Binary(data) => match protocol::parse_binary_frame(data) {
                Ok(text) => text.to_string(),
                Err(e) => {
                    let resp =
                        protocol::ResponseMessage::error(uuid::Uuid::new_v4(), 498, e);
                    let json = serde_json::to_string(&resp)?;
                    sink.send(Message::Text(json)).await?;
                    continue;
                }
            },
            Message::Close(_) => break,
            Message::Ping(data) => {
                sink.send(Message::Pong(data.clone())).await?;
                continue;
            }
            _ => continue,
        };

        let req = match protocol::parse_request(&json_text) {
            Ok(req) => req,
            Err(e) => {
                let resp =
                    protocol::ResponseMessage::error(uuid::Uuid::new_v4(), 498, e);
                let json = serde_json::to_string(&resp)?;
                sink.send(Message::Text(json)).await?;
                continue;
            }
        };

        let response = handle_request(&req, &store, graph).await;
        let json = serde_json::to_string(&response)?;
        sink.send(Message::Text(json)).await?;
    }

    Ok(())
}

async fn handle_request(
    req: &protocol::RequestMessage,
    store: &Arc<dyn GraphStore>,
    graph: &str,
) -> protocol::ResponseMessage {
    match req.op.as_str() {
        "eval" => handle_eval(req, store, graph).await,
        "close" => protocol::ResponseMessage::no_content(req.request_id),
        other => protocol::ResponseMessage::error(
            req.request_id,
            500,
            format!("unsupported op: {other}"),
        ),
    }
}

async fn handle_eval(
    req: &protocol::RequestMessage,
    store: &Arc<dyn GraphStore>,
    graph: &str,
) -> protocol::ResponseMessage {
    let gremlin = match req.args.get("gremlin").and_then(|v| v.as_str()) {
        Some(g) => g.to_string(),
        None => {
            return protocol::ResponseMessage::error(
                req.request_id,
                499,
                "missing 'gremlin' in args".to_string(),
            );
        }
    };

    let bindings: std::collections::HashMap<String, serde_json::Value> = req
        .args
        .get("bindings")
        .and_then(|v| v.as_object())
        .map(|obj| obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();

    let target_graph = req
        .args
        .get("aliases")
        .and_then(|v| v.as_object())
        .and_then(|obj| obj.get("g"))
        .and_then(|v| v.as_str())
        .unwrap_or(graph)
        .to_string();

    let store = Arc::clone(store);
    let request_id = req.request_id;

    match tokio::task::spawn_blocking(move || {
        executor::execute_gremlin_text(&*store, &target_graph, &gremlin, &bindings)
    })
    .await
    {
        Ok(Ok(data)) => {
            protocol::ResponseMessage::success(request_id, serde_json::Value::Array(data))
        }
        Ok(Err(e)) => protocol::ResponseMessage::error(request_id, 597, e.to_string()),
        Err(e) => {
            protocol::ResponseMessage::error(request_id, 500, format!("execution panicked: {e}"))
        }
    }
}
