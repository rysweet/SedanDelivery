//! Integration tests that exercise `FabricGraphStore` against an in-process
//! HTTP mock server. The mock server speaks just enough of the Azure AD OAuth
//! protocol and the Fabric Graph REST API to drive every `*_impl` function
//! through the public `GraphStore` trait, the `query`/`statement` HTTP
//! plumbing, and the OAuth token cache + refresh paths.
//!
//! These tests do not perform any real network I/O — the listener binds to
//! `127.0.0.1:0` and is torn down with the test.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use indexmap::IndexMap;
use serde_json::{json, Value as JValue};

use cypher_core::storage::{EdgeId, GraphStore, NodeId, RangeQuery};
use cypher_core::types::Value;

use gql_fabric_store::FabricGraphStore;

// ── Mock server ─────────────────────────────────────────────────────────────

type Handler = dyn Fn(&Request) -> Response + Send + Sync + 'static;

#[derive(Debug, Clone)]
struct Request {
    #[allow(dead_code)]
    method: String,
    path: String,
    #[allow(dead_code)]
    headers: HashMap<String, String>,
    body: String,
}

#[derive(Debug, Clone)]
struct Response {
    status: u16,
    body: String,
}

impl Response {
    fn json(value: JValue) -> Self {
        Response {
            status: 200,
            body: value.to_string(),
        }
    }
    fn json_status(status: u16, value: JValue) -> Self {
        Response {
            status,
            body: value.to_string(),
        }
    }
}

struct MockServer {
    addr: String,
    shutdown: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl MockServer {
    fn start<F>(handler: F) -> Self
    where
        F: Fn(&Request) -> Response + Send + Sync + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock listener");
        listener
            .set_nonblocking(true)
            .expect("set non-blocking accept");
        let addr = format!("http://{}", listener.local_addr().unwrap());
        let shutdown = Arc::new(AtomicBool::new(false));
        let handler: Arc<Handler> = Arc::new(handler);

        let sd_clone = shutdown.clone();
        let handle = thread::spawn(move || {
            while !sd_clone.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let h = handler.clone();
                        thread::spawn(move || {
                            let _ = handle_conn(stream, h);
                        });
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });

        MockServer {
            addr,
            shutdown,
            handle: Some(handle),
        }
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn handle_conn(mut stream: TcpStream, handler: Arc<Handler>) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;

    let mut buf = Vec::with_capacity(4096);
    let mut tmp = [0u8; 1024];
    let header_end = loop {
        let n = stream.read(&mut tmp)?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = find_double_crlf(&buf) {
            break pos;
        }
        if buf.len() > 1 << 20 {
            return Ok(());
        }
    };

    let head = std::str::from_utf8(&buf[..header_end]).unwrap_or("");
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();

    let mut headers = HashMap::new();
    let mut content_length = 0usize;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            let kl = k.trim().to_ascii_lowercase();
            let vt = v.trim().to_string();
            if kl == "content-length" {
                content_length = vt.parse().unwrap_or(0);
            }
            headers.insert(kl, vt);
        }
    }

    let body_start = header_end + 4;
    let mut body_bytes = buf[body_start..].to_vec();
    while body_bytes.len() < content_length {
        let need = content_length - body_bytes.len();
        let mut chunk = vec![0u8; need.min(8192)];
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        body_bytes.extend_from_slice(&chunk[..n]);
    }
    let body = String::from_utf8_lossy(&body_bytes).into_owned();

    let req = Request {
        method,
        path,
        headers,
        body,
    };

    let resp = handler(&req);
    let status_text = match resp.status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        500 => "Internal Server Error",
        _ => "Status",
    };
    let response = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        resp.status,
        status_text,
        resp.body.len(),
        resp.body
    );
    stream.write_all(response.as_bytes())?;
    let _ = stream.shutdown(std::net::Shutdown::Write);
    Ok(())
}

fn find_double_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn make_store(mock: &MockServer) -> FabricGraphStore {
    let endpoint = format!("{}/v1/workspaces/ws/graphDatabases/db", mock.addr);
    let token_url = format!("{}/oauth2/token", mock.addr);
    FabricGraphStore::new(endpoint, "tenant".into(), "client".into(), "secret".into())
        .with_token_url(token_url)
}

fn fabric_rows(rows: Vec<Vec<JValue>>) -> JValue {
    json!({ "results": [{ "headers": [], "data": rows }] })
}

fn token_resp(tok: &str, expires_in: u64) -> JValue {
    json!({ "access_token": tok, "expires_in": expires_in, "token_type": "Bearer" })
}

// ── Tests: HTTP plumbing + auth ─────────────────────────────────────────────

#[test]
fn token_fetch_succeeds_then_caches() {
    let counter = Arc::new(AtomicUsize::new(0));
    let c2 = counter.clone();
    let server = MockServer::start(move |req| {
        if req.path.contains("/oauth2/token") {
            c2.fetch_add(1, Ordering::SeqCst);
            return Response::json(token_resp("tok-A", 3600));
        }
        Response::json(fabric_rows(vec![]))
    });

    let store = make_store(&server);
    let _ = store.list_graphs().unwrap();
    let _ = store.list_graphs().unwrap();
    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "token should be cached after first fetch"
    );
}

#[test]
fn token_fetch_errors_on_missing_access_token() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/token") {
            return Response::json(json!({ "expires_in": 3600 }));
        }
        Response::json(fabric_rows(vec![]))
    });
    let store = make_store(&server);
    let err = store.list_graphs().unwrap_err();
    assert!(format!("{err}").contains("no access_token"), "{err}");
}

#[test]
fn token_fetch_errors_on_http_failure() {
    let server = MockServer::start(|_| Response::json_status(500, json!({"msg": "boom"})));
    let store = make_store(&server);
    let err = store.list_graphs().unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("token"), "{msg}");
}

#[test]
fn token_response_missing_expires_in_uses_default() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/token") {
            return Response::json(json!({ "access_token": "tok" }));
        }
        Response::json(fabric_rows(vec![]))
    });
    let store = make_store(&server);
    let _ = store.list_graphs().unwrap();
}

// ── Tests: Node read paths ──────────────────────────────────────────────────

#[test]
fn get_node_returns_some_when_row_present() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        let node = json!({
            "labels": ["Person"],
            "properties": {
                "_sd2_id": 7,
                "_sd2_graph": "g",
                "_sd2_labels": "[\"Person\"]",
                "name": "Alice",
                "age": 30,
            }
        });
        Response::json(fabric_rows(vec![vec![node]]))
    });
    let store = make_store(&server);
    let node = store.get_node("g", NodeId(7)).unwrap().unwrap();
    assert_eq!(node.id, NodeId(7));
    assert_eq!(node.labels, vec!["Person".to_string()]);
    assert_eq!(node.properties["name"], Value::Str("Alice".into()));
}

#[test]
fn get_node_returns_none_when_row_null() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        Response::json(fabric_rows(vec![vec![JValue::Null]]))
    });
    let store = make_store(&server);
    assert!(store.get_node("g", NodeId(99)).unwrap().is_none());
}

#[test]
fn get_node_returns_none_on_empty_rows() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        Response::json(fabric_rows(vec![]))
    });
    let store = make_store(&server);
    assert!(store.get_node("g", NodeId(1)).unwrap().is_none());
}

#[test]
fn get_node_serialization_error_when_missing_id() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        let node = json!({ "labels": [], "properties": { "name": "x" } });
        Response::json(fabric_rows(vec![vec![node]]))
    });
    let store = make_store(&server);
    let err = store.get_node("g", NodeId(1)).unwrap_err();
    assert!(format!("{err}").contains("_sd2_id"), "{err}");
}

#[test]
fn get_nodes_batch_empty_returns_empty() {
    let server = MockServer::start(|_| Response::json(token_resp("t", 3600)));
    let store = make_store(&server);
    let rs = store.get_nodes_batch("g", &[]).unwrap();
    assert!(rs.is_empty());
}

#[test]
fn get_nodes_batch_returns_rows() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        let n = |i: u64| {
            json!({
                "labels": ["L"],
                "properties": { "_sd2_id": i, "_sd2_labels": "[\"L\"]" }
            })
        };
        Response::json(fabric_rows(vec![
            vec![n(1)],
            vec![JValue::Null],
            vec![n(3)],
        ]))
    });
    let store = make_store(&server);
    let r = store
        .get_nodes_batch("g", &[NodeId(1), NodeId(2), NodeId(3)])
        .unwrap();
    assert_eq!(r.len(), 3);
    assert_eq!(r[0].as_ref().unwrap().id, NodeId(1));
    assert!(r[1].is_none());
    assert_eq!(r[2].as_ref().unwrap().id, NodeId(3));
}

#[test]
fn find_nodes_by_label_returns_ids() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        Response::json(fabric_rows(vec![
            vec![json!(1)],
            vec![json!(2)],
            vec![json!(5)],
        ]))
    });
    let store = make_store(&server);
    let ids = store.find_nodes_by_label("g", "Person").unwrap();
    assert_eq!(ids, vec![NodeId(1), NodeId(2), NodeId(5)]);
}

#[test]
fn find_nodes_by_label_empty_label_errors() {
    let server = MockServer::start(|_| Response::json(token_resp("t", 3600)));
    let store = make_store(&server);
    let err = store.find_nodes_by_label("g", "").unwrap_err();
    assert!(format!("{err}").contains("invalid label"));
}

#[test]
fn find_nodes_by_label_and_property_eq_range() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        Response::json(fabric_rows(vec![vec![json!(11)]]))
    });
    let store = make_store(&server);
    let ids = store
        .find_nodes_by_label_and_property(
            "g",
            "Person",
            "name",
            RangeQuery::Eq(&Value::Str("Bob".into())),
        )
        .unwrap();
    assert_eq!(ids, vec![NodeId(11)]);
}

#[test]
fn all_node_ids_returns_ids() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        Response::json(fabric_rows(vec![vec![json!(10)], vec![json!(20)]]))
    });
    let store = make_store(&server);
    let ids = store.all_node_ids("g").unwrap();
    assert_eq!(ids, vec![NodeId(10), NodeId(20)]);
}

#[test]
fn all_label_names_collects_unique_labels() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        Response::json(fabric_rows(vec![
            vec![json!("[\"Person\",\"Employee\"]")],
            vec![json!("[\"Person\"]")],
            vec![json!("not-json")],
        ]))
    });
    let store = make_store(&server);
    let mut labels = store.all_label_names("g").unwrap();
    labels.sort();
    assert_eq!(labels, vec!["Employee".to_string(), "Person".to_string()]);
}

#[test]
fn out_in_edges_return_adj_entries() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        Response::json(fabric_rows(vec![
            vec![json!(100), json!(2), json!("KNOWS")],
            vec![json!(101), json!(3), json!("LIKES")],
        ]))
    });
    let store = make_store(&server);
    assert_eq!(store.out_edges("g", NodeId(1)).unwrap().len(), 2);
    assert_eq!(store.in_edges("g", NodeId(1)).unwrap().len(), 2);
    assert_eq!(
        store
            .out_edges_by_type("g", NodeId(1), &["KNOWS"])
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        store
            .in_edges_by_type("g", NodeId(1), &["LIKES"])
            .unwrap()
            .len(),
        2
    );
    // Empty rel_types delegates to non-typed variant.
    assert_eq!(
        store.out_edges_by_type("g", NodeId(1), &[]).unwrap().len(),
        2
    );
    assert_eq!(
        store.in_edges_by_type("g", NodeId(1), &[]).unwrap().len(),
        2
    );
}

#[test]
fn out_and_in_edge_ids() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        Response::json(fabric_rows(vec![vec![json!(7)], vec![json!(8)]]))
    });
    let store = make_store(&server);
    assert_eq!(store.out_edge_ids("g", NodeId(1)).unwrap().len(), 2);
    assert_eq!(store.in_edge_ids("g", NodeId(1)).unwrap().len(), 2);
}

// ── Tests: Node write paths ─────────────────────────────────────────────────

#[test]
fn create_node_uses_provided_sd2_id() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        Response::json(fabric_rows(vec![]))
    });
    let store = make_store(&server);
    let mut props = IndexMap::new();
    props.insert("_sd2_id".to_string(), Value::Int(42));
    props.insert("name".to_string(), Value::Str("X".into()));
    let id = store.create_node("g", vec!["A".into()], props).unwrap();
    assert_eq!(id, NodeId(42));
}

#[test]
fn create_node_negative_sd2_id_errors() {
    let server = MockServer::start(|_| Response::json(token_resp("t", 3600)));
    let store = make_store(&server);
    let mut props = IndexMap::new();
    props.insert("_sd2_id".to_string(), Value::Int(-5));
    let err = store.create_node("g", vec![], props).unwrap_err();
    assert!(format!("{err}").contains("must be non-negative"));
}

#[test]
fn create_node_allocates_id_via_sequence() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        if req.body.contains("c.node_seq") {
            return Response::json(fabric_rows(vec![vec![json!(5)]]));
        }
        Response::json(fabric_rows(vec![]))
    });
    let store = make_store(&server);
    let id = store
        .create_node("g", vec!["L".into()], IndexMap::new())
        .unwrap();
    assert_eq!(id, NodeId(5));
}

#[test]
fn next_node_id_errors_when_seq_missing() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        // Sequence query returns no rows → triggers "no node_seq returned".
        Response::json(fabric_rows(vec![]))
    });
    let store = make_store(&server);
    let err = store
        .create_node("g", vec!["L".into()], IndexMap::new())
        .unwrap_err();
    assert!(format!("{err}").contains("node_seq"));
}

#[test]
fn set_replace_remove_label_node_writes_all_succeed() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        if req.body.contains("RETURN labels(n)") {
            return Response::json(fabric_rows(vec![vec![json!(["A", "B", "__SD2x"])]]));
        }
        Response::json(fabric_rows(vec![]))
    });
    let store = make_store(&server);

    let mut props = IndexMap::new();
    props.insert("k".into(), Value::Int(1));
    store
        .set_node_properties("g", NodeId(1), props.clone())
        .unwrap();
    store
        .replace_node_properties("g", NodeId(1), props.clone())
        .unwrap();
    store.remove_node_property("g", NodeId(1), "k").unwrap();
    store
        .add_node_labels("g", NodeId(1), &["X".into()])
        .unwrap();
    store
        .remove_node_labels("g", NodeId(1), &["X".into()])
        .unwrap();
    // Empty/sanitized-to-empty label vec → early return Ok(()).
    store.add_node_labels("g", NodeId(1), &[]).unwrap();
    store.remove_node_labels("g", NodeId(1), &[]).unwrap();
    store.add_node_labels("g", NodeId(1), &["".into()]).unwrap();
    store
        .remove_node_labels("g", NodeId(1), &["".into()])
        .unwrap();
}

#[test]
fn delete_node_detach_skips_relationship_check() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        Response::json(fabric_rows(vec![]))
    });
    let store = make_store(&server);
    store.delete_node("g", NodeId(1), true).unwrap();
}

#[test]
fn delete_node_without_detach_blocks_when_relationships_exist() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        Response::json(fabric_rows(vec![vec![json!(3)]]))
    });
    let store = make_store(&server);
    let err = store.delete_node("g", NodeId(7), false).unwrap_err();
    assert!(format!("{err}").contains("DETACH DELETE"));
}

#[test]
fn delete_node_without_detach_succeeds_when_zero_relationships() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        Response::json(fabric_rows(vec![vec![json!(0)]]))
    });
    let store = make_store(&server);
    store.delete_node("g", NodeId(1), false).unwrap();
}

// ── Tests: Edge paths ───────────────────────────────────────────────────────

#[test]
fn get_edge_round_trip() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        let edge = json!({
            "type": "KNOWS",
            "properties": {
                "_sd2_eid": 9,
                "_sd2_src": 1,
                "_sd2_dst": 2,
                "_sd2_graph": "g",
                "since": 2024,
            }
        });
        Response::json(fabric_rows(vec![vec![edge]]))
    });
    let store = make_store(&server);
    let e = store.get_edge("g", EdgeId(9)).unwrap().unwrap();
    assert_eq!(e.rel_type, "KNOWS");
    assert_eq!(e.src, NodeId(1));
    assert_eq!(e.dst, NodeId(2));
}

#[test]
fn get_edge_returns_none_when_row_null() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        Response::json(fabric_rows(vec![vec![JValue::Null]]))
    });
    let store = make_store(&server);
    assert!(store.get_edge("g", EdgeId(1)).unwrap().is_none());
}

#[test]
fn get_edge_serialization_errors_when_missing_eid() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        Response::json(fabric_rows(vec![vec![json!({
            "type": "T",
            "properties": { "_sd2_src": 1, "_sd2_dst": 2 }
        })]]))
    });
    let s = make_store(&server);
    let err = s.get_edge("g", EdgeId(1)).unwrap_err();
    assert!(format!("{err}").contains("_sd2_eid"));
}

#[test]
fn get_edge_serialization_errors_when_missing_type() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        Response::json(fabric_rows(vec![vec![json!({
            "properties": { "_sd2_eid": 1, "_sd2_src": 1, "_sd2_dst": 2 }
        })]]))
    });
    let s = make_store(&server);
    let err = s.get_edge("g", EdgeId(1)).unwrap_err();
    assert!(format!("{err}").contains("relationship type"));
}

#[test]
fn get_edges_batch_empty_returns_empty() {
    let server = MockServer::start(|_| Response::json(token_resp("t", 3600)));
    let store = make_store(&server);
    assert!(store.get_edges_batch("g", &[]).unwrap().is_empty());
}

#[test]
fn get_edges_batch_returns_rows() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        let edge = |i: u64| {
            json!({
                "type": "T",
                "properties": { "_sd2_eid": i, "_sd2_src": 1, "_sd2_dst": 2 }
            })
        };
        Response::json(fabric_rows(vec![vec![edge(1)], vec![JValue::Null]]))
    });
    let store = make_store(&server);
    let r = store.get_edges_batch("g", &[EdgeId(1), EdgeId(2)]).unwrap();
    assert_eq!(r.len(), 2);
    assert!(r[0].is_some());
    assert!(r[1].is_none());
}

#[test]
fn find_edges_by_type_and_all_edge_ids() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        Response::json(fabric_rows(vec![vec![json!(7)]]))
    });
    let store = make_store(&server);
    assert_eq!(store.find_edges_by_type("g", "KNOWS").unwrap().len(), 1);
    assert_eq!(store.all_edge_ids("g").unwrap().len(), 1);
}

#[test]
fn find_edges_by_type_empty_errors() {
    let server = MockServer::start(|_| Response::json(token_resp("t", 3600)));
    let store = make_store(&server);
    let err = store.find_edges_by_type("g", "").unwrap_err();
    assert!(format!("{err}").contains("invalid relationship type"));
}

#[test]
fn create_edge_uses_provided_sd2_eid() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        Response::json(fabric_rows(vec![vec![json!(99)]]))
    });
    let store = make_store(&server);
    let mut props = IndexMap::new();
    props.insert("_sd2_eid".into(), Value::Int(99));
    let id = store
        .create_edge("g", NodeId(1), NodeId(2), "KNOWS".into(), props)
        .unwrap();
    assert_eq!(id, EdgeId(99));
}

#[test]
fn create_edge_negative_sd2_eid_errors() {
    let server = MockServer::start(|_| Response::json(token_resp("t", 3600)));
    let store = make_store(&server);
    let mut props = IndexMap::new();
    props.insert("_sd2_eid".into(), Value::Int(-1));
    let err = store
        .create_edge("g", NodeId(1), NodeId(2), "T".into(), props)
        .unwrap_err();
    assert!(format!("{err}").contains("must be non-negative"));
}

#[test]
fn create_edge_allocates_via_sequence() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        if req.body.contains("c.edge_seq") {
            return Response::json(fabric_rows(vec![vec![json!(11)]]));
        }
        Response::json(fabric_rows(vec![vec![json!(11)]]))
    });
    let store = make_store(&server);
    let id = store
        .create_edge("g", NodeId(1), NodeId(2), "KNOWS".into(), IndexMap::new())
        .unwrap();
    assert_eq!(id, EdgeId(11));
}

#[test]
fn create_edge_errors_when_no_rows() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        if req.body.contains("c.edge_seq") {
            return Response::json(fabric_rows(vec![vec![json!(1)]]));
        }
        Response::json(fabric_rows(vec![]))
    });
    let store = make_store(&server);
    let err = store
        .create_edge("g", NodeId(1), NodeId(2), "KNOWS".into(), IndexMap::new())
        .unwrap_err();
    assert!(format!("{err}").contains("not found"));
}

#[test]
fn next_edge_id_errors_when_seq_missing() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        Response::json(fabric_rows(vec![]))
    });
    let store = make_store(&server);
    let err = store
        .create_edge("g", NodeId(1), NodeId(2), "K".into(), IndexMap::new())
        .unwrap_err();
    assert!(format!("{err}").contains("edge_seq"));
}

#[test]
fn set_remove_delete_edge_succeed() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        Response::json(fabric_rows(vec![]))
    });
    let store = make_store(&server);
    let mut p = IndexMap::new();
    p.insert("k".into(), Value::Int(1));
    store.set_edge_properties("g", EdgeId(1), p).unwrap();
    store.remove_edge_property("g", EdgeId(1), "k").unwrap();
    store.delete_edge("g", EdgeId(1)).unwrap();
}

#[test]
fn replace_edge_properties_uses_existing_edge() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        if req.body.contains("RETURN r LIMIT 1") {
            return Response::json(fabric_rows(vec![vec![json!({
                "type": "T",
                "properties": { "_sd2_eid": 7, "_sd2_src": 1, "_sd2_dst": 2 }
            })]]));
        }
        Response::json(fabric_rows(vec![]))
    });
    let store = make_store(&server);
    let mut p = IndexMap::new();
    p.insert("k".into(), Value::Int(2));
    store.replace_edge_properties("g", EdgeId(7), p).unwrap();
}

#[test]
fn replace_edge_properties_missing_edge_errors() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        Response::json(fabric_rows(vec![]))
    });
    let store = make_store(&server);
    let err = store
        .replace_edge_properties("g", EdgeId(7), IndexMap::new())
        .unwrap_err();
    assert!(matches!(
        err,
        cypher_core::error::StorageError::EdgeNotFound { id: 7 }
    ));
}

// ── Tests: Admin paths ──────────────────────────────────────────────────────

#[test]
fn list_indexes_returns_descriptors() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        Response::json(fabric_rows(vec![
            vec![json!("Person"), json!("name")],
            vec![json!("Movie"), json!("title")],
        ]))
    });
    let store = make_store(&server);
    let idx = store.list_indexes("g").unwrap();
    assert_eq!(idx.len(), 2);
    assert_eq!(idx[0].label, "Person");
    assert_eq!(idx[1].property, "title");
}

#[test]
fn create_and_drop_index_succeed() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        Response::json(fabric_rows(vec![]))
    });
    let store = make_store(&server);
    store.create_index("g", "Person", "name").unwrap();
    store.drop_index("g", "Person", "name").unwrap();
}

#[test]
fn create_drop_index_with_empty_label_or_prop_is_noop() {
    let server = MockServer::start(|_| Response::json(token_resp("t", 3600)));
    let store = make_store(&server);
    store.create_index("g", "", "name").unwrap();
    store.create_index("g", "Person", "").unwrap();
    store.drop_index("g", "", "name").unwrap();
    store.drop_index("g", "Person", "").unwrap();
}

#[test]
fn create_delete_graph_succeed() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        Response::json(fabric_rows(vec![]))
    });
    let store = make_store(&server);
    store.create_graph("my_graph").unwrap();
    store.delete_graph("my_graph").unwrap();
}

#[test]
fn graph_exists_true_and_false() {
    let calls = Arc::new(AtomicUsize::new(0));
    let c2 = calls.clone();
    let server = MockServer::start(move |req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        let n = c2.fetch_add(1, Ordering::SeqCst);
        let count = if n == 0 { 1 } else { 0 };
        Response::json(fabric_rows(vec![vec![json!(count)]]))
    });
    let store = make_store(&server);
    assert!(store.graph_exists("g").unwrap());
    assert!(!store.graph_exists("g").unwrap());
}

#[test]
fn list_graphs_returns_names() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        Response::json(fabric_rows(vec![vec![json!("g1")], vec![json!("g2")]]))
    });
    let store = make_store(&server);
    let names = store.list_graphs().unwrap();
    assert_eq!(names, vec!["g1".to_string(), "g2".to_string()]);
}

#[test]
fn graph_stats_parses_four_columns() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        Response::json(fabric_rows(vec![vec![
            json!(10),
            json!(20),
            json!(3),
            json!(2),
        ]]))
    });
    let store = make_store(&server);
    let s = store.graph_stats("g").unwrap();
    assert_eq!(s.node_count, 10);
    assert_eq!(s.edge_count, 20);
    assert_eq!(s.label_count, 3);
    assert_eq!(s.rel_type_count, 2);
}

#[test]
fn graph_stats_errors_on_empty_rows() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        Response::json(fabric_rows(vec![]))
    });
    let store = make_store(&server);
    let err = store.graph_stats("g").unwrap_err();
    assert!(format!("{err}").contains("no rows"));
}

#[test]
fn graph_stats_errors_on_unparseable_column() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        Response::json(fabric_rows(vec![vec![
            json!(10),
            json!("not a number"),
            json!(3),
            json!(2),
        ]]))
    });
    let store = make_store(&server);
    let err = store.graph_stats("g").unwrap_err();
    assert!(format!("{err}").contains("edge_count"));
}

// ── Misc: error paths through the HTTP layer ────────────────────────────────

#[test]
fn fabric_response_with_error_field_propagates() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        Response::json(json!({ "error": "boom" }))
    });
    let store = make_store(&server);
    let err = store.list_graphs().unwrap_err();
    assert!(format!("{err}").contains("Fabric API error"));
}

#[test]
fn fabric_invalid_json_response_errors() {
    let server = MockServer::start(|req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        Response {
            status: 200,
            body: "not-json{{".into(),
        }
    });
    let store = make_store(&server);
    let err = store.list_graphs().unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("response parse") || msg.contains("Fabric"),
        "{msg}"
    );
}

#[test]
fn graph_model_endpoint_uses_execute_query() {
    let path_seen: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let p2 = path_seen.clone();
    let server = MockServer::start(move |req| {
        if req.path.contains("/oauth2/") {
            return Response::json(token_resp("t", 3600));
        }
        *p2.lock().unwrap() = Some(req.path.clone());
        Response::json(fabric_rows(vec![]))
    });
    let endpoint = format!("{}/v1/workspaces/ws/GraphModels/gm", server.addr);
    let store = FabricGraphStore::new(endpoint, "tenant".into(), "client".into(), "secret".into())
        .with_token_url(format!("{}/oauth2/token", server.addr));
    let _ = store.list_graphs().unwrap();
    let p = path_seen.lock().unwrap().clone().unwrap();
    assert!(p.contains("/executeQuery?preview=true"), "got: {p}");
}
