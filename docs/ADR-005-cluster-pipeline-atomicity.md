# ADR-005: Redis Cluster Routing Safety for Pipelines and Metadata

**Status**: Accepted
**Date**: 2026-03-13
**Issue**: #745

## Context

Issue `#745` exposed a real Redis Cluster correctness bug through the public
Bolt path. The first theory was too narrow: removing `MULTI/EXEC` from a few
write pipelines helped, but it did **not** fully fix cluster mode.

The validated root cause had two parts:

1. **Generic pipeline execution on cluster connections was invalid**

   `redis-rs` 1.0.4 requires cluster-mode batches to execute through
   `redis::cluster::cluster_pipe()`. Generic `redis::Pipeline::query()` is not
   a safe substitute on `ClusterConnection`.

2. **Planner and DDL code still depended on cluster-unsafe keyspace `SCAN`**

   Hot paths such as:

   - `all_label_names`
   - `graph_stats`
   - `drop_index`
   - `delete_graph`

   still used `SCAN`-style enumeration. In Redis Cluster, keyspace `SCAN`
   cannot be relied on to reach the shard owning the graph slot.

The user-visible failure was the Bolt/runtime error:

```text
This command cannot be safely routed in cluster mode - Client
```

## Decision

Adopt a **routing-safe cluster design** built on five rules:

### 1. Dispatch pipelines by connection type at the execution boundary

`gql-redis-store` now uses a `RedisPipeline` abstraction from
`RedisConn::pipeline()`:

- standalone mode -> `redis::pipe()`
- cluster mode -> `redis::cluster::cluster_pipe()`

Call sites keep the concrete `RedisConn` alive until execution so the dispatch
decision remains correct.

### 2. Replace cluster-unsafe metadata scans with per-graph registries and cleanup manifests

Maintain these per-graph registry/manifest keys:

- `G:{g}:_labels`
- `G:{g}:_reltypes`
- `G:{g}:_propidx`

These are used to drive metadata enumeration and cleanup without global
keyspace scans. Callers still validate `_labels` / `_reltypes` members against
the real backing sets before trusting them. `_propidx` is a delete-time cleanup
manifest so orphaned propidx sets can still be removed after partial failures.

### 3. Make graph deletion retryable without allowing name reuse over stale keys

`delete_graph` now uses a temporary per-graph marker:

- `G:{g}:_deleting`

Cleanup sets `_deleting`, removes the per-graph keys, then removes the graph
from `__sd2_registry:graphs`, and only then clears `_deleting`. While the
marker exists, reads treat the graph as already gone and `create_graph(g)` is
rejected, but `delete_graph(g)` itself can be retried safely after partial
backend failures.
### 4. Renew long-running write locks and fail closed if ownership is lost

The per-graph `G:{g}:write_lock` remains a CAS lock, but long-running write
paths now renew the TTL while they process large scans, cleanup loops, or batch
pipelines. A mutation only reports success if the final CAS release proves it
still owned the lock. If the lease was lost, callers receive
`TransactionConflict` rather than a false success.

### 5. Preserve the existing hash-tag slot invariant

All per-graph keys continue to use the graph name as the Redis Cluster hash
tag:

```text
G:{g}:...
```

That invariant keeps graph-local batches slot-local and is now load-bearing for
both write routing and registry-based metadata operations.

## Consequences

### Positive

- Real Bolt create/read traffic now succeeds in Redis Cluster mode
- Metadata planning no longer depends on cluster-unsafe `SCAN`
- `DROP INDEX` and `DELETE GRAPH` now reconstruct slot-local keys instead of
  scanning the global keyspace
- `DELETE GRAPH` remains retryable after partial failures and blocks graph-name
  reuse until cleanup actually finishes
- long-running same-graph writes do not silently outlive their lock lease
- End-to-end resharding validation passes without backend errors

### Negative / Residual risks

#### Partial writes on transport failure

Slot-local pipelining is still not a transaction. A connection loss mid-batch
can leave partial state.

#### Registry staleness

`_labels` and `_reltypes` may retain empty members until filtered against the
real label/reltype sets. This is acceptable because readers validate them
before use.

#### Non-atomic writes still require manifests

Slot-local pipelines are still not transactions. That is why manifest-first
node/edge writes and the `_propidx` cleanup manifest are now part of the design:
they make `delete_graph` capable of cleaning up after partial write/index
failures without reintroducing cluster-unsafe keyspace scans.

#### Separate graph registry remains separate

`__sd2_registry:graphs` still lives outside any graph hash tag. That is safe
because it is never co-pipelined with graph-local commands.

## Alternatives Considered

### Alternative A: Only remove `.atomic()`

Rejected because it was **insufficient**. Store-level batch writes improved,
but the real Bolt path still failed until planner and cleanup `SCAN` paths were
removed.

### Alternative B: Keep branching at every call site

For example:

```rust
if cluster {
    cluster_pipe...
} else {
    pipe...
}
```

Rejected because the branching belongs at the connection/pipeline abstraction
boundary, not duplicated throughout the codebase.

### Alternative C: Rewrite every complex write as Lua

Rejected as overkill for a bug fix. Single-slot pipelining already satisfies
the routing requirement; Lua would add complexity without removing the need for
metadata-path redesign.

### Alternative D: Continue using `SCAN` with ad hoc workarounds

Rejected because keyspace `SCAN` is the wrong primitive for cluster-safe graph
metadata and cleanup. Registries plus entity-set reconstruction are explicit and
slot-local.

## Verification

The accepted design was validated with:

### Store tests

```bash
cargo test -p gql-redis-store
```

### Real cluster integration tests

```bash
GQL_REDIS_CLUSTER_NODES="redis://127.0.0.1:7100,redis://127.0.0.1:7101,redis://127.0.0.1:7102" \
  cargo test -p gql-redis-store --test cluster_pipeline -- --ignored --nocapture
```

### Public Bolt smoke against the cluster compose profile

- start `docker-compose.cluster.yml`
- create a graph over Bolt
- use `session(database=graph)` to create and read nodes/edges

### Resharding resilience test

```bash
VALKEY_PASSWORD='change-me' BOLT_PASSWORD='change-me' \
  ./scripts/test-resharding-resilience.sh
```

Validated result:

- `0` Bolt write errors during load
- `0` backend errors reported during slot movement

## See Also

- [Cluster Write Safety](cluster-write-safety)
- [Redis Cluster Mode](cluster-mode)
- [Redis Key Schema](redis-key-schema)
