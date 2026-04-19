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

mod adapter;
mod handshake;
mod origin;

pub use adapter::WsStreamAdapter;
pub use handshake::{accept, is_websocket_upgrade};

#[derive(Debug, thiserror::Error)]
pub enum WsError {
    #[error("WebSocket handshake failed: {0}")]
    Handshake(tokio_tungstenite::tungstenite::Error),
}

#[cfg(test)]
mod tests {
    use super::handshake::accept_with_policy;
    use super::origin::OriginPolicy;
    use super::*;
    use tokio::io::AsyncWriteExt;
    use tokio::net::{TcpListener, TcpStream};

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
