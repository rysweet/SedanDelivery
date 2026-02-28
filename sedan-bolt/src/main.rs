use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use tokio::net::TcpListener;

use gql_core::storage::InMemoryGraphStore;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let bind_addr = std::env::var("BOLT_BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:7687".to_string());

    let store = Arc::new(InMemoryGraphStore::new());

    let listener = TcpListener::bind(&bind_addr)
        .await
        .expect("failed to bind Bolt TCP listener");
    tracing::info!("sedan-bolt listening on {}", bind_addr);

    let connection_counter = Arc::new(AtomicU64::new(0));
    // Single global bookmark counter shared across all sessions so that
    // bookmarks produced by one session are meaningful when passed to another.
    let global_bookmark = Arc::new(AtomicU64::new(0));

    loop {
        let (stream, peer_addr) = listener.accept().await.expect("accept failed");
        let store = Arc::clone(&store);
        let conn_id = connection_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let bookmark = Arc::clone(&global_bookmark);

        tokio::spawn(async move {
            tracing::info!(%peer_addr, conn_id, "new connection");
            if let Err(e) = sedan_bolt::connection::session::run(stream, store, conn_id, bookmark).await {
                tracing::warn!(%peer_addr, conn_id, "session error: {}", e);
            }
            tracing::info!(%peer_addr, conn_id, "connection closed");
        });
    }
}
