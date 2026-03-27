---
layout: default
title: Cluster Write Safety
---

# Cluster Write Safety

Why SedanDelivery's Redis Cluster path is safe after `#745`, what changed, and
what residual risks still remain.

## Contents

- [The Problem: Cluster-Unsafe Pipelines and Metadata Reads](#the-problem-cluster-unsafe-pipelines-and-metadata-reads)
- [The Fix](#the-fix)
- [What Keeps Writes Correct](#what-keeps-writes-correct)
- [Residual Risk](#residual-risk)
- [The Write Lock](#the-write-lock)
- [Write-Lock Observability](#write-lock-observability)
- [Delete Size Guard](#delete-size-guard)
- [Verification](#verification)

---

## The Problem: Cluster-Unsafe Pipelines and Metadata Reads

Issue `#745` was not just a `MULTI/EXEC` problem. Two different cluster-unsafe
behaviours were present:

1. **Generic pipeline execution on cluster connections**

   `redis-rs` 1.0.4 does not let a `ClusterConnection` execute a normal
   `redis::Pipeline::query()` call. In cluster mode, slot-local batches must be
   dispatched through `redis::cluster::cluster_pipe()`.

2. **Planner and DDL paths still used keyspace `SCAN`**

   Bolt queries eventually call metadata helpers such as `graph_stats` and
   `all_label_names`. Those helpers, plus `DROP INDEX` and `DELETE GRAPH`,
   previously depended on keyspace `SCAN`. In Redis Cluster, keyspace `SCAN`
   does not carry a reliable slot hint, so the command may be routed to the
   wrong shard or rejected.

The externally visible symptom was the real Bolt error:

```text
This command cannot be safely routed in cluster mode - Client
```

That is why the first narrow fix ("remove `.atomic()`") was not sufficient on
its own: store-level writes improved, but the real planner/Bolt path still
failed until the metadata and cleanup paths were made cluster-safe too.

---

## The Fix

`#745` landed three complementary changes:

### 1. Cluster-aware pipeline dispatch

`gql-redis-store/src/connection.rs` now exposes a `RedisPipeline` wrapper via
`RedisConn::pipeline()`.

- Standalone mode executes through `redis::pipe()`
- Cluster mode executes through `redis::cluster::cluster_pipe()`

This keeps batching fast without pretending generic pipeline execution works on
cluster connections.

### 2. Preserve the concrete connection type until execution

The write/read/index call sites now keep a concrete `RedisConn` alive until the
pipeline is actually executed. That matters because the final dispatch decision
depends on whether the connection is standalone or cluster-backed.

### 3. Replace cluster-unsafe `SCAN` metadata paths with registries and cleanup manifests

Per-graph registries are now maintained alongside the existing label and
relationship-type sets:

- `G:{g}:_labels`
- `G:{g}:_reltypes`
- `G:{g}:_propidx`

These registries let SedanDelivery enumerate graph metadata and reconstruct
cleanup/index keys without global keyspace scans.

The affected paths now use registries plus entity-set fallback instead of
cluster-unsafe `SCAN`:

- `all_label_names`
- `graph_stats`
- `drop_index`
- `delete_graph`

`G:{g}:_propidx` is a cleanup manifest rather than planner metadata. It lets
`delete_graph` remove propidx sets even if non-atomic index creation or indexed
writes fail after the propidx key was created but before index metadata was
fully registered.

See [Redis Key Schema](redis-key-schema) for the full key contract.

---

## What Keeps Writes Correct

Three properties together make the cluster write path correct:

### 1. Hash-tag slot affinity

Every per-graph key uses the form `G:{g}:...`. Redis Cluster hashes only the
content inside `{...}`, so all keys for graph `g` land on the same slot.

That means a per-graph batch does **not** fan out across shards.

### 2. Write-lock serialization

Read-modify-write operations still take the per-graph lock:

```text
G:{g}:write_lock
```

The lock prevents competing writers from interleaving destructive operations on
the same graph while a batch is in flight.

Long-running same-graph write paths now renew that lease while they walk large
sets or batch loops. A write only reports success if it still proves lock
ownership at release time; if the lease was lost, the caller sees
`TransactionConflict` instead of a false success.

Some hot mutators also take a narrower node/edge-scoped lock key, but that is
currently an extra ownership marker layered on top of `G:{g}:write_lock`, not a
same-graph concurrency feature. Writes within one graph still serialize through
the per-graph lock today.

`delete_graph` also uses a temporary marker:

```text
G:{g}:_deleting
```

While `_deleting` exists, reads treat the graph as already gone and
`create_graph(g)` is rejected, but `delete_graph(g)` itself can be retried. That
closes the old hole where cleanup could fail after the graph name was already
released for reuse.

### 3. Registries are validated against backing sets

`G:{g}:_labels` and `G:{g}:_reltypes` are membership registries, not blind
truth. Readers validate registry members by checking the corresponding backing
set cardinality (`SCARD G:{g}:label:{label}` / `SCARD G:{g}:reltype:{type}`).

That means a stale registry entry does not become a false visible label or
relationship type in query planning.

---

## Residual Risk

### Partial write on TCP disconnect (LOW)

Cluster-safe routing does **not** imply transaction atomicity. If the TCP
connection drops after some pipeline commands have been delivered, partial state
is still possible.

Examples:

- node hash written, but some label updates missing
- edge hash written, but adjacency sets incomplete
- some delete/index cleanup commands applied, but not all

This is an accepted trade-off of slot-local pipelining without `MULTI/EXEC`.
The write lock prevents cross-writer corruption, but it cannot prevent a single
writer's transport failure mid-batch. Manifest-first node/edge writes plus the
`_propidx` cleanup manifest are what make post-failure graph deletion recoverable
without a cluster-unsafe keyspace scan.

### Stale registry members (LOW)

Label and relationship-type registries may retain names whose backing sets are
now empty. This does not break planning because readers filter them against the
real per-label / per-reltype sets before use.

Explicit cleanup is available through the Redis-only
`POST /admin/graphs/{name}/audit` route and the underlying
`RedisGraphStore::audit_graph_metadata(..., repair=true)` API. That repair path
stays graph-scoped, uses the graph write lock for mutations, and re-checks
candidate `_labels`, `_reltypes`, and `_propidx` members under the lock before
pruning them.

`_deleting` is treated more conservatively than the other registries: audit does
not resume deletion and does not clear an active marker while graph-owned
cleanup roots still exist. Even when those roots are already gone, audit only
reports `deletingOrphaned=true`; it does not auto-finalize delete state or
release the graph name for reuse.

### Graph registry remains separate from per-graph slot-local keys

`__sd2_registry:graphs` still has no graph hash tag and still lives in a
different slot. That is fine because it is never co-pipelined with per-graph
commands.

---

## The Write Lock

`G:{g}:write_lock` remains a CAS-protected distributed lock implemented in
`lock.rs`.

### Acquire

```text
SET G:{g}:write_lock <uuid-v4-token> NX PX 5000
```

### Release

```lua
if redis.call("GET", KEYS[1]) == ARGV[1] then
    return redis.call("DEL", KEYS[1])
else
    return 0
end
```

The release path is still cluster-safe because the Lua script operates on a
single slotted key.

### Renew

Long-running operations also renew the same lock token with a CAS-style TTL
refresh:

```lua
if redis.call("GET", KEYS[1]) == ARGV[1] then
    return redis.call("PEXPIRE", KEYS[1], ARGV[2])
else
    return 0
end
```

That keeps `create_index`, `drop_index`, `delete_graph`, and other large
same-graph mutations from silently outliving their lease.

---

## Write-Lock Observability

Redis-backed deployments now emit write-lock lifecycle telemetry on the
existing `/metrics` surfaces:

- Bolt: `http://<host>:<BOLT_HEALTH_PORT>/metrics`
- gql-api: `http://<host>:<GQL_API_PORT>/metrics`

The metrics intentionally use only low-cardinality labels. There is no graph
name, tenant, or user label on any of these series.

| Metric | Type | Meaning |
|--------|------|---------|
| `redis_write_lock_acquire_attempts_total` | Counter | Writes that entered lock acquisition. |
| `redis_write_lock_acquire_successes_total` | Counter | Writes that acquired the lock and entered the mutation body. |
| `redis_write_lock_acquire_contention_total` | Counter | Writes that saw at least one contended acquire attempt before eventual success or failure. |
| `redis_write_lock_contention_failures_total` | Counter | Writes that exhausted retries because the lock stayed contended. |
| `redis_write_lock_acquire_wait_seconds` | Histogram | Time spent waiting for acquisition, recorded for both successful and failed lock attempts. |
| `redis_write_lock_renew_successes_total` | Counter | Successful lease renewals during long-running writes. |
| `redis_write_lock_renew_failures_total` | Counter | Failed renewals. |
| `redis_write_lock_release_successes_total` | Counter | Successful releases after write completion. |
| `redis_write_lock_release_failures_total` | Counter | Failed releases after write completion. If the write body had already failed before release, this counter still increments even though no extra conflict outcome is emitted. |
| `redis_write_lock_hold_seconds` | Histogram | Time the write actually held the lock before release. |
| `redis_write_lock_conflict_outcomes_total{stage="renew"\| "release"}` | Counter | Lease-loss outcomes that were intentionally surfaced as `TransactionConflict`. |
| `redis_write_lock_backend_failures_total{stage="acquire"\| "renew"\| "release"}` | Counter | Redis/pool/backend failures on the coordination path, split by lifecycle stage. |

### How to interpret them

- If `redis_write_lock_acquire_contention_total` and
  `redis_write_lock_acquire_wait_seconds` rise while
  `redis_write_lock_backend_failures_total` stays flat, the coordination path is
  busy but Redis is probably healthy. The hot graph is the bottleneck.
- If `redis_write_lock_contention_failures_total` rises, writers are timing out
  under contention rather than merely waiting longer.
- If `redis_write_lock_backend_failures_total{stage="acquire"}` rises, that is a
  real coordination-path backend problem such as Redis unavailability, routing
  trouble, or pool failure.
- If `redis_write_lock_conflict_outcomes_total` rises, SedanDelivery lost lock
  ownership mid-write and deliberately failed closed instead of returning a
  false success.

This distinction matters because the public Bolt error surface still keeps the
broader storage error model. A write-lock timeout and a real Redis failure can
both produce a generic storage/backend-looking client error, but `/metrics`
separates them into contention, conflict, and backend categories.

---

## Delete Size Guard

`delete_graph` now uses `SCARD` to preflight both the node and edge manifests
before it sets `G:{g}:_deleting` or removes the graph from
`__sd2_registry:graphs`. If either set exceeds the built-in
`MAX_NODES_FOR_DELETE` threshold, the operation is rejected immediately with
`StorageError::DeleteGraphTooLarge`. Only graphs that pass that bound load the
full manifests with `SMEMBERS` for cleanup enumeration.

The same compile-time limit is used for both node and edge
manifests:

| Guarded manifest | Limit source | Current value |
|------------------|-------------|---------------|
| Node IDs (`G:{g}:nodes`) | `gql_redis_store::MAX_NODES_FOR_DELETE` | `100_000` |
| Edge IDs (`G:{g}:edges`) | `gql_redis_store::MAX_NODES_FOR_DELETE` | `100_000` |

No `_deleting` marker is set and no graph metadata is removed when the size
guard fires, so the graph remains fully intact and visible.

### Why the guard exists

`delete_graph` walks the full per-graph key space — nodes, edges, adjacency
sets, label sets, and property-index keys — while holding the per-graph write
lock. For very large graphs, that cleanup can outlive the lock TTL and turn a
best-effort delete into a partially applied mutation.

Rejecting oversized graphs before cleanup starts is safer than beginning the
walk and losing the lease midway through. The typed error makes that refusal
explicit to callers instead of disguising it as a generic backend failure.

### HTTP and Bolt contract

The REST API maps `StorageError::DeleteGraphTooLarge` to **HTTP 413 Payload Too
Large** with a stable error token:

```json
{
  "error": "graph_too_large_for_delete",
  "message": "graph 'bigdata' is too large for atomic deletion; use selective deletion instead"
}
```

The Bolt bridge maps the same condition to:

```text
Neo.ClientError.Database.DeleteGraphTooLarge
```

Both wire contracts intentionally omit the internal node/edge count and the
configured threshold value. Those details remain available in the internal
`StorageError::DeleteGraphTooLarge` variant for logs and tests, but they are not
surfaced to clients.

### Operator guidance

If the size guard fires, delete the graph selectively in batches or use an
offline cleanup tool. This branch does not expose a runtime environment
variable for the delete threshold; the limit is the compile-time constant
`MAX_NODES_FOR_DELETE`.

---

## Verification

The final `#745` fix was validated at three layers:

### Store-level tests

```bash
cargo test -p gql-redis-store
```

### Real cluster regression tests

Run `tests/cluster_pipeline.rs` against a real cluster exposed via
`GQL_REDIS_CLUSTER_NODES`:

```bash
GQL_REDIS_CLUSTER_NODES="redis://127.0.0.1:7100,redis://127.0.0.1:7101,redis://127.0.0.1:7102" \
  cargo test -p gql-redis-store --test cluster_pipeline -- --ignored --nocapture
```

These tests cover:

- pipelined node/edge reads
- batch write paths
- graph metadata reads
- `DROP INDEX`
- `DELETE GRAPH`

### Public Bolt path under resharding

```bash
VALKEY_PASSWORD='change-me' BOLT_PASSWORD='change-me' \
  ./scripts/test-resharding-resilience.sh
```

The script defaults to `docker-compose.cluster.yml`, but CI can override
`COMPOSE_FILE` and set `CI_PREBUILD_BOLT=1` to reuse a host-built Bolt image.
Build that image with `./scripts/build-prebuilt-bolt-image.sh` and combine
`docker-compose.cluster.yml:docker-compose.cluster.ci.yml`.

The validated result for the final fix was:

- slot ownership changed from the source master to the destination master
- `0` write errors from the live Bolt load generator
- `0` read-check errors during the reshard window
- `0` label-mutation errors during the reshard window
- post-reshard read-back matched the write count
- `0` backend errors reported via `/metrics`

### External write-lock telemetry check

```bash
./scripts/test-write-lock-metrics.sh
```

This validator starts a local Redis-backed Bolt instance, forces both transient
and sustained write-lock contention through Bolt, and confirms `/metrics`
classifies successful lock acquisition, contention timeouts, and genuine
acquire-stage backend failures separately.

---

## See Also

- [ADR-005: Redis Cluster Routing Safety](ADR-005-cluster-pipeline-atomicity)
- [Redis Cluster Mode](cluster-mode)
- [Redis Key Schema](redis-key-schema)
