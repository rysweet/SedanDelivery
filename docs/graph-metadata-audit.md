---
layout: default
title: Graph Metadata Audit
---

# Graph Metadata Audit

How Redis graph metadata drifts, what the audit checks, and what `repair=true`
does — and does **not** do.

## Why these registries drift

SedanDelivery keeps three Redis metadata registries per graph:

| Key | Role |
|---|---|
| `G:{g}:_labels` | label-name registry |
| `G:{g}:_reltypes` | relationship-type registry |
| `G:{g}:_propidx` | manifest of propidx keys used for delete-time cleanup |

`_labels` and `_reltypes` are append-on-write: create/add paths register names,
but remove/delete paths do not eagerly prune the registry member when the last
backing set entry disappears.

`_propidx` can drift after `drop_index`: the backing propidx keys are deleted,
but the cleanup manifest can retain stale members.

These stale entries do not surface through normal schema reads because readers
already cross-check the registries against their backing sets. The audit exists
to clean up the leftover metadata.

## What the audit checks

The route is:

```text
POST /admin/graphs/{name}/audit?repair=true|false
```

It checks four things:

### `_labels`

A label member is stale when the registry still contains the label but the
backing set is empty:

```text
SCARD G:{g}:label:{label} == 0
```

### `_reltypes`

A relationship-type member is stale when the registry still contains the type
but the backing set is empty:

```text
SCARD G:{g}:reltype:{type} == 0
```

### `_propidx`

A propidx manifest member is stale when it either:

- points outside the graph's own `G:{g}:propidx:*` namespace, or
- points to a propidx key that no longer exists

### `_deleting`

`G:{g}:_deleting` marks a graph as mid-delete. The audit reports
`deletingOrphaned=true` when that marker exists but the normal delete-time
cleanup roots are already gone:

- `node_seq`
- `edge_seq`
- `nodes`
- `edges`
- `indexes`
- `_labels`
- `_reltypes`
- `_propidx`

This is a signal that delete state looks orphan-like, **not** a signal that the
graph name is safe to reuse.

## Two-phase detection

The audit is intentionally conservative:

1. **Read phase (no lock)**  
   Collect candidate stale registry members with `SMEMBERS`, then check backing
   sets with `SCARD` / `EXISTS`.

2. **Repair phase (write lock, only when `repair=true`)**  
   Re-check every candidate under the graph write lock and prune only members
   that are still stale.

This keeps lock hold time short while avoiding time-of-check/time-of-use races
on the actual pruning step.

## What `repair=true` does

With `repair=true`, the audit may:

- `SREM` stale members from `G:{g}:_labels`
- `SREM` stale members from `G:{g}:_reltypes`
- `SREM` stale or malformed members from `G:{g}:_propidx`

The operation is idempotent: rerunning repair after cleanup leaves the same
registries clean.

## What `repair=true` does NOT do

Audit is **not** a delete-recovery engine.

It does **not**:

- retry `delete_graph`
- clear an active `_deleting` marker
- release the graph name for reuse
- scan arbitrary Redis keys outside the graph's propidx namespace

If `_deleting` is present, the conservative recovery path is still a human
decision or an explicit `delete_graph` retry — not an audit-side auto-clear.

## Response fields

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

With `repair=false`, the `stale*` arrays are the lock-free read-phase findings.
With `repair=true`, they are the subset revalidated under the graph write lock
and pruned.

## Errors

| HTTP status | Meaning |
|---|---|
| `400` | invalid graph name |
| `404` | graph absent and not mid-delete |
| `500` | backend/storage failure, including write-lock acquisition failure surfaced as backend error |
| `501` | non-Redis backend |

## Operational guidance

Use `repair=false` when you want a dry run.

Use `repair=true` when you want to prune stale `_labels`, `_reltypes`, or
`_propidx` members that the audit can still prove stale under the lock.

If `deletingOrphaned=true`, treat that as an operator signal to inspect delete
state — not as an automatic recovery path.
