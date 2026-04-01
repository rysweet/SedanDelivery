//! Gremlin WebSocket server for SedanDelivery.
//!
//! Accepts Gremlin text queries (`eval` op) over WebSocket and routes them
//! through gremlin-core's text parser → compiler → cypher-core executor.
//! Returns results in GraphSON v3 format.

mod executor;
mod protocol;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cypher_core::storage::GraphStore;
use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio_tungstenite::tungstenite::Message;

fn create_store() -> Arc<dyn GraphStore> {
    sd2_redis_store::create_store::create_store()
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(true)
        .init();

    let bind_addr = env_or("GREMLIN_SERVER_BIND_ADDR", "0.0.0.0:8182");
    let max_connections: usize =
        cypher_core::config::parse_env_range("GREMLIN_SERVER_MAX_CONNECTIONS", 1024usize, 1, 65535);
    let idle_timeout_secs: u64 =
        cypher_core::config::parse_env_range("GREMLIN_SERVER_IDLE_TIMEOUT_SECS", 300u64, 1, 86400);
    let idle_timeout = Duration::from_secs(idle_timeout_secs);

    let store = create_store();
    tracing::info!("graph store initialised");

    // Pre-create default graph
    match store.create_graph("default") {
        Ok(()) => {}
        Err(cypher_core::error::StorageError::GraphAlreadyExists { .. }) => {}
        Err(e) => panic!("failed to create default graph: {e}"),
    }

    let active_connections = Arc::new(AtomicU64::new(0));
    let connection_semaphore = Arc::new(Semaphore::new(max_connections));

    let listener = TcpListener::bind(&bind_addr).await.unwrap_or_else(|e| {
        eprintln!("failed to bind {bind_addr}: {e}");
        std::process::exit(1);
    });
    tracing::info!(%bind_addr, "gremlin-server WebSocket server listening");

    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("failed to register SIGTERM handler");

    loop {
        let accept_result = tokio::select! {
            biased;
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("received SIGINT, shutting down");
                break;
            }
            _ = sigterm.recv() => {
                tracing::info!("received SIGTERM, shutting down");
                break;
            }
            result = listener.accept() => result,
        };

        let (stream, peer_addr) = match accept_result {
            Ok(pair) => pair,
            Err(e) => {
                tracing::error!("accept error: {e}");
                continue;
            }
        };

        let permit = match Arc::clone(&connection_semaphore).acquire_owned().await {
            Ok(permit) => permit,
            Err(_) => break,
        };
        let store = Arc::clone(&store);
        let active = Arc::clone(&active_connections);
        active.fetch_add(1, Ordering::Relaxed);

        tokio::spawn(async move {
            let _permit = permit;
            let _guard = ConnectionGuard(Arc::clone(&active));
            tracing::debug!(%peer_addr, "new connection");

            if let Err(e) = handle_connection(stream, store, idle_timeout).await {
                tracing::warn!(%peer_addr, "connection error: {e}");
            }
        });
    }
}

async fn handle_connection(
    stream: TcpStream,
    store: Arc<dyn GraphStore>,
    idle_timeout: Duration,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig {
        max_frame_size: Some(64 * 1024),
        ..Default::default()
    };
    let ws_stream = tokio_tungstenite::accept_async_with_config(stream, Some(config)).await?;
    let (mut sink, mut source) = ws_stream.split();

    let graph = env_or("GREMLIN_SERVER_DEFAULT_GRAPH", "default");

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
                    let resp = protocol::ResponseMessage::error(uuid::Uuid::new_v4(), 498, e);
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
                let resp = protocol::ResponseMessage::error(uuid::Uuid::new_v4(), 498, e);
                let json = serde_json::to_string(&resp)?;
                sink.send(Message::Text(json)).await?;
                continue;
            }
        };

        let response = handle_request(&req, &store, &graph).await;
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

    // Extract bindings (parameter map)
    let bindings: HashMap<String, serde_json::Value> = req
        .args
        .get("bindings")
        .and_then(|v| v.as_object())
        .map(|obj| obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();

    // Determine graph from aliases (e.g. {"g": "mygraph"})
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

struct ConnectionGuard(Arc<AtomicU64>);

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}
