---
layout: default
title: Read-After-Write Routing
---

# Read-After-Write Routing

SedanDelivery keeps a **5-second read-after-write freshness window** per Bolt
session. When a session has written recently, subsequent reads from that same
session are routed to the primary Redis node instead of a replica.

This prevents the most important replica-lag failure mode: a client writes data
successfully and then immediately fails to read it back because a replica has
not caught up yet.

## Why this exists

Redis replication is asynchronous. A write that has committed on the primary can
still be invisible on replicas for a short period.

Without a freshness policy:

```text
t=0  CREATE (:Order {id: "ord-99"})   -> primary
t=1  MATCH (:Order {id: "ord-99"})    -> replica
      => 0 rows
```

That is a correctness bug for workloads that expect read-your-writes behavior.

## How the contract works

### 1. Bolt sessions track freshness

Each Bolt session stores `last_write_at: Option<Instant>` and derives a routing
policy from it:

- `ReadConsistency::SessionFreshPrimary` when the last write is younger than
  `READ_AFTER_WRITE_WINDOW`
- `ReadConsistency::TopologyDefault` otherwise

The current implementation uses a fixed constant:

```rust
/// The read-after-write freshness window.
///
/// Reads within this duration of a write are routed to the primary instead of
/// a replica. Exported so benchmarks and external routing-contract tests can
/// reference the authoritative value directly.
pub const READ_AFTER_WRITE_WINDOW: Duration = Duration::from_secs(5);
```

The constant is exported at
`sedandelivery_bolt::protocol::handler::READ_AFTER_WRITE_WINDOW`.

This window is **not operator-configurable today**.

When a Neo4j driver reconnects and forwards a recent **write-backed** bookmark,
SedanDelivery can also rehydrate `last_write_at` from that bookmark so the
primary pin survives connection reuse instead of only same-socket reuse.

### 2. Bolt applies the policy at the storage boundary

`sedandelivery-bolt` does not thread a new routing flag through every executor
call. Instead it resolves a session-scoped store view before query execution:

```rust
store
    .with_read_consistency(session.read_consistency())
    .unwrap_or(store)
```

The convenience predicate `Session::force_primary_read()` wraps this check and
returns `true` when reads should be pinned to the primary. The query dispatch
path in `run.rs` uses it for debug logging:

```rust
if session.force_primary_read() {
    tracing::debug!(graph = %graph, "routing reads to primary …");
}
```

Backends that care about topology can return a specialized store view. Backends
that do not care simply return `None` and keep their original behavior.

### 3. RedisGraphStore owns topology routing

`RedisGraphStore` carries the requested `ReadConsistency` in the cloned store
view:

- `route_read()` starts on the primary for `SessionFreshPrimary`
- `route_read()` prefers replicas for `TopologyDefault`
- `route_read_for_graph()` still retries once on the primary after a replica
  returns a false `GraphNotFound`
- `route_read_for_graph()` does **not** take that retry path when the request
  already started on the primary

This keeps the responsibility split clean:

- **Bolt owns session freshness**
- **The store owns topology routing**

## Explicit transaction semantics

Explicit transactions need one extra rule: the pin should refresh when a write
becomes durably visible, but same-session reads inside the transaction must also
stay correct.

SedanDelivery handles this in two cases:

### Backends with transaction snapshots

Backends like `InMemoryGraphStore` can isolate writes in a transaction snapshot.

- successful write `RUN`s mark `tx_had_write = true`
- `last_write_at` is refreshed on `COMMIT`
- reads inside the transaction use the snapshot, so they still see their own
  writes before commit

### Backends without transaction snapshots

`RedisGraphStore` currently uses the default no-op transaction snapshot hooks,
so successful write `RUN`s become visible immediately.

- successful write `RUN`s refresh `last_write_at` immediately
- `COMMIT` refreshes the timestamp again if the transaction wrote anything
- this preserves same-session read-your-writes behavior both **inside** the
  transaction and **immediately after commit**

`ROLLBACK` and `RESET` clear the transaction-local `tx_had_write` flag, but
they do **not** blindly erase `last_write_at`, because some backends may already
have made the write visible.

## Operational boundaries

### Bookmark-assisted across fresh connections

The primary pin is still driven by session freshness, but a fresh Bolt
connection can restore that freshness if the client supplies a recent
**write-backed** bookmark. Unrelated sessions without bookmarks do not inherit
the pin.

Read-only bookmarks remain valid for causal chaining, but they do **not**
create a new read-after-write window on a fresh session.

Standalone `USE <database>` is also session-local only: it returns a bookmark
for protocol continuity, but that bookmark is not treated as write-backed
freshness.

Bookmark freshness metadata is kept in-process and, when Redis is available,
mirrored into short-lived Redis keys. Rehydration across fresh connections
works only while that timestamp metadata is still available. If the metadata is
missing (for example after expiry or a Redis-side timestamp write failure),
SedanDelivery falls back to the normal topology-default routing policy even if
the bookmark string itself is still valid.

### Time-bounded, not acknowledgement-based

The 5-second window is a heuristic. If replica lag is longer than 5 seconds, a
session can still fall back to replica routing before replication catches up.

### Replica configuration still matters

When `GQL_REDIS_REPLICA_URL` is unset, all reads already go to the primary and
the policy has no visible effect.

## `READ_AFTER_WRITE_WINDOW` and the routing benchmark

`sedandelivery-bolt/benches/bolt_benchmarks.rs` includes dedicated routing
microbenchmarks:

```rust
fn bench_read_routing_decision(c: &mut Criterion) {
    c.bench_function("read_consistency_recent_write", |b| {
        b.iter(|| Session::read_consistency_for_age(Some(Duration::from_secs(1))))
    });

    c.bench_function("read_consistency_window_expired", |b| {
        b.iter(|| {
            Session::read_consistency_for_age(Some(
                READ_AFTER_WRITE_WINDOW + Duration::from_secs(1),
            ))
        })
    });
}
```

The benchmark uses a fixed `1s` representative age for the inside-window path
and `READ_AFTER_WRITE_WINDOW + 1s` for the expired-window path. That keeps the
expired-path probe compiler-coupled to the exported constant while leaving the
inside-window probe as an obvious, stable representative case.

Run the routing microbench:

```bash
cargo bench -p sedandelivery-bolt --bench bolt_benchmarks -- read_consistency
```

See [benchmarks](benchmarks.md#sedandelivery-boltbenchesbolt_benchmarksrs----routing-decision-cost)
for the full suite description and baseline snapshot.

## How to validate it

Issue `#737` includes an executable outside-in proof:

- validator script: `scripts/test-replica-read-after-write.sh`
- Gadugi scenario:
  `tests/qa/gadugi/issue-737-read-after-write-primary-pinning.yaml`

The validator proves three observable behaviors against a local replica
topology:

1. a same-session read still succeeds after a write when replicas are stopped
2. a fresh session without recent writes does not get that privilege
3. after the 5-second window expires, the original session also returns to the
   default replica-routed behavior

It also exercises the non-default database path and confirms that `CREATE INDEX`
refreshes the same-session pin.
