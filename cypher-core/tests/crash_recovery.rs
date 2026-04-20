//! Crash-recovery / WAL-replay determinism tests for `PersistentGraphStore`.
//!
//! These tests pin down the recovery contract for the WAL-backed in-memory
//! persistence path. Recovery is exercised by:
//!   1. Opening a `PersistentGraphStore` and writing committed entries.
//!   2. Dropping the store (simulates clean shutdown — flushes the WAL).
//!   3. Optionally mutating the on-disk WAL (simulating a torn write).
//!   4. Re-opening and asserting the recovered state.
//!
//! See `docs/recovery_contract.md` for the contract these tests enforce.

use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::Duration;

use cypher_core::storage::{GraphStore, NodeId, PersistentConfig, PersistentGraphStore, SyncMode};
use cypher_core::types::Value;
use indexmap::IndexMap;

// ── Helpers ──────────────────────────────────────────────────────────────────

fn cfg(dir: &Path) -> PersistentConfig {
    PersistentConfig {
        data_dir: dir.to_path_buf(),
        sync_mode: SyncMode::Immediate,
        // Long interval so the background snapshot thread does not fire
        // during the test window — keeps the WAL deterministic.
        snapshot_interval: Duration::from_secs(3600),
    }
}

fn props(name: &str, idx: i64) -> IndexMap<String, Value> {
    let mut p = IndexMap::new();
    p.insert("name".to_string(), Value::Str(name.into()));
    p.insert("idx".to_string(), Value::Int(idx));
    p
}

fn write_n(store: &PersistentGraphStore, graph: &str, n: i64) -> Vec<NodeId> {
    let mut ids = Vec::with_capacity(n as usize);
    for i in 0..n {
        let nid = store
            .create_node(graph, vec!["Item".to_string()], props(&format!("n{i}"), i))
            .expect("create_node");
        ids.push(nid);
    }
    ids
}

fn assert_n_present(store: &PersistentGraphStore, graph: &str, ids: &[NodeId]) {
    for (i, nid) in ids.iter().enumerate() {
        let node = store
            .get_node(graph, *nid)
            .expect("get_node")
            .unwrap_or_else(|| panic!("missing node {nid:?}"));
        assert_eq!(node.labels, vec!["Item".to_string()]);
        assert_eq!(node.properties["idx"], Value::Int(i as i64));
        assert_eq!(
            node.properties["name"],
            Value::Str(format!("n{i}").into_boxed_str())
        );
    }
    let stats = store.graph_stats(graph).expect("graph_stats");
    assert_eq!(stats.node_count as usize, ids.len());
}

// ── 1. Happy path: write N, drop, reopen, all present ────────────────────────

#[test]
fn recovery_happy_path_all_committed_entries_present() {
    let dir = tempfile::tempdir().unwrap();
    const N: i64 = 50;

    let ids = {
        let store = PersistentGraphStore::open(cfg(dir.path())).unwrap();
        store.create_graph("g").unwrap();
        write_n(&store, "g", N)
        // store dropped here → WAL flushed
    };

    let store = PersistentGraphStore::open(cfg(dir.path())).unwrap();
    assert_n_present(&store, "g", &ids);
}

// ── 2. Empty WAL: open with no committed entries succeeds ────────────────────

#[test]
fn recovery_empty_wal_opens_cleanly() {
    let dir = tempfile::tempdir().unwrap();

    // First open: writes nothing, then drops. WAL file exists but is empty.
    {
        let _store = PersistentGraphStore::open(cfg(dir.path())).unwrap();
    }
    // Sanity: WAL exists and is empty.
    let wal_path = dir.path().join("wal.jsonl");
    assert!(wal_path.exists(), "WAL should be created on first open");
    assert_eq!(
        fs::metadata(&wal_path).unwrap().len(),
        0,
        "WAL should be empty when no writes occurred"
    );

    // Reopen: should recover an empty graph state.
    let store = PersistentGraphStore::open(cfg(dir.path())).unwrap();
    assert_eq!(store.list_graphs().unwrap(), Vec::<String>::new());
}

// Edge case: WAL file does not exist at all (first-ever open).
#[test]
fn recovery_no_wal_file_opens_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!dir.path().join("wal.jsonl").exists());
    let store = PersistentGraphStore::open(cfg(dir.path())).unwrap();
    assert_eq!(store.list_graphs().unwrap(), Vec::<String>::new());
}

// ── 3. Torn tail: WAL truncated mid-record must not panic ────────────────────

/// Truncate the WAL file to `keep_bytes` bytes. Simulates a crash that left
/// a partially-written final record on disk.
fn truncate_wal_to(dir: &Path, keep_bytes: u64) {
    let wal_path = dir.join("wal.jsonl");
    let mut f = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&wal_path)
        .expect("open wal");
    f.seek(SeekFrom::Start(0)).unwrap();
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).unwrap();
    assert!(
        keep_bytes < buf.len() as u64,
        "test bug: nothing to truncate (have {} bytes, asked to keep {})",
        buf.len(),
        keep_bytes
    );
    f.set_len(keep_bytes).expect("set_len");
}

#[test]
fn recovery_torn_tail_does_not_panic_and_preserves_full_records() {
    let dir = tempfile::tempdir().unwrap();
    const N: i64 = 10;

    // Phase 1: persist N committed entries.
    {
        let store = PersistentGraphStore::open(cfg(dir.path())).unwrap();
        store.create_graph("g").unwrap();
        write_n(&store, "g", N);
    }

    // Inspect the WAL to find the byte offset just after the (N-1)th newline,
    // i.e. the start of what would be the next record's bytes if any.
    let wal_path = dir.path().join("wal.jsonl");
    let contents = fs::read(&wal_path).unwrap();
    let total_len = contents.len() as u64;
    assert!(total_len > 10, "WAL should have content");

    // Locate the offset just past the last newline → fully-committed prefix.
    let last_nl = contents
        .iter()
        .rposition(|&b| b == b'\n')
        .expect("WAL has at least one newline") as u64;
    let prefix_end = last_nl + 1;

    // Append a partial record that is missing its trailing newline AND is
    // not valid JSON when parsed alone — this simulates a process killed
    // mid-write.
    {
        let mut f = fs::OpenOptions::new().append(true).open(&wal_path).unwrap();
        f.write_all(b"{\"op\":\"CreateNode\",\"graph\":\"g\",\"labels\":[\"X\"")
            .unwrap();
        f.sync_all().unwrap();
    }
    let torn_len = fs::metadata(&wal_path).unwrap().len();
    assert!(torn_len > prefix_end, "torn tail should extend the WAL");

    // Phase 2: re-open. The store MUST tolerate the torn tail:
    //   * `open()` succeeds.
    //   * All N fully-committed records are visible.
    //   * The on-disk WAL has been truncated back to the last clean record
    //     boundary (the prefix end) so subsequent appends start aligned.
    let store = PersistentGraphStore::open(cfg(dir.path())).expect("open must tolerate torn tail");
    let stats = store.graph_stats("g").unwrap();
    assert_eq!(
        stats.node_count, N as u64,
        "torn-tail recovery exposed wrong node count"
    );
    for i in 0..N {
        let n = store
            .get_node("g", NodeId(i as u64 + 1))
            .unwrap()
            .expect("committed node missing");
        assert_eq!(n.properties["idx"], Value::Int(i));
    }
    drop(store);

    let on_disk_len = fs::metadata(&wal_path).unwrap().len();
    assert_eq!(
        on_disk_len, prefix_end,
        "torn-tail open must truncate WAL to last clean record boundary \
         (expected {prefix_end}, got {on_disk_len})"
    );
}

/// A complete, newline-terminated WAL line that contains invalid JSON is real
/// corruption (not a torn write) and `open()` MUST surface it as an error
/// rather than silently dropping the record.
#[test]
fn recovery_complete_but_corrupt_line_returns_error() {
    let dir = tempfile::tempdir().unwrap();

    // Phase 1: commit a few entries.
    {
        let store = PersistentGraphStore::open(cfg(dir.path())).unwrap();
        store.create_graph("g").unwrap();
        write_n(&store, "g", 3);
    }

    // Append a line that is fully terminated (\n) but is invalid JSON. This
    // is NOT a torn write — it's a fully-committed corrupt record.
    let wal_path = dir.path().join("wal.jsonl");
    {
        let mut f = fs::OpenOptions::new().append(true).open(&wal_path).unwrap();
        f.write_all(b"{\"op\":\"CreateNode\",\"graph\":\"g\",\"labels\":[\"X\"\n")
            .unwrap();
        f.sync_all().unwrap();
    }

    let result = PersistentGraphStore::open(cfg(dir.path()));
    let err = result.expect_err("open must reject newline-terminated corrupt lines");
    let msg = format!("{err}").to_lowercase();
    assert!(
        msg.contains("parse") || msg.contains("serial") || msg.contains("wal"),
        "expected WAL/serialization error, got: {err}"
    );
}

#[test]
fn recovery_truncate_to_record_boundary_yields_prefix() {
    let dir = tempfile::tempdir().unwrap();
    const N: i64 = 6;

    {
        let store = PersistentGraphStore::open(cfg(dir.path())).unwrap();
        store.create_graph("g").unwrap();
        write_n(&store, "g", N);
    }

    // Truncate to the byte offset right after the 3rd newline. This mimics
    // a crash where exactly 1 (CreateGraph) + 3 (CreateNode) records are
    // durable, and any later writes were lost.
    let wal_path = dir.path().join("wal.jsonl");
    let bytes = fs::read(&wal_path).unwrap();
    let mut nl_offsets: Vec<usize> = bytes
        .iter()
        .enumerate()
        .filter(|(_, &b)| b == b'\n')
        .map(|(i, _)| i)
        .collect();
    assert!(nl_offsets.len() >= 4);
    let keep = (nl_offsets.remove(3) + 1) as u64;
    truncate_wal_to(dir.path(), keep);

    // Must not panic. Result may be Ok (with prefix state) or Err.
    let result = PersistentGraphStore::open(cfg(dir.path()));
    if let Ok(store) = result {
        let stats = store.graph_stats("g").unwrap();
        // First 3 CreateNode entries land as IDs 1..=3.
        assert_eq!(stats.node_count, 3);
        for i in 0..3 {
            let n = store.get_node("g", NodeId(i as u64 + 1)).unwrap().unwrap();
            assert_eq!(n.properties["idx"], Value::Int(i));
        }
        // Nodes that were never durable must be absent.
        assert!(store.get_node("g", NodeId(4)).unwrap().is_none());
    }
    // If Err, the test still passes — we only forbid panic and data
    // destruction, both of which are implicitly checked.
}

// ── 4. Duplicate replay: replaying the same WAL N times is idempotent ───────
//
// Two flavors of the "same WAL replayed multiple times" guarantee (#1701):
//
//  (a) `recovery_same_wal_replayed_n_times_is_stable` — in-place replay: we
//      take an already-recovered `PersistentGraphStore` and call its public
//      `replay_wal_from` API N times against its own on-disk WAL. The final
//      observable state must equal the state after a single recovery.
//
//  (b) `recovery_duplicate_replay_two_stores_produce_identical_state` — two
//      independent stores opened from byte-identical on-disk WALs expose
//      identical content.

#[test]
fn recovery_same_wal_replayed_n_times_is_stable() {
    let dir = tempfile::tempdir().unwrap();

    // Build a WAL with a representative mix of operations (creates, mutates,
    // delete, edge, index). Then drop to flush.
    {
        let store = PersistentGraphStore::open(cfg(dir.path())).unwrap();
        store.create_graph("g").unwrap();
        let ids = write_n(&store, "g", 6);
        let mut update = IndexMap::new();
        update.insert("touched".into(), Value::Bool(true));
        store.set_node_properties("g", ids[0], update).unwrap();
        store
            .add_node_labels("g", ids[1], &["Special".to_string()])
            .unwrap();
        store.delete_node("g", ids[2], true).unwrap();
        store
            .create_edge("g", ids[3], ids[4], "REL".to_string(), IndexMap::new())
            .unwrap();
        store.create_index("g", "Item", "idx").unwrap();
    }

    // Baseline: open fresh, capture observable state after a single replay.
    let store = PersistentGraphStore::open(cfg(dir.path())).unwrap();
    let wal_path = dir.path().join("wal.jsonl");

    let baseline_stats = store.graph_stats("g").unwrap();
    let baseline_graphs = store.list_graphs().unwrap();
    let baseline_indexes = store.list_indexes("g").unwrap();
    let baseline_node_ids: Vec<NodeId> = (1..=6)
        .map(NodeId)
        .filter(|id| store.get_node("g", *id).unwrap().is_some())
        .collect();
    let baseline_node_snapshots: Vec<_> = baseline_node_ids
        .iter()
        .map(|id| store.get_node("g", *id).unwrap().unwrap())
        .collect();

    // Re-apply the same WAL N more times. The new API surface (#1701)
    // guarantees this converges to the baseline state rather than panicking
    // or duplicating work.
    for cycle in 0..4 {
        store
            .replay_wal_from(&wal_path)
            .unwrap_or_else(|e| panic!("in-place replay failed at cycle {cycle}: {e}"));

        let stats = store.graph_stats("g").unwrap();
        assert_eq!(
            stats.node_count,
            baseline_stats.node_count,
            "node_count drifted after {} extra replay(s)",
            cycle + 1
        );
        assert_eq!(
            stats.edge_count,
            baseline_stats.edge_count,
            "edge_count drifted after {} extra replay(s)",
            cycle + 1
        );
        assert_eq!(store.list_graphs().unwrap(), baseline_graphs);
        assert_eq!(store.list_indexes("g").unwrap(), baseline_indexes);

        for (id, expected) in baseline_node_ids.iter().zip(&baseline_node_snapshots) {
            let actual = store
                .get_node("g", *id)
                .unwrap()
                .unwrap_or_else(|| panic!("node {id:?} vanished at cycle {cycle}"));
            assert_eq!(actual.labels, expected.labels);
            assert_eq!(actual.properties, expected.properties);
        }

        // Deleted node stays deleted across replays.
        assert!(
            store.get_node("g", NodeId(3)).unwrap().is_none(),
            "deleted node re-appeared at cycle {cycle}"
        );
    }
}

#[test]
fn recovery_duplicate_replay_two_stores_produce_identical_state() {
    let dir_a = tempfile::tempdir().unwrap();
    let dir_b = tempfile::tempdir().unwrap();
    const N: i64 = 8;

    // Build a WAL with a representative mix of operations.
    {
        let store = PersistentGraphStore::open(cfg(dir_a.path())).unwrap();
        store.create_graph("g").unwrap();
        let ids = write_n(&store, "g", N);
        let mut update = IndexMap::new();
        update.insert("touched".into(), Value::Bool(true));
        store.set_node_properties("g", ids[0], update).unwrap();
        store
            .add_node_labels("g", ids[1], &["Special".to_string()])
            .unwrap();
        store.delete_node("g", ids[2], true).unwrap();
        store
            .create_edge("g", ids[3], ids[4], "REL".to_string(), IndexMap::new())
            .unwrap();
        store.create_index("g", "Item", "idx").unwrap();
    }

    // Copy the WAL (and snapshot, if any) to a fresh directory and re-open
    // both. This exercises the "open a SECOND store from the same WAL"
    // scenario called out in issue #1701.
    fs::copy(
        dir_a.path().join("wal.jsonl"),
        dir_b.path().join("wal.jsonl"),
    )
    .unwrap();
    let snap_a = dir_a.path().join("snapshot.json");
    if snap_a.exists() {
        fs::copy(&snap_a, dir_b.path().join("snapshot.json")).unwrap();
    }

    let store_a = PersistentGraphStore::open(cfg(dir_a.path())).unwrap();
    let store_b = PersistentGraphStore::open(cfg(dir_b.path())).unwrap();

    // Compare observable state across both replays.
    let stats_a = store_a.graph_stats("g").unwrap();
    let stats_b = store_b.graph_stats("g").unwrap();
    assert_eq!(stats_a.node_count, stats_b.node_count);
    assert_eq!(stats_a.edge_count, stats_b.edge_count);
    assert_eq!(
        store_a.list_graphs().unwrap(),
        store_b.list_graphs().unwrap()
    );
    assert_eq!(
        store_a.list_indexes("g").unwrap(),
        store_b.list_indexes("g").unwrap()
    );

    // Node-level projection must be byte-identical.
    for id in 1..=(N as u64) {
        let nid = NodeId(id);
        assert_eq!(
            store_a.get_node("g", nid).unwrap(),
            store_b.get_node("g", nid).unwrap(),
            "node {id} diverged across duplicate-replay",
        );
    }

    // Spot-check the mutated node.
    let n1_a = store_a.get_node("g", NodeId(1)).unwrap().unwrap();
    assert_eq!(n1_a.properties["touched"], Value::Bool(true));

    // Deleted node must be absent in both.
    assert!(store_a.get_node("g", NodeId(3)).unwrap().is_none());
    assert!(store_b.get_node("g", NodeId(3)).unwrap().is_none());
}

// ── 5. Reopen-loop: repeated open/close cycles converge on the same state ────
//
// This is a stronger form of duplicate replay: we open, close, and reopen
// the SAME on-disk store multiple times. Each cycle executes the recovery
// codepath against the previously-persisted artifacts. The observable state
// must be identical after every cycle.

#[test]
fn recovery_repeated_open_close_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    const N: i64 = 12;

    let ids = {
        let store = PersistentGraphStore::open(cfg(dir.path())).unwrap();
        store.create_graph("g").unwrap();
        write_n(&store, "g", N)
    };

    for cycle in 0..3 {
        let store = PersistentGraphStore::open(cfg(dir.path())).unwrap();
        assert_n_present(&store, "g", &ids);
        // Verify list_graphs is stable across cycles.
        assert_eq!(
            store.list_graphs().unwrap(),
            vec!["g".to_string()],
            "graphs differ at cycle {cycle}"
        );
    }
}
