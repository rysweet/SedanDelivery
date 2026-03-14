//! WebSocket transport for the Bolt protocol.
//!
//! Browser-based Neo4j drivers (JavaScript) connect via WebSocket rather than
//! raw TCP.  The Bolt messages are identical — they're just wrapped in
//! WebSocket binary frames instead of being sent directly over TCP.
//!
//! This module provides:
//! - Detection of WebSocket upgrade requests vs raw Bolt connections
//! - WebSocket upgrade using `tokio-tungstenite`
//! - An `AsyncRead + AsyncWrite` adapter that bridges WebSocket frames to
//!   the byte-stream interface expected by `session::run`

use std::io;
use std::pin::Pin;
use std::sync::OnceLock;
use std::task::{Context, Poll};

use futures_util::stream::Stream;
use futures_util::Sink;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

/// Default maximum WebSocket write buffer size (16 MiB).
const DEFAULT_WS_MAX_WRITE_BUF: usize = 16_777_216;

fn ws_max_write_buf() -> usize {
    static SIZE: OnceLock<usize> = OnceLock::new();
    *SIZE.get_or_init(|| {
        std::env::var("BOLT_WS_MAX_WRITE_BUF")
            .ok()
            .and_then(|v| match v.parse::<usize>() {
                Ok(val) if val > 0 => Some(val),
                _ => {
                    tracing::warn!(
                        "invalid BOLT_WS_MAX_WRITE_BUF value, using default {DEFAULT_WS_MAX_WRITE_BUF}"
                    );
                    None
                }
            })
            .unwrap_or(DEFAULT_WS_MAX_WRITE_BUF)
    })
}

#[derive(Clone, Debug)]
enum OriginPolicy {
    RejectPresentedOrigins,
    AllowAll,
    Allowlist(Vec<Box<str>>),
}

impl OriginPolicy {
    fn from_env() -> Self {
        match std::env::var("BOLT_WS_ALLOWED_ORIGINS") {
            Ok(value) => {
                let trimmed = value.trim();
                if trimmed == "*" {
                    Self::AllowAll
                } else {
                    let allowlist = trimmed
                        .split(',')
                        .map(str::trim)
                        .filter(|origin| !origin.is_empty())
                        .map(Box::<str>::from)
                        .collect::<Vec<_>>();
                    if allowlist.is_empty() {
                        tracing::warn!(
                            "BOLT_WS_ALLOWED_ORIGINS is empty; rejecting WebSocket upgrades with Origin headers"
                        );
                        Self::RejectPresentedOrigins
                    } else {
                        Self::Allowlist(allowlist)
                    }
                }
            }
            Err(_) => Self::RejectPresentedOrigins,
        }
    }

    fn allows(&self, origin: &str) -> bool {
        match self {
            Self::RejectPresentedOrigins => false,
            Self::AllowAll => true,
            Self::Allowlist(allowlist) => {
                allowlist.iter().any(|allowed| allowed.as_ref() == origin)
            }
        }
    }
}

fn origin_policy_from_env() -> &'static OriginPolicy {
    static POLICY: OnceLock<OriginPolicy> = OnceLock::new();
    POLICY.get_or_init(OriginPolicy::from_env)
}

fn forbidden_origin(message: &str) -> ErrorResponse {
    Response::builder()
        .status(StatusCode::FORBIDDEN)
        .body(Some(message.to_string()))
        .expect("forbidden websocket response should be buildable")
}

// `ErrorResponse` is `Response<Option<String>>` from tungstenite, which is
// larger than Clippy's default threshold for `result_large_err`.  The large
// type is unavoidable here: it is the exact error type required by the
// `accept_hdr_async` callback signature in `tokio-tungstenite`.  We cannot
// use a smaller wrapper because the callback must return `Result<_, ErrorResponse>`
// — there is no conversion path that avoids materialising an `ErrorResponse`
// at the call site.  Heap-boxing the error would add an allocation on every
// rejected WebSocket handshake for no correctness benefit.
#[allow(clippy::result_large_err)]
fn validate_origin(request: &Request, policy: &OriginPolicy) -> Result<(), ErrorResponse> {
    let Some(origin) = request.headers().get("Origin") else {
        return match policy {
            OriginPolicy::RejectPresentedOrigins | OriginPolicy::AllowAll => Ok(()),
            OriginPolicy::Allowlist(_) => Err(forbidden_origin(
                "WebSocket Origin header is required when BOLT_WS_ALLOWED_ORIGINS is configured",
            )),
        };
    };

    let origin = origin
        .to_str()
        .map_err(|_| forbidden_origin("WebSocket Origin must be valid UTF-8"))?;

    if policy.allows(origin) {
        Ok(())
    } else {
        Err(forbidden_origin(
            "WebSocket Origin is not allowed; configure BOLT_WS_ALLOWED_ORIGINS",
        ))
    }
}

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
async fn accept_with_policy(
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

#[derive(Debug, thiserror::Error)]
pub enum WsError {
    #[error("WebSocket handshake failed: {0}")]
    Handshake(tokio_tungstenite::tungstenite::Error),
}

/// Adapter that wraps a `WebSocketStream` to implement `AsyncRead + AsyncWrite`.
///
/// Reads: receives WebSocket binary frames, buffers the payload, and serves
/// bytes from the buffer to the reader.
///
/// Writes: buffers bytes, and on `flush()` sends them as a single WebSocket
/// binary frame.
pub struct WsStreamAdapter {
    inner: WebSocketStream<TcpStream>,
    read_buf: Vec<u8>,
    read_pos: usize,
    write_buf: Vec<u8>,
}

impl WsStreamAdapter {
    pub fn new(stream: WebSocketStream<TcpStream>) -> Self {
        Self {
            inner: stream,
            read_buf: Vec::new(),
            read_pos: 0,
            write_buf: Vec::new(),
        }
    }
}

impl AsyncRead for WsStreamAdapter {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        // If we have buffered data from a previous frame, serve it first.
        if self.read_pos < self.read_buf.len() {
            let remaining = &self.read_buf[self.read_pos..];
            let to_copy = remaining.len().min(buf.remaining());
            buf.put_slice(&remaining[..to_copy]);
            self.read_pos += to_copy;
            // Reset buffer if fully consumed.
            if self.read_pos >= self.read_buf.len() {
                self.read_buf.clear();
                self.read_pos = 0;
            }
            return Poll::Ready(Ok(()));
        }

        // No buffered data — poll the WebSocket for the next frame.
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(None) => {
                // Stream ended — return 0 bytes (EOF).
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Some(Err(e))) => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                format!("WebSocket read error: {e}"),
            ))),
            Poll::Ready(Some(Ok(msg))) => {
                match msg {
                    Message::Binary(data) => {
                        let to_copy = data.len().min(buf.remaining());
                        buf.put_slice(&data[..to_copy]);
                        // Buffer any overflow.
                        if to_copy < data.len() {
                            self.read_buf = data[to_copy..].to_vec();
                            self.read_pos = 0;
                        }
                        Poll::Ready(Ok(()))
                    }
                    Message::Close(_) => {
                        // Treat close as EOF.
                        Poll::Ready(Ok(()))
                    }
                    Message::Ping(_) | Message::Pong(_) | Message::Text(_) => {
                        // Ignore non-binary frames; wake to try again.
                        cx.waker().wake_by_ref();
                        Poll::Pending
                    }
                    _ => {
                        cx.waker().wake_by_ref();
                        Poll::Pending
                    }
                }
            }
        }
    }
}

impl AsyncWrite for WsStreamAdapter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let max = ws_max_write_buf();
        if self.write_buf.len().saturating_add(buf.len()) > max {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::OutOfMemory,
                format!(
                    "WebSocket write buffer size {} would exceed maximum {}",
                    self.write_buf.len() + buf.len(),
                    max
                ),
            )));
        }
        // Buffer the bytes; they'll be sent as a WebSocket frame on flush.
        self.write_buf.extend_from_slice(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.write_buf.is_empty() {
            return Poll::Ready(Ok(()));
        }

        let data = std::mem::take(&mut self.write_buf);
        let msg = Message::Binary(data.into());

        // Feed the message into the sink.
        match Pin::new(&mut self.inner).poll_ready(cx) {
            Poll::Pending => {
                // Put the data back.
                self.write_buf = msg.into_data().to_vec();
                return Poll::Pending;
            }
            Poll::Ready(Err(e)) => {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    format!("WebSocket write error: {e}"),
                )));
            }
            Poll::Ready(Ok(())) => {}
        }

        if let Err(e) = Pin::new(&mut self.inner).start_send(msg) {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                format!("WebSocket send error: {e}"),
            )));
        }

        // Flush the underlying sink.
        match Pin::new(&mut self.inner).poll_flush(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(())) => Poll::Ready(Ok(())),
            Poll::Ready(Err(e)) => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                format!("WebSocket flush error: {e}"),
            ))),
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match Pin::new(&mut self.inner).poll_close(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(())) => Poll::Ready(Ok(())),
            Poll::Ready(Err(e)) => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                format!("WebSocket close error: {e}"),
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    /// Helper: creates a connected TCP pair (client, server) for testing.
    async fn tcp_pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let client = TcpStream::connect(addr).await.unwrap();
        let (server, _) = listener.accept().await.unwrap();
        (client, server)
    }

    fn websocket_upgrade_request(origin: Option<&str>) -> String {
        let origin_header = origin
            .map(|origin| format!("Origin: {origin}\r\n"))
            .unwrap_or_default();
        format!(
            "GET /bolt HTTP/1.1\r\n\
             Host: localhost\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
             Sec-WebSocket-Version: 13\r\n\
             {origin_header}\
             \r\n"
        )
    }

    #[tokio::test]
    async fn is_websocket_upgrade_detects_http_get() {
        let (mut client, server) = tcp_pair().await;
        tokio::io::AsyncWriteExt::write_all(&mut client, b"GET /")
            .await
            .unwrap();
        let result = is_websocket_upgrade(&server).await.unwrap();
        assert!(
            result,
            "'GET ' prefix should be detected as websocket upgrade"
        );
    }

    #[tokio::test]
    async fn is_websocket_upgrade_detects_bolt_magic() {
        let (mut client, server) = tcp_pair().await;
        tokio::io::AsyncWriteExt::write_all(&mut client, &[0x60])
            .await
            .unwrap();
        let result = is_websocket_upgrade(&server).await.unwrap();
        assert!(
            !result,
            "first byte 0x60 should NOT be detected as websocket upgrade"
        );
    }

    #[tokio::test]
    async fn is_websocket_upgrade_returns_false_on_eof() {
        let (client, server) = tcp_pair().await;
        drop(client);
        let result = is_websocket_upgrade(&server).await.unwrap();
        assert!(!result, "EOF should return false");
    }

    #[tokio::test]
    async fn is_websocket_upgrade_random_byte_is_not_ws() {
        let (mut client, server) = tcp_pair().await;
        tokio::io::AsyncWriteExt::write_all(&mut client, &[0xAB])
            .await
            .unwrap();
        let result = is_websocket_upgrade(&server).await.unwrap();
        assert!(
            !result,
            "random byte 0xAB should not be detected as websocket"
        );
    }

    #[tokio::test]
    async fn is_websocket_upgrade_waits_for_fragmented_http_prefix() {
        let (mut client, server) = tcp_pair().await;
        let detect = tokio::spawn(async move { is_websocket_upgrade(&server).await.unwrap() });

        client.write_all(b"G").await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        client.write_all(b"ET /").await.unwrap();

        let result = detect.await.unwrap();
        assert!(
            result,
            "fragmented GET prefix should still be detected as websocket"
        );
    }

    #[test]
    fn ws_error_display_format() {
        let err = WsError::Handshake(tokio_tungstenite::tungstenite::Error::ConnectionClosed);
        let msg = format!("{}", err);
        assert!(msg.contains("WebSocket handshake failed"));
    }

    #[test]
    fn ws_error_is_debug() {
        let err = WsError::Handshake(tokio_tungstenite::tungstenite::Error::ConnectionClosed);
        let dbg = format!("{:?}", err);
        assert!(dbg.contains("Handshake"));
    }

    #[tokio::test]
    async fn accept_rejects_disallowed_origin() {
        let (mut client, server) = tcp_pair().await;
        let accept_task = tokio::spawn(async move {
            accept_with_policy(
                server,
                OriginPolicy::Allowlist(vec![Box::<str>::from("https://app.example")]),
            )
            .await
        });

        client
            .write_all(websocket_upgrade_request(Some("https://evil.example")).as_bytes())
            .await
            .unwrap();

        let result = accept_task.await.unwrap();
        assert!(
            result.is_err(),
            "websocket upgrade should reject disallowed Origin values"
        );
    }

    #[tokio::test]
    async fn accept_allows_configured_origin() {
        let (mut client, server) = tcp_pair().await;
        let accept_task = tokio::spawn(async move {
            accept_with_policy(
                server,
                OriginPolicy::Allowlist(vec![Box::<str>::from("https://app.example")]),
            )
            .await
        });

        client
            .write_all(websocket_upgrade_request(Some("https://app.example")).as_bytes())
            .await
            .unwrap();

        let result = accept_task.await.unwrap();
        assert!(
            result.is_ok(),
            "websocket upgrade should accept allowed Origin values"
        );
    }

    #[tokio::test]
    async fn accept_rejects_missing_origin_when_allowlist_configured() {
        let (mut client, server) = tcp_pair().await;
        let accept_task = tokio::spawn(async move {
            accept_with_policy(
                server,
                OriginPolicy::Allowlist(vec![Box::<str>::from("https://app.example")]),
            )
            .await
        });

        client
            .write_all(websocket_upgrade_request(None).as_bytes())
            .await
            .unwrap();

        let result = accept_task.await.unwrap();
        assert!(
            result.is_err(),
            "websocket upgrade should reject missing Origin when an allowlist is configured"
        );
    }

    #[tokio::test]
    async fn accept_allows_missing_origin_when_policy_is_wildcard() {
        let (mut client, server) = tcp_pair().await;
        let accept_task =
            tokio::spawn(async move { accept_with_policy(server, OriginPolicy::AllowAll).await });

        client
            .write_all(websocket_upgrade_request(None).as_bytes())
            .await
            .unwrap();

        let result = accept_task.await.unwrap();
        assert!(
            result.is_ok(),
            "wildcard origin policy should allow websocket clients without an Origin header"
        );
    }
}
