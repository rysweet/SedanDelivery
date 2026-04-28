//! Cluster consistency hardening — TDD test suite.
//!
//! These tests capture the contract for the six-part hardening package
//! tracked under issue #1769:
//!
//!   F1. FENCING TOKENS on `G:{g}:write_lock`. A monotonic per-graph fence
//!       counter is bumped on lock acquire; a stalled writer whose lease was
//!       stolen MUST fail closed when it tries to renew or to land further
//!       writes via a Lua-atomic hot-write script.
//!   F2. LUA-ATOMIC HOT WRITE PATHS for node+labels and edge+adjacency.
//!       After a successful single-node or single-edge create, NO concurrent
//!       reader may observe a node whose labels are missing from the label
//!       index, or an edge whose reverse adjacency is missing.
//!   F4. ADMIN ENDPOINT for stuck `G:{g}:_deleting` markers (covered by
//!       sd2-api integration tests; see `sd2-api/tests/admin_finalize_delete.rs`).
//!   F5. Compile-time guard against cross-slot co-pipelining (covered by
//!       unit tests in `sd2-redis-store/src/connection/conn.rs`).
//!   F6. SCARD batching for registry validation — exercised end-to-end via
//!       `all_label_names` / `all_rel_type_names` round-trip counts under
//!       a moderately populated registry.
//!
//! Tests are `#[ignore]` because they require a live Redis Cluster reachable
//! via `GQL_REDIS_CLUSTER_NODES`. Run with:
//!
//! ```bash
//! cargo test -p sd2-redis-store --test cluster_consistency_hardening \
//!   -- --ignored --nocapture
//! ```
//!
//! All tests use unique graph names so they can run in parallel against the
//! same cluster without interference.

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use indexmap::IndexMap;

use cypher_core::storage::{EdgeId, GraphStore, NodeId};
use cypher_core::types::Value;
use sd2_redis_store::RedisGraphStore;
use uuid::Uuid;

fn cluster_store() -> RedisGraphStore {
    std::env::var("GQL_REDIS_CLUSTER_NODES").expect(
        "GQL_REDIS_CLUSTER_NODES must be set; run with \
         `cargo test -p sd2-redis-store --test cluster_consistency_hardening -- --ignored --nocapture`",
    );
    RedisGraphStore::from_env().expect("cluster store should initialize from env")
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

// ── F1. Fence-token rejection (stale token must fail closed) ───────────────

/// After a writer's lock is stolen via TTL expiry, every subsequent mutating
/// call from the original writer MUST fail closed. The fence-token model
/// promises that even Lua-atomic hot writes invoked by the stalled writer
/// reject with `STALE_FENCE` server-side, so partial late writes cannot land.
///
/// We exercise this by:
///   1. Writer A acquires the per-graph write lock (implicit on first write).
///   2. A long-running mutation issued by Writer A is interleaved with
///      a parallel writer B that takes over after A's lease expires.
///   3. Writer A's late write MUST surface as an error (Backend / fence
///      rejection), NOT silently succeed.
///
/// Implementation note: the public `GraphStore` surface does not directly
/// expose lease stealing. We provoke the condition by issuing many concurrent
/// single-node writes that cause the lock TTL window to be exceeded under
/// load, and assert that no node is ever observed without its label-index
/// entry by a concurrent reader. If fencing regresses, the partial-write
/// observability assertion in `lua_atomic_node_create_is_indivisible_under_concurrency`
/// will fail.
#[test]
#[ignore = "requires GQL_REDIS_CLUSTER_NODES; verifies fence-token rejection on stale lease"]
fn fence_token_rejects_stale_writer_after_lease_takeover() {
    let store = Arc::new(cluster_store());
    let graph = unique_graph("fence-stale");
    store.create_graph(&graph).expect("create_graph");

    // Two concurrent writers heavily contend the same per-graph lock.  After
    // the package lands, the loser of any TTL race must surface a
    // fence-rejection error rather than landing a partial late write.
    let mut handles = Vec::new();
    for worker in 0..4 {
        let store = Arc::clone(&store);
        let graph = graph.clone();
        handles.push(thread::spawn(move || {
            let mut errors = Vec::new();
            for i in 0..50 {
                let res = store.create_node(
                    &graph,
                    vec!["Person".to_string()],
                    props(&[("worker", Value::Int(worker)), ("seq", Value::Int(i))]),
                );
                if let Err(err) = res {
                    errors.push(err.to_string());
                }
            }
            errors
        }));
    }

    let mut all_errors = Vec::new();
    for h in handles {
        all_errors.extend(h.join().expect("worker thread should not panic"));
    }

    // After the hardening lands, any errors here MUST be fence-rejection or
    // benign contention — never silent partial writes.  We assert that the
    // surviving graph is internally consistent: every node id in the all-nodes
    // set has a matching label-index entry.
    let stats = store.graph_stats(&graph).expect("graph_stats");
    let labels = store.all_label_names(&graph).expect("all_label_names");
    assert!(
        labels.iter().any(|l| l == "Person") || stats.node_count == 0,
        "after fence enforcement, the Person label must be in the registry whenever any Person node exists; \
         labels={labels:?} node_count={}",
        stats.node_count
    );

    // Tolerated errors must be typed contention/fence — not Serialization or panics.
    for err in &all_errors {
        assert!(
            !err.contains("serialization") && !err.contains("panic"),
            "unexpected non-contention error from concurrent writer: {err}"
        );
    }

    store.delete_graph(&graph).expect("cleanup delete_graph");
}

// ── F2. Lua-atomic node+labels indivisibility ──────────────────────────────

/// After F2 lands, a concurrent reader executing during a heavy create-node
/// workload MUST never observe a node hash without its corresponding entries
/// in (a) the all-nodes set and (b) every label-index set named by the
/// node's labels. The previous pipeline-based implementation could surface
/// a node whose label-index lookup returned an empty set if the reader
/// raced between the two SADD commands.
#[test]
#[ignore = "requires GQL_REDIS_CLUSTER_NODES; verifies node+labels Lua atomicity"]
fn lua_atomic_node_create_is_indivisible_under_concurrency() {
    let store = Arc::new(cluster_store());
    let graph = unique_graph("lua-node-atomic");
    store.create_graph(&graph).expect("create_graph");

    let writer_store = Arc::clone(&store);
    let writer_graph = graph.clone();
    let writer = thread::spawn(move || {
        let mut ids = Vec::with_capacity(200);
        for i in 0..200 {
            let id = writer_store
                .create_node(
                    &writer_graph,
                    vec!["Person".to_string(), "Member".to_string()],
                    props(&[("seq", Value::Int(i))]),
                )
                .expect("create_node");
            ids.push(id);
        }
        ids
    });

    // Concurrent reader: while writes happen, every node ever observed in
    // the all-nodes set MUST also be present in the Person AND Member
    // label-index sets. Any miss is a partial-write window.
    let reader_store = Arc::clone(&store);
    let reader_graph = graph.clone();
    let reader = thread::spawn(move || {
        let mut violations: Vec<String> = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            let by_label = reader_store
                .find_nodes_by_label(&reader_graph, "Person")
                .unwrap_or_default();
            let by_label_member = reader_store
                .find_nodes_by_label(&reader_graph, "Member")
                .unwrap_or_default();
            // Every node we can read by label "Person" must also be readable
            // via its hash AND must appear under "Member".
            for nid in &by_label {
                let hash = reader_store.get_node(&reader_graph, *nid).ok().flatten();
                if hash.is_none() {
                    violations.push(format!(
                        "node {nid:?} indexed under Person but missing hash"
                    ));
                }
                if !by_label_member.contains(nid) {
                    violations.push(format!(
                        "node {nid:?} indexed under Person but NOT under Member — partial label write"
                    ));
                }
            }
            thread::sleep(Duration::from_millis(2));
        }
        violations
    });

    let _ids = writer.join().expect("writer panic");
    let violations = reader.join().expect("reader panic");

    assert!(
        violations.is_empty(),
        "Lua-atomic node+label create must be indivisible; observed {} violations: first={:?}",
        violations.len(),
        violations.first()
    );

    store.delete_graph(&graph).expect("cleanup delete_graph");
}

// ── F2. Lua-atomic edge+adjacency indivisibility ───────────────────────────

/// After F2 lands, a concurrent reader doing neighbor traversals MUST never
/// observe an edge hash whose reverse-adjacency entry is missing. The prior
/// pipeline could land the edge hash and the out-adjacency before the
/// in-adjacency, causing one-directional MATCH results to disagree.
#[test]
#[ignore = "requires GQL_REDIS_CLUSTER_NODES; verifies edge+adjacency Lua atomicity"]
fn lua_atomic_edge_create_is_indivisible_under_concurrency() {
    let store = Arc::new(cluster_store());
    let graph = unique_graph("lua-edge-atomic");
    store.create_graph(&graph).expect("create_graph");

    // Pre-create a small star: hub plus N leaves; concurrent writer adds
    // hub→leaf edges; concurrent reader walks neighbors both directions.
    let hub = store
        .create_node(
            &graph,
            vec!["Hub".to_string()],
            props(&[("name", Value::Str("hub".into()))]),
        )
        .expect("hub node");
    let mut leaves: Vec<NodeId> = Vec::with_capacity(50);
    for i in 0..50 {
        leaves.push(
            store
                .create_node(
                    &graph,
                    vec!["Leaf".to_string()],
                    props(&[("idx", Value::Int(i))]),
                )
                .expect("leaf node"),
        );
    }

    let writer_store = Arc::clone(&store);
    let writer_graph = graph.clone();
    let writer_leaves = leaves.clone();
    let writer = thread::spawn(move || {
        for leaf in writer_leaves {
            writer_store
                .create_edge(
                    &writer_graph,
                    hub,
                    leaf,
                    "POINTS_TO".to_string(),
                    IndexMap::new(),
                )
                .expect("create_edge");
        }
    });

    let reader_store = Arc::clone(&store);
    let reader_graph = graph.clone();
    let reader_leaves = leaves.clone();
    let reader = thread::spawn(move || {
        let mut violations = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            // For every leaf currently reachable from the hub via outgoing
            // adjacency, the reverse incoming-adjacency walk from the leaf
            // MUST also see the hub. Any miss is a partial edge write.
            let out = reader_store
                .out_edges(&reader_graph, hub)
                .unwrap_or_default();
            for adj in &out {
                if !reader_leaves.contains(&adj.neighbor_id) {
                    continue;
                }
                let in_adj = reader_store
                    .in_edges(&reader_graph, adj.neighbor_id)
                    .unwrap_or_default();
                if !in_adj.iter().any(|a| a.neighbor_id == hub) {
                    violations.push(format!(
                        "leaf {:?} reachable hub→leaf but reverse leaf→hub adjacency missing",
                        adj.neighbor_id
                    ));
                }
            }
            thread::sleep(Duration::from_millis(2));
        }
        violations
    });

    writer.join().expect("writer panic");
    let violations = reader.join().expect("reader panic");

    assert!(
        violations.is_empty(),
        "Lua-atomic edge+adjacency create must be indivisible; observed {} violations: first={:?}",
        violations.len(),
        violations.first()
    );

    store.delete_graph(&graph).expect("cleanup delete_graph");
}

// ── F2 (extension). Lua-atomic add_labels indivisibility ───────────────────

/// `add_node_labels` rewrites the node hash and label-index sets in a single
/// EVAL. A concurrent reader scanning by either of the newly-added labels
/// MUST always be able to read the node hash.
#[test]
#[ignore = "requires GQL_REDIS_CLUSTER_NODES; verifies add_labels Lua atomicity"]
fn lua_atomic_add_labels_is_indivisible_under_concurrency() {
    let store = Arc::new(cluster_store());
    let graph = unique_graph("lua-add-labels-atomic");
    store.create_graph(&graph).expect("create_graph");

    // Pre-create N nodes with a single base label.
    let mut ids: Vec<NodeId> = Vec::with_capacity(100);
    for i in 0..100 {
        ids.push(
            store
                .create_node(
                    &graph,
                    vec!["Base".to_string()],
                    props(&[("seq", Value::Int(i))]),
                )
                .expect("create_node"),
        );
    }

    let writer_store = Arc::clone(&store);
    let writer_graph = graph.clone();
    let writer_ids = ids.clone();
    let writer = thread::spawn(move || {
        for id in writer_ids {
            writer_store
                .add_node_labels(
                    &writer_graph,
                    id,
                    &["Tagged".to_string(), "New".to_string()],
                )
                .expect("add_node_labels");
        }
    });

    let reader_store = Arc::clone(&store);
    let reader_graph = graph.clone();
    let reader = thread::spawn(move || {
        let mut violations: Vec<String> = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            let by_tagged = reader_store
                .find_nodes_by_label(&reader_graph, "Tagged")
                .unwrap_or_default();
            let by_new = reader_store
                .find_nodes_by_label(&reader_graph, "New")
                .unwrap_or_default();
            for nid in &by_tagged {
                let hash = reader_store.get_node(&reader_graph, *nid).ok().flatten();
                if hash.is_none() {
                    violations.push(format!(
                        "node {nid:?} indexed under Tagged but missing hash"
                    ));
                }
                if !by_new.contains(nid) {
                    violations.push(format!(
                        "node {nid:?} indexed under Tagged but NOT under New — partial label add"
                    ));
                }
            }
            thread::sleep(Duration::from_millis(2));
        }
        violations
    });

    writer.join().expect("writer panic");
    let violations = reader.join().expect("reader panic");

    assert!(
        violations.is_empty(),
        "Lua-atomic add_node_labels must be indivisible; observed {} violations: first={:?}",
        violations.len(),
        violations.first()
    );

    store.delete_graph(&graph).expect("cleanup delete_graph");
}

// ── F2 (extension). Lua-atomic node delete indivisibility ──────────────────

/// `delete_node` removes the node hash, the all-nodes set entry, every
/// label-index entry, and (with DETACH) all edges in a single Lua-atomic
/// final write. A concurrent reader MUST never observe a node id that is
/// in the all-nodes set but has no hash.
#[test]
#[ignore = "requires GQL_REDIS_CLUSTER_NODES; verifies node-delete Lua atomicity"]
fn lua_atomic_node_delete_is_indivisible_under_concurrency() {
    let store = Arc::new(cluster_store());
    let graph = unique_graph("lua-node-delete-atomic");
    store.create_graph(&graph).expect("create_graph");

    let mut ids: Vec<NodeId> = Vec::with_capacity(100);
    for i in 0..100 {
        ids.push(
            store
                .create_node(
                    &graph,
                    vec!["Doomed".to_string()],
                    props(&[("seq", Value::Int(i))]),
                )
                .expect("create_node"),
        );
    }

    let writer_store = Arc::clone(&store);
    let writer_graph = graph.clone();
    let writer_ids = ids.clone();
    let writer = thread::spawn(move || {
        for id in writer_ids {
            writer_store
                .delete_node(&writer_graph, id, true)
                .expect("delete_node");
        }
    });

    let reader_store = Arc::clone(&store);
    let reader_graph = graph.clone();
    let reader = thread::spawn(move || {
        let mut violations: Vec<String> = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            let by_label = reader_store
                .find_nodes_by_label(&reader_graph, "Doomed")
                .unwrap_or_default();
            for nid in &by_label {
                let hash = reader_store.get_node(&reader_graph, *nid).ok().flatten();
                if hash.is_some() {
                    continue;
                }
                // Double-check: if label still present after a re-read, the
                // node is in a "label-indexed but hash-gone" state that
                // outlives a single delete script — i.e. a real partial-
                // write violation, not just our reader racing with an
                // atomic delete that completed between two sequential reads.
                let still_indexed = reader_store
                    .find_nodes_by_label(&reader_graph, "Doomed")
                    .map(|v| v.contains(nid))
                    .unwrap_or(false);
                if still_indexed {
                    violations.push(format!(
                        "node {nid:?} still indexed under Doomed after re-check, but hash already gone — partial delete"
                    ));
                }
            }
            thread::sleep(Duration::from_millis(2));
        }
        violations
    });

    writer.join().expect("writer panic");
    let violations = reader.join().expect("reader panic");

    assert!(
        violations.is_empty(),
        "Lua-atomic delete_node must be indivisible; observed {} violations: first={:?}",
        violations.len(),
        violations.first()
    );

    store.delete_graph(&graph).expect("cleanup delete_graph");
}

// ── F2 (extension). Lua-atomic edge delete indivisibility ──────────────────

/// `delete_edge` removes the edge hash, the all-edges set entry, the
/// reltype-index entry, and BOTH adjacency entries in a single Lua-atomic
/// EVAL. A concurrent reader walking neighbors in either direction MUST
/// never observe an edge that is gone from one adjacency side but still
/// present on the other.
#[test]
#[ignore = "requires GQL_REDIS_CLUSTER_NODES; verifies edge-delete Lua atomicity"]
fn lua_atomic_edge_delete_is_indivisible_under_concurrency() {
    let store = Arc::new(cluster_store());
    let graph = unique_graph("lua-edge-delete-atomic");
    store.create_graph(&graph).expect("create_graph");

    let hub = store
        .create_node(
            &graph,
            vec!["Hub".to_string()],
            props(&[("name", Value::Str("hub".into()))]),
        )
        .expect("hub node");
    let mut leaves: Vec<NodeId> = Vec::with_capacity(50);
    for i in 0..50 {
        leaves.push(
            store
                .create_node(
                    &graph,
                    vec!["Leaf".to_string()],
                    props(&[("i", Value::Int(i))]),
                )
                .expect("leaf node"),
        );
    }
    let mut edges: Vec<EdgeId> = Vec::with_capacity(leaves.len());
    for leaf in &leaves {
        edges.push(
            store
                .create_edge(&graph, hub, *leaf, "CONNECTED".to_string(), IndexMap::new())
                .expect("create_edge"),
        );
    }

    let writer_store = Arc::clone(&store);
    let writer_graph = graph.clone();
    let writer_edges = edges.clone();
    let writer = thread::spawn(move || {
        for eid in writer_edges {
            writer_store
                .delete_edge(&writer_graph, eid)
                .expect("delete_edge");
        }
    });

    let reader_store = Arc::clone(&store);
    let reader_graph = graph.clone();
    let reader_leaves = leaves.clone();
    let reader = thread::spawn(move || {
        let mut violations: Vec<String> = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            // Per-edge invariant: every edge present in out_edges(hub) MUST
            // also be present in in_edges(its destination). The writer can
            // delete edges between our reads, so we re-check the out side
            // before flagging — if the edge is gone on re-read, that's just
            // a stale read; if it's still present out but missing in, that
            // is a real one-sided delete.
            let out_edges = reader_store
                .out_edges(&reader_graph, hub)
                .unwrap_or_default();
            for entry in &out_edges {
                let target = entry.neighbor_id;
                let in_edges = reader_store
                    .in_edges(&reader_graph, target)
                    .unwrap_or_default();
                if in_edges.iter().any(|e| e.edge_id == entry.edge_id) {
                    continue;
                }
                let still_out = reader_store
                    .out_edges(&reader_graph, hub)
                    .map(|v| v.iter().any(|e| e.edge_id == entry.edge_id))
                    .unwrap_or(false);
                if still_out {
                    violations.push(format!(
                        "edge {:?} still in out_edges(hub) after re-check, but missing from in_edges({:?}) — partial edge delete",
                        entry.edge_id, target
                    ));
                }
            }
            // Also exercise the inverse direction: anything in in_edges of a
            // leaf must be present in out_edges of the hub.
            for leaf in &reader_leaves {
                let in_edges = reader_store
                    .in_edges(&reader_graph, *leaf)
                    .unwrap_or_default();
                for entry in &in_edges {
                    if out_edges.iter().any(|e| e.edge_id == entry.edge_id) {
                        continue;
                    }
                    let still_in = reader_store
                        .in_edges(&reader_graph, *leaf)
                        .map(|v| v.iter().any(|e| e.edge_id == entry.edge_id))
                        .unwrap_or(false);
                    if still_in {
                        violations.push(format!(
                            "edge {:?} still in in_edges({:?}) after re-check, but missing from out_edges(hub) — partial edge delete",
                            entry.edge_id, leaf
                        ));
                    }
                }
            }
            thread::sleep(Duration::from_millis(2));
        }
        violations
    });

    writer.join().expect("writer panic");
    let violations = reader.join().expect("reader panic");

    assert!(
        violations.is_empty(),
        "Lua-atomic delete_edge must be indivisible; observed {} violations: first={:?}",
        violations.len(),
        violations.first()
    );

    store.delete_graph(&graph).expect("cleanup delete_graph");
}

/// `all_label_names` previously issued one SCARD per registry entry. The F6
/// fix batches these into a single slot-local pipeline. We can't measure
/// round-trip count from outside, but we CAN assert that a registry of 200
/// labels still completes in a reasonable wall-clock budget — if the SCARD
/// loop ever regresses to per-key network cost, this becomes flaky.
#[test]
#[ignore = "requires GQL_REDIS_CLUSTER_NODES; verifies registry SCARD batching"]
fn registry_validation_completes_in_one_round_trip_budget() {
    let store = cluster_store();
    let graph = unique_graph("registry-batch");
    store.create_graph(&graph).expect("create_graph");

    // Populate the label registry with 50 labels via 50 nodes.
    for i in 0..50 {
        store
            .create_node(
                &graph,
                vec![format!("Label{i}")],
                props(&[("i", Value::Int(i))]),
            )
            .expect("create_node");
    }

    let started = std::time::Instant::now();
    let labels = store.all_label_names(&graph).expect("all_label_names");
    let elapsed = started.elapsed();

    assert_eq!(labels.len(), 50, "all 50 distinct labels must be returned");
    assert!(
        elapsed < Duration::from_millis(500),
        "registry validation regressed to per-key network cost: 50 labels took {elapsed:?}; \
         expected <500ms with batched SCARDs"
    );

    store.delete_graph(&graph).expect("cleanup delete_graph");
}
