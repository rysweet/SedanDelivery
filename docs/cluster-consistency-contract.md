---
layout: default
title: Cluster Consistency Contract
---

# Cluster Consistency Contract

The user-facing guarantees SedanDelivery makes about Redis-backed cluster
deployments after the **cluster consistency hardening** package.

This document is the canonical contract that the Bolt spec, the REST API docs,
and the Redis module command set all defer to.

## Contents

- [Scope](#scope)
- [Per-Graph Serializability](#per-graph-serializability)
- [No Cross-Graph Transactions](#no-cross-graph-transactions)
- [Fencing Tokens and Stale-Writer Rejection](#fencing-tokens-and-stale-writer-rejection)
- [Hot-Write Atomicity (Single Node, Single Edge)](#hot-write-atomicity-single-node-single-edge)
- [Behavior Under Failure](#behavior-under-failure)
- [Error Surfaces](#error-surfaces)
- [Audit and Repair Endpoints](#audit-and-repair-endpoints)
- [What This Contract Does Not Promise](#what-this-contract-does-not-promise)

---

## Scope

This contract applies to:

- Redis-backed deployments (`RedisGraphStore`), in both standalone and Redis
  Cluster mode
- All four network surfaces: Bolt v4.4, REST, Gremlin, and the Redis module
  commands (`GQL.QUERY`, `GQL.MUTATE`)

Embedded and in-memory deployments are stricter (single-process) and inherit
these guarantees trivially.

---

## Per-Graph Serializability

For any one named graph, concurrent writers are **serializable** with respect to
each other. The mechanism is:

1. **Per-graph write lock** at `G:{g}:write_lock` with a UUID lease token.
2. **Monotonic fence counter** at `G:{g}:write_lock_fence` incremented on every
   successful acquire.
3. **Server-side fence validation** at every lease renewal and inside every
   slot-local Lua hot-write script.

A writer that loses its lease — because it stalled past TTL and a competing
writer acquired — cannot land any further mutations. The next renew or hot-write
returns `LockFenced` and the operation fails closed.

Reads are not lock-coordinated. A read returns a consistent view as of the
moment its component commands executed; readers may observe writes from any
serialized writer that completed before them.

---

## No Cross-Graph Transactions

There is **no** atomic multi-graph mutation primitive. Specifically:

- Bolt `BEGIN` / `COMMIT` over multiple graph names is not supported. A Bolt
  session is scoped to a single `database` field per `RUN`.
- A query that writes to graphs `A` and `B` is two independent serialized
  writes, one against `A` and one against `B`. A failure between them leaves
  one applied and the other not.
- Redis module commands (`GQL.QUERY`, `GQL.MUTATE`) operate against a single
  graph per call. Multi-graph mutation in a single command is not supported and
  is rejected with the surface's standard "unsupported operation" error.

This is a deliberate property of the slot-local cluster design: every per-graph
key uses the `{g}` hash tag, and Redis Cluster's atomicity unit is a single
slot.

If you need atomicity across two graphs, model them as one graph with a label
discriminator.

---

## Fencing Tokens and Stale-Writer Rejection

Every write lock acquisition produces a `(token, fence)` pair:

- `token` — UUIDv4 lease identifier (back-compat with the pre-hardening lock)
- `fence` — `u64` from `INCR G:{g}:write_lock_fence`

Both are validated server-side via Lua at two checkpoints:

| Checkpoint | Script | Failure mode |
|---|---|---|
| Lease renew | `CAS_RENEW_SCRIPT` | `LockFenced` if either token or fence does not match |
| Hot-write Lua (node create, edge create) | `NODE_CREATE_WITH_LABELS_SCRIPT`, `EDGE_CREATE_WITH_ADJACENCY_SCRIPT` | `STALE_FENCE` Redis error → `LockFenced` |

Renewals happen at roughly `lock_ttl / 3`, so a stalled writer is detected and
fenced within one renewal interval of the competing acquire — bounded by
`lock_ttl / 3` (≈ 1.7 s with the default 5 s lease).

### Why fences are kept separate from lease tokens

UUID lease tokens defend against accidentally releasing somebody else's lock.
Monotonic fences defend against a writer that legitimately held the lock,
stalled, lost it, and then tried to land late writes against shared state. The
two failure modes are distinct, and the new `LockFenced` error variant lets
clients and operators see the difference in metrics and logs.

---

## Hot-Write Atomicity (Single Node, Single Edge)

Single-entity hot-write paths are now atomic per Redis slot:

| Operation | Script | Atomic across |
|---|---|---|
| `CREATE (n:Label1:Label2 {props})` (single node) | `NODE_CREATE_WITH_LABELS_SCRIPT` | node hash + nodes set + per-label sets + label registry |
| `CREATE (a)-[:R {props}]->(b)` (single edge) | `EDGE_CREATE_WITH_ADJACENCY_SCRIPT` | edge hash + edges set + per-reltype set + outbound adjacency + inbound adjacency + reltype registry |

Because all keys touched by either script share the `{g}` hash tag, the script
executes inside a single Redis slot and is fully atomic. This closes the
previously documented partial-write windows where:

- a `MATCH (n:Label)` could see `n` before its label set was updated, or
- a `MATCH (a)-[:R]->(b)` could see the edge hash before the reverse
  `(b)<-[:R]-(a)` adjacency was written.

### What is **not** Lua-ified

Bulk and manifest paths are intentionally untouched:

- multi-row `UNWIND ... CREATE` of nodes
- multi-row `UNWIND ... CREATE` of edges
- `LOAD CSV`-style bulk import
- `delete_graph` cleanup walk

Bulk paths still use slot-local pipelines and inherit the per-graph lock + fence
protection at lease-renewal cadence. That trade-off is documented in
[ADR-005](ADR-005-cluster-pipeline-atomicity).

---

## Behavior Under Failure

| Failure | Visible result | Recovery |
|---|---|---|
| Writer crashes mid-bulk | Partial writes possible; lock TTL expires; next writer fences out the dead one | Run `POST /admin/graphs/{name}/audit?repair=true` to prune stale registry entries |
| TCP drop mid-Lua hot-write | No effect (Lua is atomic per slot — the script either ran or did not) | None required |
| TCP drop mid-bulk pipeline | Some commands of the pipeline may have applied | Audit + repair as above |
| Lease stolen by competing writer | Original writer sees `LockFenced` on next renew or hot-write | None required; client should retry on the same connection |
| `delete_graph` interrupted before the marker is cleared | Graph stuck in `_deleting` state; new `create_graph` of the same name is rejected | Operator runs `POST /admin/graphs/{name}/finalize-delete?confirm=true` after audit reports `deletingOrphaned=true` |
| Cluster reshard in flight | Pipelines and hot-write scripts retry through `cluster_pipe()` MOVED handling | None required |

---

## Error Surfaces

The hardening package introduces one new error variant: `StorageError::LockFenced`.
Its mapping across surfaces:

| Surface | Code / Token | Client retry guidance |
|---|---|---|
| REST | `423 Locked` with body `{"error":"lock_fenced","graph":"<name>"}` | Retry once with small jitter (50–200 ms); do not retry indefinitely |
| Bolt v4.4 | `Neo.TransientError.Transaction.LockClientStopped` (driver-retryable) | Standard Neo4j driver retry policy applies |
| Redis module | `-FENCED <graph>` RESP error | Retry once; treat repeated fences as a contention signal |
| Internal | `StorageError::LockFenced { graph: String }` (defined in `cypher-core::error`) | — |

Distinct from `TransactionConflict` (which means "lease was already lost"),
`LockFenced` means "lease was lost and a server-side fence proved it." Clients
should retry both, but the metrics are separate so operators can tell drift
from abandonment. Both are uncommon under healthy operation; persistent
`LockFenced` rejections indicate either pathological writer stalls or
lock-TTL misconfiguration, not a normal retry loop.

### Metric

```
sd2_redis_lock_fence_rejected_total{graph="<name>"}
```

`LockFenced` is **not** counted in `sedandelivery_bolt_backend_errors_total`
and does **not** count toward the resharding-resilience zero-backend-errors
gate. That distinction is verified by `scripts/test-resharding-resilience.sh`.

---

## Audit and Repair Endpoints

| Endpoint | Purpose | Required state |
|---|---|---|
| `POST /admin/graphs/{name}/audit` | Detect stale registry members and orphaned `_deleting` markers | Any |
| `POST /admin/graphs/{name}/audit?repair=true` | Prune stale `_labels`, `_reltypes`, `_propidx` members under the graph write lock | Graph exists |
| `POST /admin/graphs/{name}/finalize-delete?confirm=true` | Clear an orphaned `_deleting` marker and release the graph name | Audit must report `deletingOrphaned=true` |

See [Admin API](admin-api) for full request/response schemas and
[Cluster Write Safety — Stuck `_deleting` Runbook](cluster-write-safety#runbook-clearing-a-stuck-_deleting-marker)
for the operational procedure.

---

## What This Contract Does Not Promise

- **No snapshot isolation across graphs.** Two reads against different graphs
  may straddle a writer that completed between them.
- **No global ordering across graphs.** Wall-clock ordering of writes to
  different graphs is not preserved by the storage layer.
- **No durability beyond Redis's own AOF/RDB configuration.** SedanDelivery
  does not add a write-ahead log on top of Redis Cluster.
- **No automatic finalize of stuck deletes.** `finalize-delete` is always an
  operator-confirmed action; audit will never clear `_deleting` on its own.
- **No transactional read-modify-write on bulk paths.** Bulk paths are
  serialized per graph but not slot-atomic; the audit/repair endpoints exist
  to clean up the bounded drift this admits.

---

## See Also

- [Cluster Write Safety](cluster-write-safety)
- [Redis Cluster Mode](cluster-mode)
- [Admin API](admin-api)
- [ADR-005: Redis Cluster Routing Safety](ADR-005-cluster-pipeline-atomicity)
- [Bolt Spec — Consistency Contract](specs/sedandelivery-bolt-spec#consistency-contract)
