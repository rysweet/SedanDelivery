//! Gremlin WebSocket server library for SedanDelivery.
//!
//! Accepts Gremlin text queries (`eval` op) over WebSocket and routes them
//! through gremlin-core's text parser → compiler → cypher-core executor.
//! Returns results in GraphSON v3 format.

pub mod auth;
pub mod executor;
pub mod protocol;

use std::sync::Arc;
use std::time::Duration;

use cypher_core::storage::GraphStore;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite;
use tungstenite::Message;

/// Callback for WebSocket upgrade that validates the `Authorization` header.
///
/// If `auth_token` is `None`, auth is disabled and all upgrades succeed.
/// If `auth_token` is `Some`:
///   - Present + valid header → accept, mark `Authenticated`
///   - Present + invalid header → reject HTTP 401
///   - Absent header → accept, mark `PendingFirstMessage`
struct AuthCallback {
    auth_token: Option<Arc<str>>,
    state: Arc<std::sync::Mutex<auth::AuthState>>,
}

#[allow(clippy::result_large_err)]
impl tungstenite::handshake::server::Callback for AuthCallback {
    fn on_request(
        self,
        request: &tungstenite::http::Request<()>,
        response: tungstenite::http::Response<()>,
    ) -> Result<tungstenite::http::Response<()>, tungstenite::http::Response<Option<String>>> {
        let token = match &self.auth_token {
            None => {
                *self.state.lock().unwrap() = auth::AuthState::Disabled;
                return Ok(response);
            }
            Some(t) => t,
        };

        match request.headers().get("authorization") {
            Some(value) => {
                let value_str = value.to_str().unwrap_or("");
                match auth::parse_bearer_token(value_str) {
                    Some(presented) if auth::validate_token(presented, token) => {
                        tracing::info!("client authenticated via Authorization header");
                        *self.state.lock().unwrap() = auth::AuthState::Authenticated;
                        Ok(response)
                    }
                    _ => {
                        tracing::warn!("WebSocket upgrade rejected: invalid Authorization header");
                        Err(tungstenite::http::Response::builder()
                            .status(tungstenite::http::StatusCode::UNAUTHORIZED)
                            .body(Some("Unauthorized".to_string()))
                            .expect("valid response"))
                    }
                }
            }
            None => {
                *self.state.lock().unwrap() = auth::AuthState::PendingFirstMessage;
                Ok(response)
            }
        }
    }
}

/// Handle a single WebSocket connection.
///
/// Reads Gremlin requests (text or binary frames), executes them, and sends
/// back GraphSON v3 responses. Closes on idle timeout or client disconnect.
///
/// When `auth_token` is `Some`, the server requires bearer authentication
/// either via the `Authorization` header during upgrade or via the `bearer`
/// field in the first Gremlin request message.
pub async fn handle_connection(
    stream: TcpStream,
    store: Arc<dyn GraphStore>,
    idle_timeout: Duration,
    graph: &str,
    auth_token: Option<Arc<str>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = tungstenite::protocol::WebSocketConfig::default().max_frame_size(Some(64 * 1024));

    let auth_state_shared = Arc::new(std::sync::Mutex::new(auth::AuthState::Disabled));
    let callback = AuthCallback {
        auth_token: auth_token.clone(),
        state: Arc::clone(&auth_state_shared),
    };

    let ws_stream =
        tokio_tungstenite::accept_hdr_async_with_config(stream, callback, Some(config)).await?;
    let (mut sink, mut source) = ws_stream.split();

    let mut auth_state = auth_state_shared.lock().unwrap().clone();

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
            Message::Text(text) => text.as_str().to_string(),
            Message::Binary(data) => match protocol::parse_binary_frame(data) {
                Ok(text) => text.to_string(),
                Err(e) => {
                    let resp = protocol::ResponseMessage::error(uuid::Uuid::new_v4(), 498, e);
                    let json = serde_json::to_string(&resp)?;
                    sink.send(Message::Text(json.into())).await?;
                    continue;
                }
            },
            Message::Close(_) => break,
            Message::Ping(data) => {
                sink.send(Message::Pong(data.clone())).await?;
                continue;
            }
            _ => {
                tracing::trace!("ignoring unhandled websocket message variant");
                continue;
            }
        };

        let req = match protocol::parse_request(&json_text) {
            Ok(req) => req,
            Err(e) => {
                let resp = protocol::ResponseMessage::error(uuid::Uuid::new_v4(), 498, e);
                let json = serde_json::to_string(&resp)?;
                sink.send(Message::Text(json.into())).await?;
                continue;
            }
        };

        // First-message bearer auth check
        if auth_state == auth::AuthState::PendingFirstMessage {
            let expected = auth_token
                .as_ref()
                .expect("auth_token must be set when PendingFirstMessage");
            match &req.bearer {
                Some(presented) if auth::validate_token(presented, expected) => {
                    tracing::info!("client authenticated via first-message bearer");
                    auth_state = auth::AuthState::Authenticated;
                }
                _ => {
                    tracing::warn!("first-message bearer auth failed");
                    let resp = protocol::ResponseMessage::error(
                        req.request_id,
                        401,
                        "authentication required".to_string(),
                    );
                    let json = serde_json::to_string(&resp)?;
                    sink.send(Message::Text(json.into())).await?;
                    let _ = sink.send(Message::Close(None)).await;
                    break;
                }
            }
        }

        let response = handle_request(&req, &store, graph).await;
        let json = serde_json::to_string(&response)?;
        sink.send(Message::Text(json.into())).await?;
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
