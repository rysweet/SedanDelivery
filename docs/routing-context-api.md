---
layout: default
title: Read Consistency API Reference
---

# Read Consistency API Reference

This page documents the current API boundary for read-after-write routing.

SedanDelivery no longer uses a thread-local routing context. The contract is now
explicit:

- Bolt sessions compute `ReadConsistency`
- `GraphStore::with_read_consistency(...)` carries that policy into the storage
  layer
- `RedisGraphStore` implements the policy when choosing between primary and
  replica connections

## Core types

### `ReadConsistency`

**Location:** `gql-core/src/storage/mod.rs`

```rust
pub enum ReadConsistency {
    TopologyDefault,
    SessionFreshPrimary,
}
```

- `TopologyDefault` means "use the backend's normal routing policy"
- `SessionFreshPrimary` means "this session recently wrote; start reads on the
  primary"

### `GraphStore::with_read_consistency(...)`

**Location:** `gql-core/src/storage/mod.rs`

```rust
fn with_read_consistency(
    &self,
    consistency: ReadConsistency,
) -> Option<Arc<dyn GraphStore>>
```

This hook is optional by design.

- backends that care about routing can return a cloned store view with the
  requested consistency attached
- backends that do not care return `None`

That keeps the executor surface area stable while still letting topology-aware
stores honor session freshness.

## Bolt-side API

### `READ_AFTER_WRITE_WINDOW`

**Location:** `sedandelivery-bolt/src/protocol/handler/session.rs`

```rust
pub(crate) const READ_AFTER_WRITE_WINDOW: Duration = Duration::from_secs(5);
```

This is the fixed freshness window used to decide whether a session should keep
reading from the primary.

### `Session::read_consistency()`

**Location:** `sedandelivery-bolt/src/protocol/handler/session.rs`

```rust
pub(crate) fn read_consistency(&self) -> ReadConsistency
```

It maps `last_write_at` to one of the two consistency modes:

- recent write -> `SessionFreshPrimary`
- no recent write -> `TopologyDefault`

When a client sends a recent **write-backed** bookmark on a fresh Bolt
connection, the handler rehydrates `last_write_at` before routing so the same
causal session still starts on the primary.

### `Session::record_successful_write()`

Tracks successful writes for the current session.

- outside explicit transactions, it refreshes `last_write_at` immediately
- inside explicit transactions, it marks `tx_had_write = true`
- if the backend has no transaction snapshot, it also refreshes immediately so
  same-session reads inside the transaction stay correct

### `Session::finish_explicit_transaction(committed: bool)`

Finalizes transaction-local write tracking.

- on `COMMIT`, refreshes `last_write_at` if the transaction wrote anything
- on `ROLLBACK` or `RESET`, clears the transaction-local write flag without
  inventing a new freshness window

### Bookmark timestamp tracking

**Location:** `sedandelivery-bolt/src/bookmark.rs`

`BookmarkCounter` now records a short-lived timestamp for each emitted
write-backed bookmark. `RUN` and `BEGIN` use those timestamps to restore
session freshness when the client presents a bookmark that is still inside
`READ_AFTER_WRITE_WINDOW`.

Read-only bookmarks are still accepted and forwarded by drivers, but they do
not count as evidence of a recent write and therefore do not rehydrate
`SessionFreshPrimary`.

Likewise, standalone `USE <database>` updates the session's active graph but is
not treated as a write-backed bookmark for fresh-session routing.

The timestamp cache is best-effort: SedanDelivery keeps it locally and mirrors
it into short-lived Redis keys when Redis is available. If that timestamp
metadata is unavailable on the receiving Bolt process, routing falls back to
`TopologyDefault` even though the bookmark itself is still syntactically valid.

## Store-side API

### `RedisGraphStore::with_read_consistency_mode(...)`

**Location:** `gql-redis-store/src/lib.rs`

Returns a cloned `RedisGraphStore` with the requested `ReadConsistency`.

### `RedisGraphStore::route_read()`

Uses the stored consistency mode to choose the first pool:

- `SessionFreshPrimary` -> primary pool
- `TopologyDefault` -> replica pool when configured, else primary

### `RedisGraphStore::route_read_for_graph()`

Performs graph-scoped reads with one extra safety rule.

If a request started on a replica and that replica falsely reports
`GraphNotFound`, the store retries once on the primary before surfacing the
error.

That retry is **only** for replica-started reads. A session already using
`SessionFreshPrimary` starts on the primary and does not take the extra retry
path.

## End-to-end handoff

The core handoff in `sedandelivery-bolt/src/protocol/handler/run.rs` is:

```rust
fn apply_read_consistency(session: &Session, store: Arc<dyn GraphStore>) -> Arc<dyn GraphStore> {
    store
        .with_read_consistency(session.read_consistency())
        .unwrap_or(store)
}
```

This is the current cross-layer contract for issue `#737`.

## Validation references

The following tests and QA artifacts exercise the contract:

- `sedandelivery-bolt/src/protocol/handler/session.rs`
- `sedandelivery-bolt/src/protocol/handler/run.rs`
- `sedandelivery-bolt/src/protocol/handler/mod.rs`
- `scripts/test-replica-read-after-write.sh`
- `tests/qa/gadugi/issue-737-read-after-write-primary-pinning.yaml`
