//! Per-connection handler for Gremlin WebSocket sessions.
//!
//! Each accepted TCP connection is upgraded to WebSocket, then enters a
//! read loop dispatching messages to the protocol handler.

use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;

use gql_core::storage::GraphStore;

use crate::protocol;

/// Errors that can occur during a connection lifecycle.
#[derive(Debug, thiserror::Error)]
pub enum ConnectionError {
    #[error("websocket error: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),

    #[error("protocol error: {0}")]
    Protocol(#[from] protocol::ProtocolError),
}

/// State for a single Gremlin session.
pub struct GremlinSession {
    pub(crate) store: Arc<dyn GraphStore>,
    pub(crate) graph: String,
    pub(crate) authenticated: bool,
}

impl GremlinSession {
    fn new(store: Arc<dyn GraphStore>) -> Self {
        Self {
            store,
            graph: "default".to_string(),
            authenticated: false,
        }
    }

    /// Returns whether authentication is required but not yet completed.
    pub(crate) fn requires_auth(&self) -> bool {
        let password_set = std::env::var("GREMLIN_PASSWORD")
            .ok()
            .filter(|p| !p.is_empty())
            .is_some();
        password_set && !self.authenticated
    }
}

/// Handle a single WebSocket connection end-to-end.
pub async fn handle_connection(
    stream: TcpStream,
    store: Arc<dyn GraphStore>,
) -> Result<(), ConnectionError> {
    let ws_stream = tokio_tungstenite::accept_async(stream).await?;
    let (mut sink, mut source) = ws_stream.split();

    let mut session = GremlinSession::new(store);

    // If GREMLIN_PASSWORD is set, the first message must authenticate.
    if session.requires_auth() {
        let challenge = protocol::response::ResponseMessage::authenticate_challenge();
        sink.send(Message::Text(serde_json::to_string(&challenge).unwrap()))
            .await?;
    }

    while let Some(msg) = source.next().await {
        let msg = msg?;

        match msg {
            Message::Text(text) => {
                let response =
                    protocol::handler::handle_message(&text, &mut session)?;
                let json = serde_json::to_string(&response)
                    .expect("ResponseMessage must serialise");
                sink.send(Message::Text(json)).await?;
            }
            Message::Binary(_) => {
                // GraphSON is text-based; binary frames are not supported in Phase 1.
                let response = protocol::response::ResponseMessage::error(
                    uuid::Uuid::new_v4(),
                    protocol::response::StatusCode::ServerError,
                    "binary frames not supported".to_string(),
                );
                let json = serde_json::to_string(&response).unwrap();
                sink.send(Message::Text(json)).await?;
            }
            Message::Close(_) => break,
            Message::Ping(data) => {
                sink.send(Message::Pong(data)).await?;
            }
            _ => {}
        }
    }

    Ok(())
}
