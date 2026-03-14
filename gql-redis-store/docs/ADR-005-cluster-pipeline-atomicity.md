# ADR-005: Redis Cluster Routing Safety for Pipelines and Metadata

**Status:** Accepted
**Date:** 2026-03-13
**Issue:** #745

## Context

`#745` was exposed through the real Bolt path, not just through unit tests.
The original hypothesis ("remove `.atomic()`") was directionally useful but
incomplete.

The validated root cause had two separate pieces:

1. `ClusterConnection` cannot safely execute generic `redis::Pipeline::query()`
   batches; cluster-mode batching must go through `cluster_pipe()`.
2. Several planner and DDL code paths still used cluster-unsafe keyspace
   `SCAN`, notably metadata enumeration and cleanup.

That combination produced the real runtime failure:

```text
This command cannot be safely routed in cluster mode - Client
```

## Decision

Adopt a routing-safe cluster design with five rules:

### 1. Execute pipelines through a connection-aware abstraction

`RedisConn::pipeline()` now returns a `RedisPipeline` wrapper that dispatches to:

- `redis::pipe()` in standalone mode
- `redis::cluster::cluster_pipe()` in cluster mode

Call sites keep `RedisConn` concrete until execution so the dispatch decision is
made at the correct layer.

### 2. Replace metadata / cleanup `SCAN` paths with registries and cleanup manifests

Maintain:

- `G:{g}:_labels`
- `G:{g}:_reltypes`
- `G:{g}:_propidx`

These registries allow cluster-safe implementations of:

- `all_label_names`
- `graph_stats`
- `drop_index`
- `delete_graph`

Callers validate `_labels` / `_reltypes` members against the real backing sets
before using them, so stale entries do not become false planner metadata.
`_propidx` is a cleanup manifest so `delete_graph` can remove orphaned propidx
sets after partial failures.

### 3. Make graph deletion retryable without allowing name reuse over stale keys

`delete_graph` now uses a temporary per-graph marker:

- `G:{g}:_deleting`

Cleanup sets `_deleting`, removes per-graph keys, removes the graph from the
global registry, and only then clears `_deleting`. Reads therefore treat a
retryable delete as already gone, but `delete_graph` itself can be retried and
`create_graph(g)` stays blocked until cleanup is truly finished.

### 4. Renew long-running write locks and fail closed if ownership is lost

The per-graph `G:{g}:write_lock` remains a CAS lock, but long-running write
paths now renew the TTL while they process large scans, cleanup loops, or batch
pipelines. A mutation only reports success if the final CAS release proves it
still owned the lock. If the lease was lost, callers receive
`TransactionConflict` rather than a false success.

### 5. Keep the graph hash-tag invariant

All per-graph keys remain `G:{g}:...`. That invariant is required for slot-local
pipeline routing and for registry-driven reconstruction of graph-local keys.

## Consequences

### Positive

- cluster write/read/index paths now execute through the correct pipeline API
- planner metadata and DDL cleanup no longer depend on global keyspace scans
- real Bolt traffic succeeds in cluster mode
- resharding validation passes without backend errors
- `delete_graph` is retryable after partial failures without allowing unsafe
  graph-name reuse
- long-running same-graph writes do not silently outlive their lock lease

### Negative / Residual risks

#### Partial write on connection failure

Slot-local pipelining is still not a transaction. A dropped connection can
leave partial state.

#### Registry staleness

`_labels` and `_reltypes` may retain empty members until filtered against the
backing sets. This is acceptable because the readers do that validation before
using them.

#### Non-atomic writes still need manifests

Slot-local pipelines are still not transactions. That is why manifest-first
node/edge writes and the `_propidx` cleanup manifest are part of the design:
they let `delete_graph` clean up partial write/index failures without falling
back to cluster-unsafe scans.

#### Graph registry remains separate

`__sd2_registry:graphs` stays in its own slot. That is safe because it is never
co-pipelined with per-graph commands.

## Alternatives Considered

### Alternative A: Only remove `.atomic()`

Rejected because it did not fix the planner/Bolt path once the first store-level
pipeline failures were gone.

### Alternative B: Keep cluster-vs-standalone branching at each call site

Rejected in favor of centralizing the decision in `RedisConn::pipeline()`.

### Alternative C: Replace complex writes with Lua

Rejected as overkill for a bug fix. It would add complexity without removing
the need to redesign metadata enumeration.

### Alternative D: Keep using `SCAN` with special handling

Rejected because registry-based metadata is explicit, slot-local, and easier to
reason about than ad hoc cluster `SCAN` workarounds.

## Verification

Validated with:

```bash
cargo test -p gql-redis-store
```

```bash
GQL_REDIS_CLUSTER_NODES="redis://127.0.0.1:7100,redis://127.0.0.1:7101,redis://127.0.0.1:7102" \
  cargo test -p gql-redis-store --test cluster_pipeline -- --ignored --nocapture
```

```bash
VALKEY_PASSWORD='change-me' BOLT_PASSWORD='change-me' \
  ./scripts/test-resharding-resilience.sh
```
