//! Verifies that TCP keepalive can be set on accepted connections using the
//! same `socket2` approach as `main.rs`.
//!
//! This test does not start a full Bolt server -- it only confirms that the
//! keepalive socket options take effect on a real TCP stream, matching the
//! pattern used in the accept loop.

use std::time::Duration;

use socket2::SockRef;
use tokio::net::TcpListener;

#[tokio::test]
async fn keepalive_is_set_on_accepted_connection() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind");
    let addr = listener.local_addr().unwrap();

    // Connect from the client side so we can accept on the server side.
    let client = tokio::net::TcpStream::connect(addr)
        .await
        .expect("connect failed");
    let (server_stream, _peer) = listener.accept().await.expect("accept failed");

    // Configure keepalive exactly as main.rs does.
    let keepalive = socket2::TcpKeepalive::new()
        .with_time(Duration::from_secs(120))
        .with_interval(Duration::from_secs(30))
        .with_retries(3);

    let sock = SockRef::from(&server_stream);
    sock.set_tcp_keepalive(&keepalive)
        .expect("set_tcp_keepalive failed on server stream");

    // Verify SO_KEEPALIVE is enabled.
    assert!(
        sock.keepalive().expect("failed to read keepalive"),
        "SO_KEEPALIVE should be enabled on the server socket"
    );

    // Also verify it works on the client side (symmetry check).
    let client_sock = SockRef::from(&client);
    client_sock
        .set_tcp_keepalive(&keepalive)
        .expect("set_tcp_keepalive failed on client stream");
    assert!(
        client_sock.keepalive().expect("failed to read keepalive"),
        "SO_KEEPALIVE should be enabled on the client socket"
    );
}
