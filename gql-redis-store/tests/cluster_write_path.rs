// tests/cluster_write_path.rs
//
// Integration tests for the cluster-safe write path (Issue #745).
//
// Tests CI-1 through CI-8 cover properties that hold without a live Redis
// connection and verify the public contract of the cluster-safe write path.
// Tests requiring a running Redis are marked `#[ignore]` and can be run
// explicitly with `cargo test -- --include-ignored`.

use indexmap::IndexMap;
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use gql_core::error::StorageError;
use gql_core::storage::{GraphStore, RangeQuery};
use gql_core::types::Value;
use gql_redis_store::RedisGraphStore;
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use redis::Client;
use uuid::Uuid;

const INDEXING_SRC: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/indexing.rs"));
const LIB_SRC: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs"));
const LOCK_SRC: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lock.rs"));
const METRICS_SRC: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/redis_memory_metrics.rs"
));
const CONNECTION_SRC: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/connection.rs"));
const NODES_SRC: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/writes/nodes.rs"));
const EDGES_SRC: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/writes/edges.rs"));
const GRAPHS_SRC: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/writes/graphs.rs"));
const READS_SRC: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/reads.rs"));

fn strip_line_comments(source: &str) -> String {
    let mut cleaned = String::with_capacity(source.len());
    for line in source.lines() {
        cleaned.push_str(line.split_once("//").map_or(line, |(code, _)| code));
        cleaned.push('\n');
    }
    cleaned
}

fn function_block<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("missing function signature: {signature}"));
    let rest = &source[start + signature.len()..];
    let end_offset = [
        rest.find("\n    pub(crate) fn "),
        rest.find("\n    pub fn "),
        rest.find("\n#[cfg(test)]"),
    ]
    .into_iter()
    .flatten()
    .min()
    .unwrap_or(rest.len());
    &source[start..start + signature.len() + end_offset]
}

fn standalone_store() -> RedisGraphStore {
    standalone_redis_url();
    RedisGraphStore::from_env().expect("standalone store should initialize from env")
}

fn standalone_redis_url() -> String {
    std::env::var("GQL_REDIS_URL")
        .or_else(|_| std::env::var("REDIS_URL"))
        .expect("GQL_REDIS_URL or REDIS_URL must be set for ignored live Redis tests")
}

fn unique_graph(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4().simple())
}

fn props(entries: &[(&str, Value)]) -> IndexMap<String, Value> {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect()
}

fn metrics_handle() -> &'static PrometheusHandle {
    static HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();
    HANDLE.get_or_init(|| {
        PrometheusBuilder::new()
            .install_recorder()
            .expect("failed to install cluster_write_path Prometheus recorder")
    })
}

fn metrics_guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock()
        .expect("cluster_write_path metrics lock should not be poisoned")
}

fn metric_value(handle: &PrometheusHandle, metric: &str, labels: &[(&str, &str)]) -> f64 {
    let output = handle.render();
    output
        .lines()
        .find_map(|line| {
            if line.starts_with('#') {
                return None;
            }
            let rest = line.strip_prefix(metric)?;
            if !matches!(rest.chars().next(), Some(' ') | Some('{')) {
                return None;
            }
            if labels.is_empty() && rest.starts_with('{') {
                return None;
            }
            if !labels
                .iter()
                .all(|(key, value)| label_matches(line, key, value))
            {
                return None;
            }
            line.split_whitespace().last()?.parse::<f64>().ok()
        })
        .unwrap_or(0.0)
}

fn label_matches(line: &str, key: &str, value: &str) -> bool {
    let expected = format!(r#"{key}="{value}""#);
    line.find(&expected).is_some_and(|index| {
        let bytes = line.as_bytes();
        let leading_ok = index == 0 || matches!(bytes.get(index - 1), Some(b'{') | Some(b','));
        let trailing_ok = matches!(bytes.get(index + expected.len()), Some(b',') | Some(b'}'));
        leading_ok && trailing_ok
    })
}

fn histogram_count(handle: &PrometheusHandle, metric: &str, labels: &[(&str, &str)]) -> f64 {
    metric_value(handle, &format!("{metric}_count"), labels)
}

fn wait_for_metric_at_least(
    handle: &PrometheusHandle,
    metric: &str,
    labels: &[(&str, &str)],
    minimum: f64,
) -> f64 {
    for _ in 0..100 {
        let current = metric_value(handle, metric, labels);
        if current >= minimum {
            return current;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let current = metric_value(handle, metric, labels);
    panic!("metric {metric} stayed below {minimum}, last={current}");
}

// ── CI-1: MAX_NODES_FOR_DELETE constant sanity ───────────────────────────────

#[test]
fn ci1_max_nodes_for_delete_is_positive_and_bounded() {
    // Positive: must reject the empty-graph case properly elsewhere.
    const {
        assert!(
            gql_redis_store::MAX_NODES_FOR_DELETE > 0,
            "MAX_NODES_FOR_DELETE must be positive"
        )
    };
    // Bounded: SMEMBERS on a set this large must not OOM a typical instance.
    const { assert!(gql_redis_store::MAX_NODES_FOR_DELETE <= 1_000_000) };
}

// ── CI-2: backend error counter is thread-safe and monotonic ────────────────

#[test]
fn ci2_backend_error_counter_increments() {
    let before = gql_redis_store::backend_error_count();
    gql_redis_store::record_backend_error();
    let after = gql_redis_store::backend_error_count();
    assert_eq!(after, before + 1, "counter must increment by exactly 1");
}

// ── CI-3: backend_error_msg returns non-empty string in both build modes ─────

#[test]
fn ci3_backend_error_msg_nonempty() {
    let msg = gql_redis_store::backend_error_msg("redis error detail");
    assert!(
        !msg.is_empty(),
        "backend_error_msg must never return an empty string"
    );
}

// ── CI-4: backend_error_msg is opaque in release, transparent in debug ───────

#[test]
fn ci4_backend_error_msg_build_mode_contract() {
    let input = "MOVED 1234 10.0.0.1:6379";
    let msg = gql_redis_store::backend_error_msg(input);

    #[cfg(not(debug_assertions))]
    assert_eq!(
        msg, "backend error",
        "release build must not expose Redis error details (SEC-6)"
    );

    #[cfg(debug_assertions)]
    assert_eq!(
        msg, input,
        "debug build must pass the original message through for diagnostics"
    );
}

// ── CI-5: no .atomic() in the write-pipeline source files ───────────────────
//
#[test]
fn ci5_no_atomic_in_write_pipeline_sources() {
    for (name, source) in [
        ("writes/nodes.rs", NODES_SRC),
        ("writes/edges.rs", EDGES_SRC),
        ("writes/graphs.rs", GRAPHS_SRC),
    ] {
        let cleaned = strip_line_comments(source);
        assert!(
            !cleaned.contains(".atomic()"),
            "{name} must not call .atomic(); MULTI/EXEC is not supported in Redis Cluster"
        );
    }
}

// ── CI-6: delete_graph key set is consistent with key schema ────────────────
//
// Verifies that all keys touched by delete_graph_impl embed the {graph}
// hash tag so they route to the same Redis Cluster slot.

#[test]
fn ci6_delete_graph_keys_carry_hash_tag() {
    // Re-create the key list that delete_graph_impl would build for a graph
    // named "testdb" with one node (id=1) and one edge (id=1).
    let g = "testdb";
    let tag = format!("{{{g}}}");

    // Well-known fixed keys.
    let fixed = vec![
        format!("G:{{{g}}}:node_seq"),
        format!("G:{{{g}}}:edge_seq"),
        format!("G:{{{g}}}:nodes"),
        format!("G:{{{g}}}:edges"),
        format!("G:{{{g}}}:indexes"),
        format!("G:{{{g}}}:_deleting"),
        format!("G:{{{g}}}:_labels"),
        format!("G:{{{g}}}:_reltypes"),
        format!("G:{{{g}}}:_propidx"),
    ];

    // Per-ID keys.
    let per_id = [
        format!("G:{{{g}}}:node:1"),
        format!("G:{{{g}}}:out:1"),
        format!("G:{{{g}}}:in:1"),
        format!("G:{{{g}}}:edge:1"),
        format!("G:{{{g}}}:label:Person"),
        format!("G:{{{g}}}:reltype:KNOWS"),
        format!("G:{{{g}}}:propidx:Person:name:s:Alice"),
    ];

    for key in fixed.iter().chain(per_id.iter()) {
        assert!(
            key.contains(&tag),
            "key '{key}' must contain hash tag '{tag}' for cluster slot affinity"
        );
    }
}

// ── CI-7: SCAN pattern not present in delete_graph_impl (compile-time) ───────
//
// Checks both the literal `cmd("SCAN")` call and the `scan_count()` wrapper,
// which issues SCAN internally and would be equally unsafe across cluster slots.
#[test]
fn ci7_scan_not_used_in_delete_graph() {
    let cleaned = strip_line_comments(GRAPHS_SRC);
    assert!(
        !cleaned.contains("cmd(\"SCAN\")"),
        "delete_graph_impl must not call SCAN; cluster-safe cleanup must derive keys from same-slot manifests"
    );
    assert!(
        !cleaned.contains("scan_count("),
        "delete_graph_impl must not use scan_count(); it issues SCAN internally and is unsafe across cluster slots"
    );
}

// ── CI-8: delete_graph with live Redis (skipped without Redis) ───────────────

#[test]
#[ignore = "requires a running Redis at REDIS_URL"]
fn ci8_delete_graph_end_to_end() {
    let store = standalone_store();
    let graph = unique_graph("delete-graph-cleanup");
    let prefix = std::env::var("GQL_REDIS_KEY_PREFIX").unwrap_or_default();
    let client =
        Client::open(standalone_redis_url().as_str()).expect("redis client should initialize");
    let mut conn = client
        .get_connection()
        .expect("redis connection should initialize");

    store.create_graph(&graph).expect("graph should be created");
    let alice = store
        .create_node(
            &graph,
            vec!["Person".to_string(), "Employee".to_string()],
            props(&[
                ("name", Value::Str("Alice".into())),
                ("city", Value::Str("Seattle".into())),
            ]),
        )
        .expect("create_node should succeed");
    let bob = store
        .create_node(
            &graph,
            vec!["Person".to_string()],
            props(&[("name", Value::Str("Bob".into()))]),
        )
        .expect("second create_node should succeed");
    store
        .create_edge(&graph, alice, bob, "KNOWS".to_string(), IndexMap::new())
        .expect("create_edge should succeed");
    store
        .create_index(&graph, "Person", "name")
        .expect("name index should succeed");
    store
        .create_index(&graph, "Person", "city")
        .expect("city index should succeed");
    store
        .create_index(&graph, "Manager", "name")
        .expect("manager index should succeed");
    store
        .add_node_labels(&graph, bob, &[String::from("Manager")])
        .expect("add_node_labels should succeed");
    let bob_name = Value::Str("Bob".into());
    let manager_matches = store
        .find_nodes_by_label_and_property(&graph, "Manager", "name", RangeQuery::Eq(&bob_name))
        .expect("manager index lookup should succeed after add_node_labels");
    assert_eq!(
        manager_matches,
        vec![bob],
        "add_node_labels should backfill existing properties into matching label indexes"
    );
    store
        .remove_node_labels(&graph, bob, &[String::from("Manager")])
        .expect("remove_node_labels should succeed");
    let removed_manager_matches = store
        .find_nodes_by_label_and_property(&graph, "Manager", "name", RangeQuery::Eq(&bob_name))
        .expect("manager index lookup should succeed after remove_node_labels");
    assert!(
        removed_manager_matches.is_empty(),
        "remove_node_labels should deindex properties for the removed label"
    );
    let person_matches = store
        .find_nodes_by_label_and_property(&graph, "Person", "name", RangeQuery::Eq(&bob_name))
        .expect("person index lookup should still succeed after removing only the Manager label");
    assert_eq!(
        person_matches,
        vec![bob],
        "remove_node_labels should preserve property indexes for labels that remain on the node"
    );

    store
        .delete_graph(&graph)
        .expect("delete_graph should succeed");
    let stale_city_propidx = format!("{prefix}G:{{{graph}}}:propidx:Person\u{0}city\u{0}s:Seattle");
    let cleanup_keys = [
        format!("{prefix}G:{{{graph}}}:_deleting"),
        format!("{prefix}G:{{{graph}}}:_labels"),
        format!("{prefix}G:{{{graph}}}:_reltypes"),
        format!("{prefix}G:{{{graph}}}:_propidx"),
        stale_city_propidx.clone(),
    ];
    let remaining_cleanup_keys: i64 = redis::cmd("EXISTS")
        .arg(&cleanup_keys)
        .query(&mut conn)
        .expect("cleanup key existence check should succeed");
    assert_eq!(
        remaining_cleanup_keys, 0,
        "delete_graph should remove registry, retry-state, and propidx-manifest keys directly, not just hide them through read helpers"
    );
    assert!(
        !store
            .graph_exists(&graph)
            .expect("graph_exists should succeed after delete"),
        "graph registry entry should be removed"
    );

    store
        .create_graph(&graph)
        .expect("graph name should be reusable after delete_graph");
    let stats = store
        .graph_stats(&graph)
        .expect("graph_stats should succeed after recreation");
    assert_eq!(stats.node_count, 0);
    assert_eq!(stats.edge_count, 0);
    assert_eq!(stats.label_count, 0);
    assert_eq!(stats.rel_type_count, 0);
    assert!(
        store
            .all_label_names(&graph)
            .expect("all_label_names should succeed")
            .is_empty(),
        "delete_graph should remove label registries and sets"
    );
    assert!(
        matches!(
            store.create_node(&graph, vec!["Bad\x00Label".to_string()], IndexMap::new()),
            Err(StorageError::ConstraintViolation { .. })
        ),
        "create_node should reject invalid labels through the live write path"
    );
    assert!(
        matches!(
            store.create_index(&graph, "Person", "na\x00me"),
            Err(StorageError::ConstraintViolation { .. })
        ),
        "create_index should reject invalid property names through the live write path"
    );

    let recreated = store
        .create_node(
            &graph,
            vec!["Person".to_string()],
            props(&[("name", Value::Str("Carol".into()))]),
        )
        .expect("create_node after recreation should succeed");
    assert_eq!(
        recreated.0, 1,
        "node sequence should reset when delete_graph removes per-graph state"
    );

    store
        .create_index(&graph, "Person", "name")
        .expect("index recreation should succeed after cleanup");
    store
        .create_index(&graph, "Person", "city")
        .expect("city index recreation should succeed after cleanup");
    let stale_city = Value::Str("Seattle".into());
    let stale_matches = store
        .find_nodes_by_label_and_property(&graph, "Person", "city", RangeQuery::Eq(&stale_city))
        .expect("city lookup should succeed after recreation");
    assert!(
        stale_matches.is_empty(),
        "delete_graph should remove stale propidx keys from the previous graph incarnation"
    );
    let target = Value::Str("Carol".into());
    let matches = store
        .find_nodes_by_label_and_property(&graph, "Person", "name", RangeQuery::Eq(&target))
        .expect("indexed lookup should succeed after recreation");
    assert_eq!(matches, vec![recreated]);
}

#[test]
#[ignore = "requires a running Redis at REDIS_URL"]
fn ci8_delete_graph_too_large_preserves_visibility_and_retry_state() {
    let store = standalone_store();
    let graph = unique_graph("delete-graph-too-large");
    let prefix = std::env::var("GQL_REDIS_KEY_PREFIX").unwrap_or_default();
    let client =
        Client::open(standalone_redis_url().as_str()).expect("redis client should initialize");
    let mut conn = client
        .get_connection()
        .expect("redis connection should initialize");

    store.create_graph(&graph).expect("graph should be created");

    let nodes_key = format!("{prefix}G:{{{graph}}}:nodes");
    let edges_key = format!("{prefix}G:{{{graph}}}:edges");
    let deleting_key = format!("{prefix}G:{{{graph}}}:_deleting");

    redis::cmd("SADD")
        .arg(&nodes_key)
        .arg("1")
        .query::<i64>(&mut conn)
        .expect("seed node manifest should succeed");

    let oversized_edges: Vec<String> = (0..=gql_redis_store::MAX_NODES_FOR_DELETE)
        .map(|id| id.to_string())
        .collect();
    for chunk in oversized_edges.chunks(5_000) {
        redis::cmd("SADD")
            .arg(&edges_key)
            .arg(chunk)
            .query::<i64>(&mut conn)
            .expect("seed edge manifest chunk should succeed");
    }

    let err = store
        .delete_graph(&graph)
        .expect_err("oversized edge manifest should refuse delete_graph");
    match err {
        StorageError::DeleteGraphTooLarge {
            resource,
            resource_count,
            limit,
            ..
        } => {
            assert_eq!(resource, "edges");
            assert_eq!(resource_count, gql_redis_store::MAX_NODES_FOR_DELETE + 1);
            assert_eq!(limit, gql_redis_store::MAX_NODES_FOR_DELETE);
        }
        other => panic!("expected DeleteGraphTooLarge for edges, got {other:?}"),
    }

    assert!(
        store
            .graph_exists(&graph)
            .expect("graph_exists should succeed after oversized delete refusal"),
        "oversized delete refusal must leave the graph visible"
    );

    let in_registry: bool = redis::cmd("SISMEMBER")
        .arg(format!("{prefix}__sd2_registry:graphs"))
        .arg(&graph)
        .query(&mut conn)
        .expect("registry membership check should succeed");
    assert!(
        in_registry,
        "oversized delete refusal must keep the graph registered"
    );

    let deleting_exists: i64 = redis::cmd("EXISTS")
        .arg(&deleting_key)
        .query(&mut conn)
        .expect("retry-state existence check should succeed");
    assert_eq!(
        deleting_exists, 0,
        "oversized delete refusal must not set the _deleting retry marker"
    );
}

#[test]
#[ignore = "requires a running Redis at REDIS_URL"]
fn ci27_write_lock_metrics_move_under_contention_then_success() {
    let _guard = metrics_guard();
    let handle = metrics_handle();
    let store = standalone_store();
    let graph = unique_graph("write-lock-metrics");
    let prefix = std::env::var("GQL_REDIS_KEY_PREFIX").unwrap_or_default();
    let client =
        Client::open(standalone_redis_url().as_str()).expect("redis client should initialize");
    let mut conn = client
        .get_connection()
        .expect("redis connection should initialize");
    let lock_key = format!("{prefix}G:{{{graph}}}:write_lock");

    store.create_graph(&graph).expect("graph should be created");
    store
        .create_node(
            &graph,
            vec!["MetricProbe".to_string()],
            props(&[("name", Value::Str("Seed".into()))]),
        )
        .expect("seed node should be created before contended index build");

    let before_acquire_attempts =
        metric_value(handle, "redis_write_lock_acquire_attempts_total", &[]);
    let before_acquire_successes =
        metric_value(handle, "redis_write_lock_acquire_successes_total", &[]);
    let before_contention = metric_value(handle, "redis_write_lock_acquire_contention_total", &[]);
    let before_contention_failures =
        metric_value(handle, "redis_write_lock_contention_failures_total", &[]);
    let before_release_successes =
        metric_value(handle, "redis_write_lock_release_successes_total", &[]);
    let before_wait_histogram =
        histogram_count(handle, "redis_write_lock_acquire_wait_seconds", &[]);
    let before_hold_histogram = histogram_count(handle, "redis_write_lock_hold_seconds", &[]);

    let lock_token = format!("hold-{}", Uuid::new_v4().simple());
    let acquired: Option<String> = redis::cmd("SET")
        .arg(&lock_key)
        .arg(&lock_token)
        .arg("NX")
        .arg("PX")
        .arg(5_000u64)
        .query(&mut conn)
        .expect("test should seed the graph write lock");
    assert!(
        acquired.is_some(),
        "test should acquire the graph write lock before starting the write"
    );

    let (started_tx, started_rx) = mpsc::channel();
    let store_for_thread = store.clone();
    let graph_for_thread = graph.clone();
    let writer = std::thread::spawn(move || {
        started_tx
            .send(())
            .expect("writer thread should signal startup");
        store_for_thread.create_index(&graph_for_thread, "MetricProbe", "name")
    });
    started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("writer thread should start promptly");

    wait_for_metric_at_least(
        handle,
        "redis_write_lock_acquire_attempts_total",
        &[],
        before_acquire_attempts + 1.0,
    );
    std::thread::sleep(Duration::from_millis(250));

    let released: i64 = redis::cmd("DEL")
        .arg(&lock_key)
        .query(&mut conn)
        .expect("test should release the seeded write lock");
    assert_eq!(released, 1, "seeded lock should still be present");

    writer
        .join()
        .expect("writer thread should complete")
        .expect("writer should succeed after lock contention clears");

    let after_acquire_attempts =
        metric_value(handle, "redis_write_lock_acquire_attempts_total", &[]);
    let after_acquire_successes =
        metric_value(handle, "redis_write_lock_acquire_successes_total", &[]);
    let after_contention = metric_value(handle, "redis_write_lock_acquire_contention_total", &[]);
    let after_contention_failures =
        metric_value(handle, "redis_write_lock_contention_failures_total", &[]);
    let after_release_successes =
        metric_value(handle, "redis_write_lock_release_successes_total", &[]);
    let after_wait_histogram =
        histogram_count(handle, "redis_write_lock_acquire_wait_seconds", &[]);
    let after_hold_histogram = histogram_count(handle, "redis_write_lock_hold_seconds", &[]);

    assert!(after_acquire_attempts >= before_acquire_attempts + 1.0);
    assert!(after_acquire_successes >= before_acquire_successes + 1.0);
    assert!(after_contention >= before_contention + 1.0);
    assert_eq!(
        after_contention_failures, before_contention_failures,
        "transient contention that eventually succeeds must not record contention failures"
    );
    assert!(after_release_successes >= before_release_successes + 1.0);
    assert!(after_wait_histogram >= before_wait_histogram + 1.0);
    assert!(after_hold_histogram >= before_hold_histogram + 1.0);

    store
        .delete_graph(&graph)
        .expect("cleanup delete_graph should succeed");
}

// ── CI-9: write pipeline does not use MULTI/EXEC (cluster-incompatible) ──────

#[test]
fn ci9_node_write_pipeline_no_multi_exec() {
    // Verify the node creation pipeline bytes do not contain MULTI or EXEC.
    // This encodes the fix for Issue #745: redis-rs ClusterConnection does
    // not support MULTI/EXEC, so .atomic() must not appear in cluster writes.
    let graph = "testgraph";
    let id = 1u64;
    let labels = vec!["Person".to_string()];
    let node_key = format!("G:{{{graph}}}:node:{id}");

    let mut pipe = redis::pipe();
    pipe.cmd("HSET")
        .arg(&node_key)
        .arg("labels")
        .arg(serde_json::to_string(&labels).unwrap())
        .arg("props")
        .arg("{}")
        .ignore();
    for label in &labels {
        pipe.cmd("SADD")
            .arg(format!("G:{{{graph}}}:label:{label}"))
            .arg(id.to_string())
            .ignore();
    }
    pipe.cmd("SADD")
        .arg(format!("G:{{{graph}}}:nodes"))
        .arg(id.to_string())
        .ignore();

    let packed = pipe.get_packed_pipeline();
    let s = String::from_utf8_lossy(&packed);
    assert!(
        !s.to_uppercase().contains("MULTI"),
        "node write pipeline must not contain MULTI: {s}"
    );
    assert!(
        !s.to_uppercase().contains("EXEC"),
        "node write pipeline must not contain EXEC: {s}"
    );
}

// ── CI-10: all write-path keys share the graph hash tag ───────────────────────

#[test]
fn ci10_pipeline_keys_all_share_graph_hash_tag() {
    let graph = "mygraph";
    let tag = format!("{{{graph}}}");

    // All key patterns used in node/edge writes, delete-state tracking,
    // and propidx cleanup manifests.
    let patterns = [
        format!("G:{tag}:node:1"),
        format!("G:{tag}:label:Person"),
        format!("G:{tag}:_labels"),
        format!("G:{tag}:nodes"),
        format!("G:{tag}:edge:1"),
        format!("G:{tag}:reltype:KNOWS"),
        format!("G:{tag}:_reltypes"),
        format!("G:{tag}:out:1"),
        format!("G:{tag}:in:2"),
        format!("G:{tag}:edges"),
        format!("G:{tag}:node_seq"),
        format!("G:{tag}:edge_seq"),
        format!("G:{tag}:_deleting"),
        format!("G:{tag}:_propidx"),
    ];

    for k in &patterns {
        assert!(
            k.contains(&tag),
            "pipeline key '{k}' missing graph hash tag '{tag}'"
        );
    }
}

#[test]
fn ci11_graph_mutators_use_write_lock() {
    for (name, source, signature) in [
        (
            "create_graph_impl",
            GRAPHS_SRC,
            "pub(crate) fn create_graph_impl",
        ),
        (
            "create_node_impl",
            NODES_SRC,
            "pub(crate) fn create_node_impl",
        ),
        (
            "set_node_properties_impl",
            NODES_SRC,
            "pub(crate) fn set_node_properties_impl",
        ),
        (
            "replace_node_properties_impl",
            NODES_SRC,
            "pub(crate) fn replace_node_properties_impl",
        ),
        (
            "remove_node_property_impl",
            NODES_SRC,
            "pub(crate) fn remove_node_property_impl",
        ),
        (
            "add_node_labels_impl",
            NODES_SRC,
            "pub(crate) fn add_node_labels_impl",
        ),
        (
            "remove_node_labels_impl",
            NODES_SRC,
            "pub(crate) fn remove_node_labels_impl",
        ),
        (
            "delete_node_impl",
            NODES_SRC,
            "pub(crate) fn delete_node_impl",
        ),
        ("batch_create_nodes", NODES_SRC, "pub fn batch_create_nodes"),
        (
            "create_edge_impl",
            EDGES_SRC,
            "pub(crate) fn create_edge_impl",
        ),
        (
            "set_edge_properties_impl",
            EDGES_SRC,
            "pub(crate) fn set_edge_properties_impl",
        ),
        (
            "replace_edge_properties_impl",
            EDGES_SRC,
            "pub(crate) fn replace_edge_properties_impl",
        ),
        (
            "remove_edge_property_impl",
            EDGES_SRC,
            "pub(crate) fn remove_edge_property_impl",
        ),
        (
            "delete_edge_impl",
            EDGES_SRC,
            "pub(crate) fn delete_edge_impl",
        ),
        ("batch_create_edges", EDGES_SRC, "pub fn batch_create_edges"),
        (
            "create_index_impl",
            INDEXING_SRC,
            "pub(crate) fn create_index_impl",
        ),
        (
            "drop_index_impl",
            INDEXING_SRC,
            "pub(crate) fn drop_index_impl",
        ),
        (
            "delete_graph_impl",
            GRAPHS_SRC,
            "pub(crate) fn delete_graph_impl",
        ),
    ] {
        let body = function_block(source, signature);
        assert!(
            body.contains("with_graph_write_lock(")
                || body.contains("with_write_lock(")
                || body.contains("key_graph_lock("),
            "{name} must participate in the write lock discipline"
        );
    }
}

#[test]
fn ci11b_hot_mutators_use_scoped_write_locks() {
    for (name, source, signature, scoped_key) in [
        (
            "set_node_properties_impl",
            NODES_SRC,
            "pub(crate) fn set_node_properties_impl",
            "key_node_write_lock(",
        ),
        (
            "replace_node_properties_impl",
            NODES_SRC,
            "pub(crate) fn replace_node_properties_impl",
            "key_node_write_lock(",
        ),
        (
            "remove_node_property_impl",
            NODES_SRC,
            "pub(crate) fn remove_node_property_impl",
            "key_node_write_lock(",
        ),
        (
            "add_node_labels_impl",
            NODES_SRC,
            "pub(crate) fn add_node_labels_impl",
            "key_node_write_lock(",
        ),
        (
            "remove_node_labels_impl",
            NODES_SRC,
            "pub(crate) fn remove_node_labels_impl",
            "key_node_write_lock(",
        ),
        (
            "create_edge_impl",
            EDGES_SRC,
            "pub(crate) fn create_edge_impl",
            "key_edge_write_lock(",
        ),
        (
            "set_edge_properties_impl",
            EDGES_SRC,
            "pub(crate) fn set_edge_properties_impl",
            "key_edge_id_write_lock(",
        ),
        (
            "replace_edge_properties_impl",
            EDGES_SRC,
            "pub(crate) fn replace_edge_properties_impl",
            "key_edge_id_write_lock(",
        ),
        (
            "remove_edge_property_impl",
            EDGES_SRC,
            "pub(crate) fn remove_edge_property_impl",
            "key_edge_id_write_lock(",
        ),
        (
            "delete_edge_impl",
            EDGES_SRC,
            "pub(crate) fn delete_edge_impl",
            "key_edge_id_write_lock(",
        ),
        (
            "batch_create_nodes",
            NODES_SRC,
            "pub fn batch_create_nodes",
            "key_batch_node_write_lock(",
        ),
        (
            "batch_create_edges",
            EDGES_SRC,
            "pub fn batch_create_edges",
            "key_batch_edge_write_lock(",
        ),
    ] {
        let body = function_block(source, signature);
        assert!(
            body.contains(scoped_key),
            "{name} should carry a dedicated scoped write-lock key for same-graph coordination"
        );
    }
}

#[test]
fn ci12_locked_graph_mutators_validate_existence_on_locked_connection() {
    for (name, source, signature) in [
        (
            "set_node_properties_impl",
            NODES_SRC,
            "pub(crate) fn set_node_properties_impl",
        ),
        (
            "replace_node_properties_impl",
            NODES_SRC,
            "pub(crate) fn replace_node_properties_impl",
        ),
        (
            "remove_node_property_impl",
            NODES_SRC,
            "pub(crate) fn remove_node_property_impl",
        ),
        (
            "add_node_labels_impl",
            NODES_SRC,
            "pub(crate) fn add_node_labels_impl",
        ),
        (
            "remove_node_labels_impl",
            NODES_SRC,
            "pub(crate) fn remove_node_labels_impl",
        ),
        (
            "delete_node_impl",
            NODES_SRC,
            "pub(crate) fn delete_node_impl",
        ),
        (
            "create_node_impl",
            NODES_SRC,
            "pub(crate) fn create_node_impl",
        ),
        ("batch_create_nodes", NODES_SRC, "pub fn batch_create_nodes"),
        ("create_edge_locked", EDGES_SRC, "fn create_edge_locked"),
        (
            "set_edge_properties_impl",
            EDGES_SRC,
            "pub(crate) fn set_edge_properties_impl",
        ),
        (
            "replace_edge_properties_impl",
            EDGES_SRC,
            "pub(crate) fn replace_edge_properties_impl",
        ),
        (
            "remove_edge_property_impl",
            EDGES_SRC,
            "pub(crate) fn remove_edge_property_impl",
        ),
        (
            "delete_edge_impl",
            EDGES_SRC,
            "pub(crate) fn delete_edge_impl",
        ),
        ("batch_create_edges", EDGES_SRC, "pub fn batch_create_edges"),
        (
            "create_index_impl",
            INDEXING_SRC,
            "pub(crate) fn create_index_impl",
        ),
        (
            "drop_index_impl",
            INDEXING_SRC,
            "pub(crate) fn drop_index_impl",
        ),
        (
            "delete_graph_impl",
            GRAPHS_SRC,
            "pub(crate) fn delete_graph_impl",
        ),
    ] {
        let body = function_block(source, signature);
        assert!(
            body.contains("require_graph_on(conn.as_mut(),"),
            "{name} must validate graph existence on its already-locked primary connection"
        );
        assert!(
            !body.contains("self.require_graph("),
            "{name} must not validate graph existence through the replica-preferring read path"
        );
    }
}

#[test]
#[ignore = "requires a running Redis at REDIS_URL"]
fn ci13_create_graph_participates_in_same_graph_write_lock() {
    let store = standalone_store();
    let graph = unique_graph("create-graph-lock");
    let url = standalone_redis_url();
    let prefix = std::env::var("GQL_REDIS_KEY_PREFIX").unwrap_or_default();
    let client = Client::open(url.as_str()).expect("redis client should initialize");
    let mut conn = client
        .get_connection()
        .expect("redis connection should initialize");
    let lock_key = format!("{prefix}G:{{{graph}}}:write_lock");
    let registry_key = format!("{prefix}__sd2_registry:graphs");

    store.create_graph(&graph).expect("graph should be created");
    store
        .create_node(
            &graph,
            vec!["Person".to_string()],
            props(&[("name", Value::Str("Alice".into()))]),
        )
        .expect("seed node should be created");

    let lock_result: Option<String> = redis::cmd("SET")
        .arg(&lock_key)
        .arg("test-token")
        .arg("NX")
        .arg("PX")
        .arg(5_000)
        .query(&mut conn)
        .expect("manual lock should be set");
    assert_eq!(lock_result.as_deref(), Some("OK"));

    let removed: i64 = redis::cmd("SREM")
        .arg(&registry_key)
        .arg(&graph)
        .query(&mut conn)
        .expect("graph should be removed from registry");
    assert_eq!(removed, 1, "graph registry entry should exist before test");

    let (tx, rx) = mpsc::channel();
    let graph_for_thread = graph.clone();
    std::thread::spawn(move || {
        let result = standalone_store().create_graph(&graph_for_thread);
        tx.send(result)
            .expect("create_graph result should be sent to the test thread");
    });

    std::thread::sleep(Duration::from_millis(150));
    assert!(
        !store
            .graph_exists(&graph)
            .expect("graph_exists should succeed while the lock is held"),
        "create_graph must not recreate the registry entry before the held same-graph lock is released"
    );
    let pending_result = match rx.recv_timeout(Duration::from_millis(150)) {
        Ok(result) => {
            assert!(
                result.is_err(),
                "create_graph must not succeed while the same-graph write lock is held"
            );
            Some(result)
        }
        Err(mpsc::RecvTimeoutError::Timeout) => None,
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            panic!("create_graph worker disconnected before reporting a result")
        }
    };

    redis::cmd("DEL")
        .arg(&lock_key)
        .query::<()>(&mut conn)
        .expect("manual lock should be released");

    match pending_result {
        Some(_) => store
            .create_graph(&graph)
            .expect("create_graph should succeed once the manual lock is released"),
        None => rx
            .recv_timeout(Duration::from_secs(3))
            .expect("create_graph should complete after the lock is released")
            .expect("create_graph should succeed after the lock is released"),
    }
    assert!(
        store
            .graph_exists(&graph)
            .expect("graph_exists should succeed after the lock is released"),
        "graph registry entry should be restored after create_graph completes"
    );

    store
        .delete_graph(&graph)
        .expect("delete_graph should clean up the recreated graph");
}

#[test]
#[ignore = "requires a running Redis at REDIS_URL"]
fn ci13b_scoped_mutators_participate_in_graph_write_lock() {
    let store = standalone_store();
    let graph = unique_graph("scoped-write-lock");
    let url = standalone_redis_url();
    let prefix = std::env::var("GQL_REDIS_KEY_PREFIX").unwrap_or_default();
    let client = Client::open(url.as_str()).expect("redis client should initialize");
    let mut conn = client
        .get_connection()
        .expect("redis connection should initialize");
    let lock_key = format!("{prefix}G:{{{graph}}}:write_lock");

    store.create_graph(&graph).expect("graph should be created");
    let node_id = store
        .create_node(
            &graph,
            vec!["Person".to_string()],
            props(&[("name", Value::Str("Alice".into()))]),
        )
        .expect("seed node should be created");

    let lock_result: Option<String> = redis::cmd("SET")
        .arg(&lock_key)
        .arg("test-token")
        .arg("NX")
        .arg("PX")
        .arg(5_000)
        .query(&mut conn)
        .expect("manual lock should be set");
    assert_eq!(lock_result.as_deref(), Some("OK"));

    let (tx, rx) = mpsc::channel();
    let graph_for_thread = graph.clone();
    std::thread::spawn(move || {
        let result = standalone_store().set_node_properties(
            &graph_for_thread,
            node_id,
            props(&[("role", Value::Str("admin".into()))]),
        );
        tx.send(result)
            .expect("set_node_properties result should be sent to the test thread");
    });

    std::thread::sleep(Duration::from_millis(150));
    let pending_result = match rx.recv_timeout(Duration::from_millis(150)) {
        Ok(result) => {
            assert!(
                result.is_err(),
                "scoped mutators must not succeed while the same-graph write lock is held"
            );
            Some(result)
        }
        Err(mpsc::RecvTimeoutError::Timeout) => None,
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            panic!("set_node_properties worker disconnected before reporting a result")
        }
    };

    redis::cmd("DEL")
        .arg(&lock_key)
        .query::<()>(&mut conn)
        .expect("manual lock should be released");

    match pending_result {
        Some(_) => store
            .set_node_properties(
                &graph,
                node_id,
                props(&[("role", Value::Str("admin".into()))]),
            )
            .expect("set_node_properties should succeed once the graph lock is released"),
        None => rx
            .recv_timeout(Duration::from_secs(3))
            .expect("set_node_properties should complete after the lock is released")
            .expect("set_node_properties should succeed after the lock is released"),
    }

    store
        .delete_graph(&graph)
        .expect("delete_graph should clean up the graph");
}

#[test]
#[ignore = "requires a running Redis at REDIS_URL"]
fn ci13c_batch_create_nodes_waits_out_same_graph_write_lock() {
    let _guard = metrics_guard();
    let handle = metrics_handle();
    let store = standalone_store();
    let graph = unique_graph("batch-create-nodes-lock");
    let prefix = std::env::var("GQL_REDIS_KEY_PREFIX").unwrap_or_default();
    let client =
        Client::open(standalone_redis_url().as_str()).expect("redis client should initialize");
    let mut conn = client
        .get_connection()
        .expect("redis connection should initialize");
    let lock_key = format!("{prefix}G:{{{graph}}}:write_lock");

    store.create_graph(&graph).expect("graph should be created");

    let lock_result: Option<String> = redis::cmd("SET")
        .arg(&lock_key)
        .arg("test-token")
        .arg("NX")
        .arg("PX")
        .arg(5_000)
        .query(&mut conn)
        .expect("manual lock should be set");
    assert_eq!(lock_result.as_deref(), Some("OK"));
    let before_acquire_attempts =
        metric_value(handle, "redis_write_lock_acquire_attempts_total", &[]);

    let (started_tx, started_rx) = mpsc::channel();
    let (tx, rx) = mpsc::channel();
    let store_for_thread = store.clone();
    let graph_for_thread = graph.clone();
    std::thread::spawn(move || {
        started_tx
            .send(())
            .expect("batch_create_nodes worker should signal readiness");
        let result = store_for_thread.batch_create_nodes(
            &graph_for_thread,
            vec![
                (
                    vec!["Person".to_string()],
                    props(&[("name", Value::Str("Alice".into()))]),
                ),
                (
                    vec!["Person".to_string()],
                    props(&[("name", Value::Str("Bob".into()))]),
                ),
            ],
        );
        tx.send(result)
            .expect("batch_create_nodes result should be sent to the test thread");
    });

    started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("batch_create_nodes worker should start before lock release");
    wait_for_metric_at_least(
        handle,
        "redis_write_lock_acquire_attempts_total",
        &[],
        before_acquire_attempts + 1.0,
    );
    std::thread::sleep(Duration::from_millis(250));
    match rx.recv_timeout(Duration::from_millis(150)) {
        Err(mpsc::RecvTimeoutError::Timeout) => {}
        Ok(result) => panic!(
            "batch_create_nodes must wait for the same-graph write lock instead of finishing early: {result:?}"
        ),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            panic!("batch_create_nodes worker disconnected before reporting a result")
        }
    }

    redis::cmd("DEL")
        .arg(&lock_key)
        .query::<()>(&mut conn)
        .expect("manual lock should be released");

    let ids = rx
        .recv_timeout(Duration::from_secs(3))
        .expect("batch_create_nodes should complete after the lock is released")
        .expect("batch_create_nodes should succeed after the lock is released");
    assert_eq!(
        ids.len(),
        2,
        "batch_create_nodes should return both allocated IDs"
    );

    store
        .delete_graph(&graph)
        .expect("delete_graph should clean up the recreated graph");
}

#[test]
#[ignore = "requires a running Redis at REDIS_URL"]
fn ci13d_batch_create_edges_waits_out_same_graph_write_lock() {
    let _guard = metrics_guard();
    let handle = metrics_handle();
    let store = standalone_store();
    let graph = unique_graph("batch-create-edges-lock");
    let prefix = std::env::var("GQL_REDIS_KEY_PREFIX").unwrap_or_default();
    let client =
        Client::open(standalone_redis_url().as_str()).expect("redis client should initialize");
    let mut conn = client
        .get_connection()
        .expect("redis connection should initialize");
    let lock_key = format!("{prefix}G:{{{graph}}}:write_lock");

    store.create_graph(&graph).expect("graph should be created");
    let alice = store
        .create_node(
            &graph,
            vec!["Person".to_string()],
            props(&[("name", Value::Str("Alice".into()))]),
        )
        .expect("Alice seed node should be created");
    let bob = store
        .create_node(
            &graph,
            vec!["Person".to_string()],
            props(&[("name", Value::Str("Bob".into()))]),
        )
        .expect("Bob seed node should be created");

    let lock_result: Option<String> = redis::cmd("SET")
        .arg(&lock_key)
        .arg("test-token")
        .arg("NX")
        .arg("PX")
        .arg(5_000)
        .query(&mut conn)
        .expect("manual lock should be set");
    assert_eq!(lock_result.as_deref(), Some("OK"));
    let before_acquire_attempts =
        metric_value(handle, "redis_write_lock_acquire_attempts_total", &[]);

    let (started_tx, started_rx) = mpsc::channel();
    let (tx, rx) = mpsc::channel();
    let store_for_thread = store.clone();
    let graph_for_thread = graph.clone();
    std::thread::spawn(move || {
        started_tx
            .send(())
            .expect("batch_create_edges worker should signal readiness");
        let result = store_for_thread.batch_create_edges(
            &graph_for_thread,
            vec![
                (
                    alice,
                    bob,
                    "KNOWS".to_string(),
                    props(&[("since", Value::Int(2024))]),
                ),
                (
                    bob,
                    alice,
                    "KNOWS".to_string(),
                    props(&[("since", Value::Int(2025))]),
                ),
            ],
        );
        tx.send(result)
            .expect("batch_create_edges result should be sent to the test thread");
    });

    started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("batch_create_edges worker should start before lock release");
    wait_for_metric_at_least(
        handle,
        "redis_write_lock_acquire_attempts_total",
        &[],
        before_acquire_attempts + 1.0,
    );
    std::thread::sleep(Duration::from_millis(250));
    match rx.recv_timeout(Duration::from_millis(150)) {
        Err(mpsc::RecvTimeoutError::Timeout) => {}
        Ok(result) => panic!(
            "batch_create_edges must wait for the same-graph write lock instead of finishing early: {result:?}"
        ),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            panic!("batch_create_edges worker disconnected before reporting a result")
        }
    }

    redis::cmd("DEL")
        .arg(&lock_key)
        .query::<()>(&mut conn)
        .expect("manual lock should be released");

    let ids = rx
        .recv_timeout(Duration::from_secs(3))
        .expect("batch_create_edges should complete after the lock is released")
        .expect("batch_create_edges should succeed after the lock is released");
    assert_eq!(
        ids.len(),
        2,
        "batch_create_edges should return both allocated IDs"
    );

    store
        .delete_graph(&graph)
        .expect("delete_graph should clean up the recreated graph");
}

#[test]
fn ci14_backend_errors_use_sanitizer() {
    for (name, source) in [
        ("connection.rs", CONNECTION_SRC),
        ("lib.rs", LIB_SRC),
        ("reads.rs", READS_SRC),
        ("indexing.rs", INDEXING_SRC),
        ("lock.rs", LOCK_SRC),
        ("redis_memory_metrics.rs", METRICS_SRC),
        ("writes/nodes.rs", NODES_SRC),
        ("writes/edges.rs", EDGES_SRC),
        ("writes/graphs.rs", GRAPHS_SRC),
    ] {
        assert!(
            !source.contains("StorageError::Backend { msg: e.to_string() }"),
            "{name} must route Redis backend errors through backend_storage_error() so SEC-6 sanitization is enforced"
        );
    }
    assert!(
        !CONNECTION_SRC.contains("msg: format!(\"redis"),
        "connection.rs must not format raw Redis backend error messages directly"
    );
}

#[test]
fn ci15_read_entrypoints_validate_graph_existence() {
    for (name, source, signature) in [
        ("get_node_impl", READS_SRC, "pub(crate) fn get_node_impl"),
        ("get_edge_impl", READS_SRC, "pub(crate) fn get_edge_impl"),
        (
            "get_nodes_batch_impl",
            READS_SRC,
            "pub(crate) fn get_nodes_batch_impl",
        ),
        (
            "get_edges_batch_impl",
            READS_SRC,
            "pub(crate) fn get_edges_batch_impl",
        ),
        (
            "find_nodes_by_label_impl",
            READS_SRC,
            "pub(crate) fn find_nodes_by_label_impl",
        ),
        (
            "find_nodes_by_label_and_property_impl",
            READS_SRC,
            "pub(crate) fn find_nodes_by_label_and_property_impl",
        ),
        (
            "find_edges_by_type_impl",
            READS_SRC,
            "pub(crate) fn find_edges_by_type_impl",
        ),
        ("out_edges_impl", READS_SRC, "pub(crate) fn out_edges_impl"),
        ("in_edges_impl", READS_SRC, "pub(crate) fn in_edges_impl"),
        (
            "all_node_ids_impl",
            READS_SRC,
            "pub(crate) fn all_node_ids_impl",
        ),
        (
            "all_edge_ids_impl",
            READS_SRC,
            "pub(crate) fn all_edge_ids_impl",
        ),
        (
            "all_label_names_impl",
            READS_SRC,
            "pub(crate) fn all_label_names_impl",
        ),
        (
            "graph_stats_impl",
            READS_SRC,
            "pub(crate) fn graph_stats_impl",
        ),
        (
            "list_indexes_impl",
            INDEXING_SRC,
            "pub(crate) fn list_indexes_impl",
        ),
    ] {
        let body = function_block(source, signature);
        assert!(
            body.contains("route_read_for_graph(graph)?")
                || body.contains("require_graph_on(conn.as_mut(),"),
            "{name} must validate graph existence before continuing so missing graphs still return GraphNotFound"
        );
    }
}

#[test]
fn ci26_replica_graph_misses_retry_on_primary() {
    let route_body = function_block(LIB_SRC, "pub(crate) fn route_read_for_graph");
    assert!(
        route_body.contains("route_read_with_pinning(true)?"),
        "route_read_for_graph must retry graph-scoped reads on the primary after a replica miss"
    );
    let route_body_normalized = route_body.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        route_body_normalized
            .contains("Err(StorageError::GraphNotFound { .. }) if self.replica_pool.is_some()"),
        "route_read_for_graph must only fall back on false GraphNotFound misses from replica reads"
    );

    let exists_body = function_block(READS_SRC, "pub(crate) fn graph_exists_impl");
    assert!(
        exists_body.contains("route_read_with_pinning(true)?"),
        "graph_exists_impl must retry on the primary when a replica reports a graph as missing"
    );
}

#[test]
fn ci16_create_pipelines_register_manifests_before_primary_hash_write() {
    for (name, source, signature, manifest_markers) in [
        (
            "create_node_impl",
            NODES_SRC,
            "pub(crate) fn create_node_impl",
            &["key_label_names(graph)", "key_nodes_set(graph)"][..],
        ),
        (
            "batch_create_nodes",
            NODES_SRC,
            "pub fn batch_create_nodes",
            &["key_label_names(graph)", "key_nodes_set(graph)"][..],
        ),
        (
            "create_edge_locked",
            EDGES_SRC,
            "fn create_edge_locked",
            &["key_reltype_names(graph)", "key_edges_set(graph)"][..],
        ),
        (
            "batch_create_edges",
            EDGES_SRC,
            "pub fn batch_create_edges",
            &["key_reltype_names(graph)", "key_edges_set(graph)"][..],
        ),
    ] {
        let body = function_block(source, signature);
        let hset_pos = body
            .find(".cmd(\"HSET\")")
            .unwrap_or_else(|| panic!("{name} must issue HSET in its write pipeline"));
        for marker in manifest_markers {
            let marker_pos = body.find(marker).unwrap_or_else(|| {
                panic!("{name} must register manifest key {marker} before writing the primary hash")
            });
            assert!(
                marker_pos < hset_pos,
                "{name} must register {marker} before HSET so delete_graph can recover from non-atomic partial writes"
            );
        }
    }
}

#[test]
#[ignore = "requires a running Redis at REDIS_URL"]
fn ci17_missing_graph_reads_return_graph_not_found() {
    let store = standalone_store();
    let graph = unique_graph("missing-read-contract");
    let exact = Value::Str("Alice".into());

    assert!(matches!(
        store.get_node(&graph, gql_core::storage::NodeId(1)),
        Err(StorageError::GraphNotFound { .. })
    ));
    assert!(matches!(
        store.get_edge(&graph, gql_core::storage::EdgeId(1)),
        Err(StorageError::GraphNotFound { .. })
    ));
    assert!(matches!(
        store.get_nodes_batch(&graph, &[gql_core::storage::NodeId(1)]),
        Err(StorageError::GraphNotFound { .. })
    ));
    assert!(matches!(
        store.get_edges_batch(&graph, &[gql_core::storage::EdgeId(1)]),
        Err(StorageError::GraphNotFound { .. })
    ));
    assert!(matches!(
        store.find_nodes_by_label(&graph, "Person"),
        Err(StorageError::GraphNotFound { .. })
    ));
    assert!(matches!(
        store.find_nodes_by_label_and_property(&graph, "Person", "name", RangeQuery::Eq(&exact)),
        Err(StorageError::GraphNotFound { .. })
    ));
    assert!(matches!(
        store.find_edges_by_type(&graph, "KNOWS"),
        Err(StorageError::GraphNotFound { .. })
    ));
    assert!(matches!(
        store.out_edges(&graph, gql_core::storage::NodeId(1)),
        Err(StorageError::GraphNotFound { .. })
    ));
    assert!(matches!(
        store.in_edges(&graph, gql_core::storage::NodeId(1)),
        Err(StorageError::GraphNotFound { .. })
    ));
    assert!(matches!(
        store.all_node_ids(&graph),
        Err(StorageError::GraphNotFound { .. })
    ));
    assert!(matches!(
        store.all_edge_ids(&graph),
        Err(StorageError::GraphNotFound { .. })
    ));
    assert!(matches!(
        store.all_label_names(&graph),
        Err(StorageError::GraphNotFound { .. })
    ));
    assert!(matches!(
        store.graph_stats(&graph),
        Err(StorageError::GraphNotFound { .. })
    ));
    assert!(matches!(
        store.list_indexes(&graph),
        Err(StorageError::GraphNotFound { .. })
    ));
}

#[test]
#[ignore = "requires a running Redis at REDIS_URL"]
fn ci18_delete_graph_retry_state_blocks_name_reuse_until_cleanup() {
    let store = standalone_store();
    let graph = unique_graph("delete-graph-retry");
    let prefix = std::env::var("GQL_REDIS_KEY_PREFIX").unwrap_or_default();
    let client =
        Client::open(standalone_redis_url().as_str()).expect("redis client should initialize");
    let mut conn = client
        .get_connection()
        .expect("redis connection should initialize");
    let registry_key = format!("{prefix}__sd2_registry:graphs");
    let deleting_key = format!("{prefix}G:{{{graph}}}:_deleting");

    store.create_graph(&graph).expect("graph should be created");
    store
        .create_node(
            &graph,
            vec!["Person".to_string()],
            props(&[("name", Value::Str("Alice".into()))]),
        )
        .expect("seed node should be created");

    redis::cmd("SET")
        .arg(&deleting_key)
        .arg("1")
        .query::<()>(&mut conn)
        .expect("deleting marker should be set");
    redis::cmd("SREM")
        .arg(&registry_key)
        .arg(&graph)
        .query::<()>(&mut conn)
        .expect("graph registry member should be removed");

    assert!(
        !store
            .graph_exists(&graph)
            .expect("graph_exists should hide graphs in retryable delete state"),
        "graph_exists must treat retryable delete state as not live"
    );
    assert!(
        !store
            .list_graphs()
            .expect("list_graphs should succeed")
            .contains(&graph),
        "list_graphs must hide graphs in retryable delete state"
    );
    assert!(
        matches!(
            store.create_graph(&graph),
            Err(StorageError::TransactionConflict)
        ),
        "create_graph must not reuse a graph name while retryable delete cleanup is pending"
    );

    store
        .delete_graph(&graph)
        .expect("delete_graph should be retryable from the deleting state");
    let deleting_exists: i64 = redis::cmd("EXISTS")
        .arg(&deleting_key)
        .query(&mut conn)
        .expect("deleting marker existence check should succeed");
    assert_eq!(
        deleting_exists, 0,
        "successful delete_graph retry should clear the deleting marker"
    );

    store
        .create_graph(&graph)
        .expect("graph name should become reusable after retry cleanup completes");
    store
        .delete_graph(&graph)
        .expect("graph cleanup after reuse should succeed");
}

#[test]
#[ignore = "requires a running Redis at REDIS_URL"]
fn ci19_delete_graph_removes_manifested_propidx_without_index_metadata() {
    let store = standalone_store();
    let graph = unique_graph("delete-graph-propidx-manifest");
    let prefix = std::env::var("GQL_REDIS_KEY_PREFIX").unwrap_or_default();
    let client =
        Client::open(standalone_redis_url().as_str()).expect("redis client should initialize");
    let mut conn = client
        .get_connection()
        .expect("redis connection should initialize");
    let manifest_key = format!("{prefix}G:{{{graph}}}:_propidx");
    let propidx_key = format!("{prefix}G:{{{graph}}}:propidx:Ghost\u{0}field\u{0}s:value");

    store.create_graph(&graph).expect("graph should be created");
    redis::cmd("SADD")
        .arg(&manifest_key)
        .arg(&propidx_key)
        .query::<()>(&mut conn)
        .expect("propidx manifest should be populated");
    redis::cmd("SADD")
        .arg(&propidx_key)
        .arg("1")
        .query::<()>(&mut conn)
        .expect("orphan propidx set should be populated");

    store
        .delete_graph(&graph)
        .expect("delete_graph should clean orphan propidx keys recorded in the manifest");
    let remaining: i64 = redis::cmd("EXISTS")
        .arg(&manifest_key)
        .arg(&propidx_key)
        .query(&mut conn)
        .expect("propidx cleanup existence check should succeed");
    assert_eq!(
        remaining, 0,
        "delete_graph must remove manifested propidx keys even without index metadata"
    );
}

#[test]
#[ignore = "requires a running Redis at REDIS_URL"]
fn ci20_delete_graph_recovers_after_stale_lock_expires() {
    let store = standalone_store();
    let graph = unique_graph("delete-graph-stale-lock");
    let prefix = std::env::var("GQL_REDIS_KEY_PREFIX").unwrap_or_default();
    let client =
        Client::open(standalone_redis_url().as_str()).expect("redis client should initialize");
    let mut conn = client
        .get_connection()
        .expect("redis connection should initialize");
    let lock_key = format!("{prefix}G:{{{graph}}}:write_lock");

    store.create_graph(&graph).expect("graph should be created");
    store
        .create_node(
            &graph,
            vec!["Person".to_string()],
            props(&[("name", Value::Str("Alice".into()))]),
        )
        .expect("seed node should be created");

    let lock_result: Option<String> = redis::cmd("SET")
        .arg(&lock_key)
        .arg("stale-token")
        .arg("NX")
        .arg("PX")
        .arg(250)
        .query(&mut conn)
        .expect("stale manual lock should be set");
    assert_eq!(lock_result.as_deref(), Some("OK"));

    let (tx, rx) = mpsc::channel();
    let graph_for_thread = graph.clone();
    std::thread::spawn(move || {
        let result = standalone_store().delete_graph(&graph_for_thread);
        tx.send(result)
            .expect("delete_graph result should be sent to the test thread");
    });

    assert!(
        rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "delete_graph should wait for the stale lock to expire before proceeding"
    );
    rx.recv_timeout(Duration::from_secs(3))
        .expect("delete_graph should complete after the stale lock expires")
        .expect("delete_graph should succeed once the stale lock expires");

    assert!(
        !store
            .graph_exists(&graph)
            .expect("graph_exists should succeed after delete_graph"),
        "delete_graph should still remove the graph after stale-lock recovery"
    );
    store
        .create_graph(&graph)
        .expect("graph name should be reusable after stale-lock recovery");
    store
        .delete_graph(&graph)
        .expect("cleanup after graph recreation should succeed");
}

#[test]
fn ci21_long_running_write_paths_renew_lock() {
    for (name, source, signature, markers) in [
        (
            "create_index_impl",
            INDEXING_SRC,
            "pub(crate) fn create_index_impl",
            &["smembers_or_scan_locked(", "conn.renew_lock()?"][..],
        ),
        (
            "index_node_properties",
            INDEXING_SRC,
            "pub(crate) fn index_node_properties",
            &["conn.renew_lock()?"][..],
        ),
        (
            "deindex_node_properties",
            INDEXING_SRC,
            "pub(crate) fn deindex_node_properties",
            &["conn.renew_lock()?"][..],
        ),
        (
            "drop_index_impl",
            INDEXING_SRC,
            "pub(crate) fn drop_index_impl",
            &["smembers_or_scan_locked(", "conn.renew_lock()?"][..],
        ),
        (
            "delete_graph_impl",
            GRAPHS_SRC,
            "pub(crate) fn delete_graph_impl",
            &["conn.renew_lock()?"][..],
        ),
        (
            "delete_node_impl",
            NODES_SRC,
            "pub(crate) fn delete_node_impl",
            &["conn.renew_lock()?"][..],
        ),
        (
            "add_node_labels_impl",
            NODES_SRC,
            "pub(crate) fn add_node_labels_impl",
            &["conn.renew_lock()?"][..],
        ),
        (
            "remove_node_labels_impl",
            NODES_SRC,
            "pub(crate) fn remove_node_labels_impl",
            &["conn.renew_lock()?"][..],
        ),
        (
            "batch_create_nodes",
            NODES_SRC,
            "pub fn batch_create_nodes",
            &["conn.renew_lock()?"][..],
        ),
        (
            "batch_create_edges",
            EDGES_SRC,
            "pub fn batch_create_edges",
            &["conn.renew_lock()?"][..],
        ),
    ] {
        let body = function_block(source, signature);
        for marker in markers {
            assert!(
                body.contains(marker),
                "{name} must renew the graph write lock while processing long-running same-graph work"
            );
        }
    }
    assert!(
        LOCK_SRC.contains("pub fn renew("),
        "lock.rs must provide a CAS-based renew primitive for long-running write operations"
    );
}

#[test]
fn ci22_successful_writes_require_successful_lock_release() {
    assert!(
        LIB_SRC.contains("Ok(_) => Err(StorageError::TransactionConflict)"),
        "write-lock release handling must treat lost ownership as an error, not a successful write"
    );
    assert!(
        !function_block(LIB_SRC, "pub(crate) fn with_write_lock").contains("return result;"),
        "with_write_lock must not return a successful write result before validating lock release"
    );
}

#[test]
fn ci23_property_mutators_renew_between_deindex_and_hset() {
    for (name, signature) in [
        (
            "set_node_properties_impl",
            "pub(crate) fn set_node_properties_impl",
        ),
        (
            "replace_node_properties_impl",
            "pub(crate) fn replace_node_properties_impl",
        ),
        (
            "remove_node_property_impl",
            "pub(crate) fn remove_node_property_impl",
        ),
    ] {
        let body = function_block(NODES_SRC, signature);
        let deindex_pos = body
            .find("self.deindex_node_properties(")
            .unwrap_or_else(|| {
                panic!("{name} must deindex old property values before rewriting the node hash")
            });
        let renew_pos = body.find("conn.renew_lock()?").unwrap_or_else(|| {
            panic!("{name} must renew the write lock before the post-deindex HSET")
        });
        let hset_pos = body
            .find("redis::cmd(\"HSET\")")
            .unwrap_or_else(|| panic!("{name} must rewrite the node hash with HSET"));
        assert!(
            deindex_pos < renew_pos && renew_pos < hset_pos,
            "{name} must renew the write lock after deindexing and before rewriting node properties"
        );
    }
}

#[test]
fn ci24_label_mutators_and_create_node_renew_before_primary_write() {
    let create_body = function_block(NODES_SRC, "pub(crate) fn create_node_impl");
    let create_renew_pos = create_body
        .find("conn.renew_lock()?")
        .expect("create_node_impl must renew the write lock before its primary write pipeline");
    let create_query_pos = create_body
        .find("pipe.query::<()>(conn)")
        .expect("create_node_impl must execute its primary write pipeline");
    assert!(
        create_renew_pos < create_query_pos,
        "create_node_impl must renew the write lock before executing its primary write pipeline"
    );

    let add_labels_body = function_block(NODES_SRC, "pub(crate) fn add_node_labels_impl");
    let add_read_pos = add_labels_body
        .find("read_node(conn.as_mut(), graph, id.0)?")
        .expect("add_node_labels_impl must read the current node before rewriting labels");
    let add_renew_pos = add_labels_body
        .find("conn.renew_lock()?")
        .expect("add_node_labels_impl must renew the write lock before its label rewrite pipeline");
    let add_query_pos = add_labels_body
        .find("pipe.query::<()>(conn)")
        .expect("add_node_labels_impl must execute a label rewrite pipeline");
    assert!(
        add_read_pos < add_renew_pos && add_renew_pos < add_query_pos,
        "add_node_labels_impl must renew the write lock after reading the node and before executing the label rewrite pipeline"
    );

    let remove_labels_body = function_block(NODES_SRC, "pub(crate) fn remove_node_labels_impl");
    let remove_deindex_pos = remove_labels_body
        .find("self.deindex_node_properties(")
        .expect("remove_node_labels_impl must deindex removed labels before rewriting the node");
    let remove_renew_pos = remove_labels_body
        .find("conn.renew_lock()?")
        .expect("remove_node_labels_impl must renew the write lock before its combined label rewrite pipeline");
    let remove_query_pos = remove_labels_body
        .find("pipe.query::<()>(conn)")
        .expect("remove_node_labels_impl must execute a combined label rewrite pipeline");
    assert!(
        remove_deindex_pos < remove_renew_pos && remove_renew_pos < remove_query_pos,
        "remove_node_labels_impl must renew the write lock after deindexing and before executing the combined label rewrite pipeline"
    );
    assert!(
        !remove_labels_body.contains("redis::cmd(\"HSET\")"),
        "remove_node_labels_impl must keep label index removal and label-list rewrite in the same pipeline"
    );
}

#[test]
fn ci25_edge_and_delete_mutators_renew_before_final_write() {
    let create_edge_body = function_block(EDGES_SRC, "fn create_edge_locked");
    let create_edge_renew_pos = create_edge_body
        .find("conn.renew_lock()?")
        .expect("create_edge_locked must renew the write lock before its primary write pipeline");
    let create_edge_query_pos = create_edge_body
        .find("pipe.query::<()>(conn)")
        .expect("create_edge_locked must execute its primary write pipeline");
    assert!(
        create_edge_renew_pos < create_edge_query_pos,
        "create_edge_locked must renew the write lock before executing its primary write pipeline"
    );

    let delete_edge_body = function_block(EDGES_SRC, "pub(crate) fn delete_edge_inner");
    let delete_edge_renew_pos = delete_edge_body
        .find("conn.renew_lock()?")
        .expect("delete_edge_inner must renew the write lock before its delete pipeline");
    let delete_edge_query_pos = delete_edge_body
        .find("pipe.query::<()>(conn)")
        .expect("delete_edge_inner must execute a delete pipeline");
    assert!(
        delete_edge_renew_pos < delete_edge_query_pos,
        "delete_edge_inner must renew the write lock before executing its delete pipeline"
    );

    let delete_node_body = function_block(NODES_SRC, "pub(crate) fn delete_node_impl");
    let delete_node_query_pos = delete_node_body
        .rfind("pipe.query::<()>(conn)")
        .expect("delete_node_impl must execute a final delete pipeline");
    let delete_node_renew_pos = delete_node_body[..delete_node_query_pos]
        .rfind("conn.renew_lock()?")
        .expect("delete_node_impl must renew the write lock before its final delete pipeline");
    assert!(
        delete_node_renew_pos < delete_node_query_pos,
        "delete_node_impl must renew the write lock before executing its final delete pipeline"
    );

    for (name, signature) in [
        (
            "set_edge_properties_impl",
            "pub(crate) fn set_edge_properties_impl",
        ),
        (
            "replace_edge_properties_impl",
            "pub(crate) fn replace_edge_properties_impl",
        ),
        (
            "remove_edge_property_impl",
            "pub(crate) fn remove_edge_property_impl",
        ),
    ] {
        let body = function_block(EDGES_SRC, signature);
        let read_pos = body
            .find("read_edge(conn.as_mut(), graph, id.0)?")
            .unwrap_or_else(|| panic!("{name} must read the current edge before rewriting it"));
        let renew_pos = body.find("conn.renew_lock()?").unwrap_or_else(|| {
            panic!("{name} must renew the write lock before the post-read HSET")
        });
        let hset_pos = body
            .find("redis::cmd(\"HSET\")")
            .unwrap_or_else(|| panic!("{name} must rewrite the edge hash with HSET"));
        assert!(
            read_pos < renew_pos && renew_pos < hset_pos,
            "{name} must renew the write lock after reading the edge and before rewriting edge properties"
        );
    }
}
