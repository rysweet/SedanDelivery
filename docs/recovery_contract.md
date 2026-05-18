# Recovery Contract — `PersistentGraphStore`

This document pins down the crash-recovery / WAL-replay contract enforced by
`cypher-core/tests/crash_recovery.rs`. It describes both the **guarantees**
the engine provides today and the **gaps** that are open follow-ups.

## Architecture (current)

`PersistentGraphStore` wraps `InMemoryGraphStore` with disk persistence
backed by two files in `data_dir`:

| File             | Purpose                                                  |
|------------------|----------------------------------------------------------|
| `snapshot.json`  | Full point-in-time dump (JSON-line `WalEntry`s).         |
| `wal.jsonl`      | Append-only write-ahead log (one JSON `WalEntry` / line).|

Recovery (`PersistentGraphStore::open`) is:

1. If `snapshot.json` exists, replay it into a fresh `InMemoryGraphStore`.
2. If `wal.jsonl` exists, replay it on top.
3. Open the WAL for append.
4. Spawn the background snapshot/truncate thread.

Snapshots are written atomically (temp file + rename). The WAL is truncated
after a successful snapshot.

## Guarantees pinned by tests

| Guarantee | Test |
|-----------|------|
| **Happy path.** Every committed entry is observable after a clean drop+reopen. | `recovery_happy_path_all_committed_entries_present` |
| **Empty WAL.** A zero-byte WAL replays cleanly and yields an empty graph state. | `recovery_empty_wal_opens_cleanly` |
| **No WAL file.** First-ever open with no `wal.jsonl` succeeds. | `recovery_no_wal_file_opens_cleanly` |
| **Torn tail (no panic).** A WAL ending with an incomplete final record never panics. The committed prefix on disk is preserved (not destroyed by the open call). | `recovery_torn_tail_does_not_panic_and_preserves_full_records` |
| **Record-boundary truncation.** A WAL truncated exactly at a record boundary recovers cleanly with the persisted prefix as the live state. | `recovery_truncate_to_record_boundary_yields_prefix` |
| **Replay determinism.** The same WAL replayed into two independent stores yields identical observable state. | `recovery_duplicate_replay_is_deterministic` |
| **Open-loop idempotence.** Repeated open/close cycles against the same on-disk store always produce the same observable state. | `recovery_repeated_open_close_is_idempotent` |

## Previously-known gaps

The following gaps were identified during design review. Items 1–3 are
resolved. Items 4–7 are addressed by the changes in this PR (issue #1201).

1. **Tolerant torn-tail recovery.** ✅ Fixed. `replay_wal_file` detects
   unterminated trailing bytes, truncates to the last clean record boundary,
   logs a `tracing::warn!`, and replays the committed prefix. Snapshot files
   use the strict `replay_file` path (a partial snapshot is real corruption).

2. **Snapshot replay ID-sequence coupling.** ✅ Fixed. `replay_entry` for
   `CreateNode`/`CreateEdge` now uses `create_node_with_id` /
   `create_edge_with_id` which bypass the auto-allocator and are idempotent:
   replaying the same WAL N times into the same store converges on the same
   state (issue #1701).

3. **WAL framing.** ✅ Fixed. Each WAL line is prefixed with `CRC:<hex8> `
   encoding a CRC32C (Castagnoli) checksum (issue #1702). Legacy lines
   without the prefix are accepted with a one-time warning.

4. **WAL written after mutation.** 🔧 In progress (this PR). All persistent
   store write methods will follow WAL-first ordering: the WAL entry is
   appended *before* the in-memory mutation is applied. Create operations
   use `allocate_node_id()` / `allocate_edge_id()` to obtain the ID ahead
   of the WAL write. See [storage-correctness.md](storage-correctness.md) §1.

5. **Constraints not persisted.** 🔧 In progress (this PR).
   `CreateConstraint` and `DropConstraint` WAL entries will be written for
   constraint mutations. Constraints will be included in snapshots. Replay
   is idempotent. See [storage-correctness.md](storage-correctness.md) §2.

6. **Snapshot reads not atomic.** 🔧 In progress (this PR).
   `write_snapshot()` will call `inner.read_snapshot()` once (a cheap `Arc`
   clone) and iterate the frozen `GraphInner` fields directly. No
   `GraphStore` trait methods will be called during serialization,
   eliminating the window for concurrent writers to produce a partial
   snapshot. See [storage-correctness.md](storage-correctness.md) §3.

7. **Node/edge overwrite without collision check.** 🔧 In progress (this PR).
   `create_node` / `create_edge` with an explicit `_sd2_id` / `_sd2_eid`
   that collides with an existing entity will return
   `StorageError::Backend`. The `_with_id` variants remain idempotent for
   WAL replay. See [storage-correctness.md](storage-correctness.md) §4.

## How crashes are simulated

`tests/crash_recovery.rs` simulates crashes purely by manipulating files
on disk after dropping the store; no `unsafe` and no process killing is
required. The "crash" is modeled as:

* Drop the `PersistentGraphStore` (mimics a clean shutdown — flushes WAL).
* Optionally append partial bytes to `wal.jsonl` or `set_len` it to a
  non-record boundary (mimics a process killed mid-`write`/`fsync`).
* Re-open and assert the recovery contract above.

This keeps the tests deterministic across platforms while exercising the
exact bytes the recovery codepath would see after a real crash.
