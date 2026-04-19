//! Driver-based Bolt v4.4 conformance tests.
//!
//! These tests connect a real Neo4j Rust driver (`neo4rs`) to a running
//! `sedandelivery-bolt` server and exercise the wire protocol end-to-end:
//! handshake, RUN/PULL framing, transaction lifecycle, parameterized queries,
//! result types (nodes / edges / lists / maps / temporals), and error paths.
//!
//! All tests are marked `#[ignore]` so that `cargo test --workspace` does not
//! require a live server. Run them via `scripts/run_bolt_conformance.sh`,
//! which boots an in-memory Bolt server on a random port and invokes
//! `cargo test -p bolt-driver-conformance -- --ignored --test-threads=1`.
//!
//! Connection target: `BOLT_URI` env var (default `bolt://127.0.0.1:7687`).
//! Auth defaults to `neo4j`/`neo4j` and is ignored by the server when
//! `BOLT_PASSWORD` is unset.

use std::env;

use neo4rs::{query, BoltType, ConfigBuilder, Graph};

fn bolt_uri() -> String {
    env::var("BOLT_URI").unwrap_or_else(|_| "bolt://127.0.0.1:7687".to_string())
}

fn bolt_user() -> String {
    env::var("BOLT_USER").unwrap_or_else(|_| "neo4j".to_string())
}

fn bolt_password() -> String {
    env::var("BOLT_PASSWORD").unwrap_or_else(|_| "neo4j".to_string())
}

async fn connect() -> Graph {
    connect_with_pool(4).await
}

async fn connect_with_pool(pool: usize) -> Graph {
    let cfg = ConfigBuilder::default()
        .uri(bolt_uri())
        .user(bolt_user())
        .password(bolt_password())
        .max_connections(pool)
        .fetch_size(256)
        .build()
        .expect("valid neo4rs config");
    Graph::connect(cfg)
        .await
        .expect("Bolt handshake / HELLO failed — is sedandelivery-bolt running on $BOLT_URI?")
}

// ---------------------------------------------------------------------------
// Handshake / HELLO / version negotiation
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn handshake_and_trivial_query() {
    let graph = connect().await;
    let mut rows = graph
        .execute(query("RETURN 1 AS n"))
        .await
        .expect("RETURN 1 succeeds");
    let row = rows.next().await.expect("stream").expect("row present");
    let n: i64 = row.get("n").expect("n is an integer");
    assert_eq!(n, 1);
    assert!(rows.next().await.expect("stream").is_none());
}

#[tokio::test]
#[ignore]
async fn multiple_sequential_queries_on_one_connection() {
    let graph = connect_with_pool(1).await;
    for i in 0..10_i64 {
        let mut rows = graph
            .execute(query("RETURN $i * 2 AS r").param("i", i))
            .await
            .expect("RUN succeeds");
        let row = rows.next().await.expect("stream").expect("row");
        let r: i64 = row.get("r").unwrap();
        assert_eq!(r, i * 2);
    }
}

#[tokio::test]
#[ignore]
async fn many_parallel_connections() {
    let graph = connect_with_pool(8).await;
    let mut handles = Vec::new();
    for i in 0..16_i64 {
        let g = graph.clone();
        handles.push(tokio::spawn(async move {
            let mut rows = g
                .execute(query("RETURN $i AS i").param("i", i))
                .await
                .unwrap();
            let row = rows.next().await.unwrap().unwrap();
            let v: i64 = row.get("i").unwrap();
            assert_eq!(v, i);
        }));
    }
    for h in handles {
        h.await.expect("task ok");
    }
}

// ---------------------------------------------------------------------------
// Primitive / collection result types
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn returns_primitive_scalar_types() {
    let graph = connect().await;
    let mut rows = graph
        .execute(query(
            "RETURN 42 AS i, 3.5 AS f, true AS b, 'hello' AS s, null AS n",
        ))
        .await
        .unwrap();
    let row = rows.next().await.unwrap().expect("row");
    assert_eq!(row.get::<i64>("i").unwrap(), 42);
    assert!((row.get::<f64>("f").unwrap() - 3.5).abs() < 1e-9);
    assert!(row.get::<bool>("b").unwrap());
    assert_eq!(row.get::<String>("s").unwrap(), "hello");
    // null: getting a typed value should fail; the BoltType view should be Null.
    let raw: BoltType = row.get("n").unwrap();
    assert!(
        matches!(raw, BoltType::Null(_)),
        "expected Null, got {raw:?}"
    );
}

#[tokio::test]
#[ignore]
async fn returns_list_and_map() {
    let graph = connect().await;
    let mut rows = graph
        .execute(query("RETURN [1, 2, 3] AS xs, {a: 1, b: 'two'} AS m"))
        .await
        .unwrap();
    let row = rows.next().await.unwrap().expect("row");
    let xs: Vec<i64> = row.get("xs").unwrap();
    assert_eq!(xs, vec![1, 2, 3]);

    // Map decoded as BoltType::Map for inspection.
    let raw: BoltType = row.get("m").unwrap();
    match raw {
        BoltType::Map(m) => {
            assert!(m.value.iter().any(|(k, _)| k.value == "a"));
            assert!(m.value.iter().any(|(k, _)| k.value == "b"));
        }
        other => panic!("expected Map, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Nodes / relationships
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn create_and_return_node() {
    let graph = connect().await;
    let label = unique_label("Person");
    let create = format!("CREATE (p:{label} {{name: 'Alice', age: 30}}) RETURN p");
    let mut rows = graph.execute(query(&create)).await.unwrap();
    let row = rows.next().await.unwrap().expect("node row");
    let node: neo4rs::Node = row.get("p").unwrap();
    assert!(node.labels().contains(&label.as_str()));
    assert_eq!(node.get::<String>("name").unwrap(), "Alice");
    assert_eq!(node.get::<i64>("age").unwrap(), 30);
}

#[tokio::test]
#[ignore]
async fn create_and_return_relationship() {
    let graph = connect().await;
    let l = unique_label("Friend");
    let q = format!(
        "CREATE (a:{l} {{n: 'a'}})-[r:KNOWS {{since: 2020}}]->(b:{l} {{n: 'b'}}) RETURN a, r, b"
    );
    let mut rows = graph.execute(query(&q)).await.unwrap();
    let row = rows.next().await.unwrap().expect("row");
    let r: neo4rs::Relation = row.get("r").unwrap();
    assert_eq!(r.typ(), "KNOWS");
    assert_eq!(r.get::<i64>("since").unwrap(), 2020);
}

// ---------------------------------------------------------------------------
// Parameterized queries
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn parameterized_query_with_mixed_types() {
    let graph = connect().await;
    let mut rows = graph
        .execute(
            query("RETURN $i AS i, $f AS f, $b AS b, $s AS s, $xs AS xs")
                .param("i", 7_i64)
                .param("f", 1.25_f64)
                .param("b", true)
                .param("s", "param")
                .param("xs", vec![10_i64, 20, 30]),
        )
        .await
        .unwrap();
    let row = rows.next().await.unwrap().expect("row");
    assert_eq!(row.get::<i64>("i").unwrap(), 7);
    assert!((row.get::<f64>("f").unwrap() - 1.25).abs() < 1e-9);
    assert!(row.get::<bool>("b").unwrap());
    assert_eq!(row.get::<String>("s").unwrap(), "param");
    assert_eq!(row.get::<Vec<i64>>("xs").unwrap(), vec![10, 20, 30]);
}

// ---------------------------------------------------------------------------
// Explicit transactions: BEGIN / COMMIT / ROLLBACK
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn explicit_transaction_commit_persists_data() {
    let graph = connect().await;
    let label = unique_label("TxCommit");

    let mut txn = graph.start_txn().await.expect("BEGIN");
    txn.run(query(&format!("CREATE (:{label} {{k: 1}})")))
        .await
        .expect("RUN inside txn");
    txn.commit().await.expect("COMMIT");

    let mut rows = graph
        .execute(query(&format!("MATCH (n:{label}) RETURN count(n) AS c")))
        .await
        .unwrap();
    let row = rows.next().await.unwrap().expect("row");
    assert_eq!(row.get::<i64>("c").unwrap(), 1);
}

#[tokio::test]
#[ignore]
async fn explicit_transaction_rollback_discards_data() {
    let graph = connect().await;
    let label = unique_label("TxRollback");

    let mut txn = graph.start_txn().await.expect("BEGIN");
    txn.run(query(&format!("CREATE (:{label} {{k: 1}})")))
        .await
        .expect("RUN inside txn");
    txn.rollback().await.expect("ROLLBACK");

    let mut rows = graph
        .execute(query(&format!("MATCH (n:{label}) RETURN count(n) AS c")))
        .await
        .unwrap();
    let row = rows.next().await.unwrap().expect("row");
    assert_eq!(row.get::<i64>("c").unwrap(), 0);
}

// ---------------------------------------------------------------------------
// Error responses
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn syntax_error_is_reported_via_failure() {
    let graph = connect().await;
    let res = graph
        .execute(query("THIS IS NOT VALID CYPHER AT ALL"))
        .await;
    // Either RUN fails or PULL surfaces an error stream.
    if let Ok(mut rows) = res {
        // Drain the result stream; either RUN already failed above or PULL
        // surfaces an error here. Both outcomes are valid for the protocol.
        let mut saw_error = false;
        loop {
            match rows.next().await {
                Ok(Some(_)) => continue,
                Ok(None) => break,
                Err(_) => {
                    saw_error = true;
                    break;
                }
            }
        }
        assert!(saw_error, "expected FAILURE for invalid Cypher");
    }
}

// ---------------------------------------------------------------------------
// Large result set (chunk framing / PULL n=-1 stress)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn large_result_set_streams_correctly() {
    const N: i64 = 1_500;
    let graph = connect().await;
    let mut rows = graph
        .execute(query("UNWIND range(1, $n) AS x RETURN x").param("n", N))
        .await
        .expect("RUN");
    let mut count: i64 = 0;
    let mut sum: i64 = 0;
    while let Some(row) = rows.next().await.expect("stream") {
        let x: i64 = row.get("x").unwrap();
        sum += x;
        count += 1;
    }
    assert_eq!(count, N);
    assert_eq!(sum, N * (N + 1) / 2);
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn unique_label(prefix: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}_{nanos:x}_{n}")
}
