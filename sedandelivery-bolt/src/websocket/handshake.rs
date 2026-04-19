//! WebSocket upgrade detection and handshake.

use std::io;

use tokio::net::TcpStream;
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::WebSocketStream;

use super::origin::{origin_policy_from_env, validate_origin, OriginPolicy};
use super::WsError;

/// Peek at the first four bytes of a TCP stream to determine if the client is
/// sending a raw Bolt handshake (first byte 0x60) or an HTTP WebSocket
/// upgrade request (starts with "GET ").
pub async fn is_websocket_upgrade(stream: &TcpStream) -> io::Result<bool> {
    let mut buf = [0u8; 4];
    loop {
        let n = stream.peek(&mut buf).await?;
        if n == 0 {
            return Ok(false);
        }
        if n >= 4 {
            return Ok(&buf[..4] == b"GET ");
        }
        if !b"GET ".starts_with(&buf[..n]) {
            return Ok(false);
        }
        stream.readable().await?;
    }
}

/// Perform the WebSocket upgrade handshake and return the WebSocket stream.
pub async fn accept(stream: TcpStream) -> Result<WebSocketStream<TcpStream>, WsError> {
    accept_with_policy(stream, origin_policy_from_env().clone()).await
}

#[allow(clippy::result_large_err)]
pub(super) async fn accept_with_policy(
    stream: TcpStream,
    policy: OriginPolicy,
) -> Result<WebSocketStream<TcpStream>, WsError> {
    let ws_stream = accept_hdr_async(stream, move |request: &Request, response: Response| {
        validate_origin(request, &policy)?;
        Ok(response)
    })
    .await
    .map_err(WsError::Handshake)?;
    Ok(ws_stream)
}
