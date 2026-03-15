---
layout: default
title: Admin API
---

# Admin API

Operational HTTP endpoints that sit beside the public graph CRUD surface.

The only admin route documented here is the Redis-backed metadata audit
endpoint below.

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
