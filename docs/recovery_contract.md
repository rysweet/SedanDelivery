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

## Known gaps (follow-ups)

These are documented as gaps rather than fixed in this PR to keep scope
tight (no new public API). Tests are written to accept either the current
or the desired behavior, so a future fix will not require test changes.

1. **Tolerant torn-tail recovery.** Today, `replay_file` returns
   `StorageError::Serialization` for a malformed final line. This is safe
   (no panic, on-disk bytes intact) but causes `open()` to fail. A
   tolerant implementation should detect the partial-record condition,
   stop replay at the last good newline boundary, log a warning, and
   surface success with the prefix state as the live state. See
   `cypher-core/src/storage/persistent/replay.rs`.

2. **Snapshot replay ID-sequence coupling.** `replay_entry` for
   `CreateNode`/`CreateEdge` requires the auto-allocated ID to equal the
   persisted ID. This works for a single replay but means a WAL cannot be
   safely replayed twice into the same store (no idempotency at the
   in-place-mutation level). The duplicate-replay test deliberately uses
   two independent fresh stores to characterize this.

3. **WAL framing.** Records are line-delimited JSON. There is no per-record
   length prefix or checksum, so a torn-tail detector must rely on
   "newline-terminated" as its only synchronization point. A length-prefixed
   or CRC-checked framing would make torn-tail detection unambiguous.

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
