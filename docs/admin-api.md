---
layout: default
title: Admin API
---

# Admin API

Operational HTTP endpoints that sit beside the public graph CRUD surface.

This page documents two Redis-backed admin routes:

- `POST /admin/graphs/{name}/audit` — registry/marker audit and optional repair
- `POST /admin/graphs/{name}/finalize-delete` — operator-confirmed clearing of
  an orphaned `_deleting` marker

Both routes are subject to the consistency guarantees documented in the
[Cluster Consistency Contract](cluster-consistency-contract).

## `POST /admin/graphs/{name}/audit`

Audit one graph's Redis metadata registries and optionally prune entries that
are still provably stale when re-checked under the graph write lock.

### Query parameters

| Field | Type | Default | Description |
|---|---|---|---|
| `repair` | bool | `false` | `false`: report-only audit. `true`: prune stale `_labels`, `_reltypes`, and `_propidx` entries after a locked re-check. |

No request body is required.

### Backend support

This route is implemented only for `RedisGraphStore`.

- Redis-backed API instances execute the graph-scoped audit
- non-Redis backends return `501 Not Implemented`

There is no separate per-route scope or rate-limit framework for this route.
When Entra auth is enabled, the route uses the API's existing bearer-token
middleware. If deployments need narrower access control, that must be enforced
at the front door (gateway, ingress, or service principal assignment).
When Entra auth is disabled, `/admin/*` is not a separate security boundary;
it follows the same API-wide no-Entra posture as the rest of the HTTP surface.

### What the audit checks

- `_labels`: a member of `G:{g}:_labels` is stale when
  `SCARD G:{g}:label:{label} == 0`
- `_reltypes`: a member of `G:{g}:_reltypes` is stale when
  `SCARD G:{g}:reltype:{type} == 0`
- `_propidx`: a member of `G:{g}:_propidx` is stale when it either:
  - points outside the graph's own `G:{g}:propidx:*` namespace, or
  - points to a propidx key that no longer exists
- `_deleting`: audit reports when the normal delete-time cleanup roots are
  already gone, but it does **not** resume deletion and does **not** clear the
  marker automatically

### Response body

```json
{
  "graph": "social",
  "repair": true,
  "staleLabels": [],
  "staleReltypes": [],
  "stalePropidx": [],
  "deletingOrphaned": false
}
```

| Field | Type | Meaning |
|---|---|---|
| `graph` | string | Graph name that was audited |
| `repair` | bool | Whether repair was requested |
| `staleLabels` | string[] | Label registry members that audit found stale |
| `staleReltypes` | string[] | Relationship-type registry members that audit found stale |
| `stalePropidx` | string[] | Stale or malformed propidx manifest members that audit found stale |
| `deletingOrphaned` | bool | `true` when `_deleting` exists but the normal delete-time cleanup roots are already absent |

When `repair=false`, the `stale*` arrays are the lock-free read-phase snapshot.
When `repair=true`, they are the subset revalidated under the graph write lock
and then pruned.

### Examples

Dry run:

```bash
curl -X POST 'http://localhost:3001/admin/graphs/social/audit'
```

Repair stale registry members:

```bash
curl -X POST 'http://localhost:3001/admin/graphs/social/audit?repair=true'
```

### Error conditions

| HTTP status | Condition |
|---|---|
| `400 Bad Request` | Invalid graph name |
| `404 Not Found` | Graph does not exist and has no active `_deleting` marker |
| `500 Internal Server Error` | Redis/backend failure, including write-lock acquisition failure surfaced as backend error |
| `501 Not Implemented` | Active backend is not `RedisGraphStore` |

For the deeper operational model — why registries drift, why `_deleting` is
report-only, and when to use `repair=true` — see
[Graph Metadata Audit](graph-metadata-audit).

---

## `POST /admin/graphs/{name}/finalize-delete`

Operator-confirmed clearing of an orphaned `G:{g}:_deleting` marker. This route
exists because the audit endpoint is intentionally report-only for `_deleting`:
even when audit reports `deletingOrphaned=true`, the marker is never cleared
automatically.

### When to use it

Only when **all** of the following are true:

1. `POST /admin/graphs/{name}/audit` reports `"deletingOrphaned": true`.
2. You have confirmed (out of band) that no in-flight `delete_graph` is
   resuming on another node.
3. You want the graph name released for reuse.

If any of those is unclear, run audit again instead. This endpoint is not a
replacement for letting a normal `delete_graph` finish.

### Query parameters

| Field | Type | Required | Description |
|---|---|---|---|
| `confirm` | string | yes | Must equal exactly `true`. Any other value (including `1`, `yes`, missing) returns `400 Bad Request`. |

The request body must be empty or the literal `{}` (≤ 1 KiB). Anything else
returns `400 Bad Request`.

### What the route does

Under a freshly acquired graph write lock with a fresh fence token:

1. Re-runs `audit_graph_metadata` with `repair=false`. If the result does not
   show `deletingOrphaned=true`, returns `409 Conflict`.
2. Sweeps remaining `G:{g}:*` keys via cluster-safe `SCAN` + `UNLINK`,
   including `G:{g}:write_lock_fence` and the lock key itself.
3. `SREM`s `{name}` from `__sd2_registry:graphs`.
4. Releases the lock.

The operation is idempotent: re-invoking on a name that is no longer in the
registry returns `404 Not Found`.

### Response body (success)

```json
{
  "graph": "social",
  "finalized": true,
  "deletedKeys": [
    "G:{social}:_deleting"
  ],
  "registryRemovedFrom": "__sd2_registry:graphs"
}
```

| Field | Type | Meaning |
|---|---|---|
| `graph` | string | Graph name that was finalized |
| `finalized` | bool | Always `true` on success |
| `deletedKeys` | string[] | Per-graph Redis keys that were `DEL`'d outright. The `_deleting` marker is the canonical entry. The lease key (`G:{g}:write_lock`) and fence counter (`G:{g}:write_lock_fence`) are NOT included — the lease is released via CAS-release after the sweep, and the fence counter is preserved across re-creations of the same graph name to keep its monotonicity contract intact. |
| `registryRemovedFrom` | string \| null | The registry set the graph membership was `SREM`'d from (typically `__sd2_registry:graphs`), or `null` if the membership was already absent. The registry key itself is **not** deleted — only the graph's membership in it. |

### Examples

Refuse without `confirm=true`:

```bash
curl -X POST 'http://localhost:3001/admin/graphs/social/finalize-delete'
# 400 Bad Request: {"error":"confirmation_required","message":"add ?confirm=true to finalize"}
```

Happy path:

```bash
# 1. Confirm orphaned state
curl -X POST 'http://localhost:3001/admin/graphs/social/audit' \
  | jq '.deletingOrphaned'
# true

# 2. Finalize
curl -X POST 'http://localhost:3001/admin/graphs/social/finalize-delete?confirm=true'
```

Refuse when graph is live:

```bash
curl -X POST 'http://localhost:3001/admin/graphs/social/finalize-delete?confirm=true'
# 409 Conflict: {"error":"graph_not_orphaned","message":"graph 'social' is not in deleting-orphaned state"}
```

### Error conditions

| HTTP status | Condition |
|---|---|
| `400 Bad Request` | Missing or wrong `confirm` value, invalid graph name, or non-empty body |
| `404 Not Found` | Graph name is not in `__sd2_registry:graphs` and no `_deleting` marker exists |
| `409 Conflict` | Audit re-check shows the graph is **not** in deleting-orphaned state (live graph or in-flight delete) |
| `423 Locked` | Lease was fenced by a competing operator during finalize; safe to retry |
| `500 Internal Server Error` | Redis/backend failure during sweep or `SREM` |
| `501 Not Implemented` | Active backend is not `RedisGraphStore` |

### Authentication

Same Entra-bearer middleware as `/admin/graphs/{name}/audit`. There is no
separate per-route RBAC; deployments that need narrower access control must
enforce it at the gateway.

Every successful or refused finalize emits an audit log line containing the
graph name, the outcome, and the cleared key list. When Entra bearer auth is
enabled, the line also includes the caller's `oid`; when auth is disabled
(local/dev deployments) the caller identity field is omitted.

### See also

- [Cluster Write Safety — Stuck `_deleting` Runbook](cluster-write-safety#runbook-clearing-a-stuck-_deleting-marker)
- [Cluster Consistency Contract](cluster-consistency-contract)
