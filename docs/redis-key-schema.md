---
layout: default
title: Redis Key Schema
---

# Redis Key Schema

Complete reference for every Redis key used by `gql-redis-store`.

## Contents

- [Per-Graph Keys](#per-graph-keys)
- [Hash-Tag Contract](#hash-tag-contract)
- [Graph Registry Key](#graph-registry-key)
- [Adjacency Encoding](#adjacency-encoding)
- [Property Index Keys](#property-index-keys)
- [Key Prefix](#key-prefix)
- [Key Uniqueness Guarantees](#key-uniqueness-guarantees)

---

## Per-Graph Keys

All per-graph keys embed the graph name inside curly braces (`{graphname}`).
This is the Redis Cluster hash tag — Redis computes the slot from only the
content inside `{...}`, guaranteeing that all keys for a given graph land on
the same cluster node.

| Key Pattern | Redis Type | Purpose |
|-------------|-----------|---------|
| `G:{g}:node:{id}` | Hash | Node data: `labels` (JSON array), `props` (JSON object) |
| `G:{g}:edge:{id}` | Hash | Edge data: `src`, `dst`, `rel_type`, `props` (JSON) |
| `G:{g}:node_seq` | String | Monotonic node ID counter (INCRBY) |
| `G:{g}:edge_seq` | String | Monotonic edge ID counter (INCRBY) |
| `G:{g}:label:{label}` | Set | Node IDs with this label |
| `G:{g}:reltype:{type}` | Set | Edge IDs with this relationship type |
| `G:{g}:_labels` | Set | Registry of label names seen in this graph |
| `G:{g}:_reltypes` | Set | Registry of relationship-type names seen in this graph |
| `G:{g}:out:{node_id}` | Set | Outgoing adjacency: `"{eid}:{neighbor_id}:{rel_type}"` members |
| `G:{g}:in:{node_id}` | Set | Incoming adjacency: `"{eid}:{neighbor_id}:{rel_type}"` members |
| `G:{g}:nodes` | Set | All node IDs in the graph |
| `G:{g}:edges` | Set | All edge IDs in the graph |
| `G:{g}:indexes` | Set | Active property indexes: `"{label}\x00{property}"` members |
| `G:{g}:_deleting` | String | Marker set while `delete_graph` cleanup is in progress and retryable |
| `G:{g}:_propidx` | Set | Manifest of fully-qualified propidx keys for delete-time cleanup |
| `G:{g}:propidx:{label}\x00{prop}\x00{value_key}` | Set | Node IDs matching this (label, property, value) triple |
| `G:{g}:write_lock` | String | Distributed write-lock token (CAS acquire, CAS renew, CAS release) |

Where `{g}` is the graph name (the hash tag content).

### Example keys for graph `social`

```
G:{social}:node:1          → Hash  {labels: '["Person"]', props: '{"name":"Alice"}'}
G:{social}:edge:1          → Hash  {src: "1", dst: "2", rel_type: "KNOWS", props: "{}"}
G:{social}:node_seq        → String "42"
G:{social}:label:Person    → Set   {"1", "3", "7"}
G:{social}:reltype:KNOWS   → Set   {"1", "2"}
G:{social}:_labels         → Set   {"Person", "Employee"}
G:{social}:_reltypes       → Set   {"KNOWS", "REPORTS_TO"}
G:{social}:out:1           → Set   {"1:2:KNOWS"}
G:{social}:in:2            → Set   {"1:1:KNOWS"}
G:{social}:nodes           → Set   {"1", "2", "3"}
G:{social}:edges           → Set   {"1", "2"}
G:{social}:_deleting       → String "1" (only while delete_graph is finishing cleanup)
G:{social}:_propidx        → Set   {"G:{social}:propidx:Person\x00name\x00s:Alice", ...}
G:{social}:write_lock      → String "<uuid-v4-token>" (when locked / being renewed)
```

---

## Hash-Tag Contract

**All per-graph keys use `{graphname}` as the hash tag.** This is a hard
invariant, enforced by the key-generation functions in `keys.rs` and verified
by the test `all_graph_keys_contain_hash_tag`.

Consequences:
- Every key for graph `g` hashes to `slot(g)` — the same cluster node.
- Multi-key pipelines (HSET + SADD + …) are routable in cluster mode without MULTI/EXEC.
- Cross-graph operations do not share a slot — each graph is isolated.

**Do not** construct keys manually. Always use the helpers in `keys.rs`:

```rust
use crate::keys::{key_node, key_edge, key_label, key_reltype, key_out, key_in,
                  key_nodes_set, key_edges_set, key_graph_lock};

let nk = key_node("social", 42);      // "G:{social}:node:42"
let lk = key_label("social", "Person"); // "G:{social}:label:Person"
```

### Hash-tag injection: why graph names reject `{` and `}`

`validate_graph_name` rejects any graph name that contains `{` or `}` with a
`ValidationError`. This is a **security boundary**, not a cosmetic naming rule.

If an attacker controls the graph name and embeds braces — for example
`{admin}` — Redis computes the slot from `admin` rather than the full graph
name. This produces two consequences:

1. **Slot hijacking:** all keys for this graph route to `slot(admin)`,
   colliding with any legitimate data tagged `{admin}`.
2. **Cross-graph collision:** key patterns become indistinguishable from keys
   belonging to a graph legitimately named `admin`, enabling reads or writes
   against another graph's data.

`validate_graph_name` enforces this before any key is constructed. A caller
receiving `ValidationError: graph name must not contain '{' or '}'` should
treat it as a rejected injection attempt, not a cosmetic formatting error.

When adding new key helper functions, every helper must be covered by a
regression test asserting the returned key contains the `{graphname}` hash
tag. The `all_graph_keys_contain_hash_tag` test enforces this invariant.

---

## Graph Registry Key

```
__sd2_registry:graphs    → Set   {"social", "transactions", ...}
```

This key stores the names of all graphs. It has **no hash tag** — it is not
a per-graph key. It routes to its own cluster slot.

It is **never** co-pipelined with per-graph commands. It is only accessed via
standalone `SADD`, `SREM`, `SMEMBERS`, and `SISMEMBER` commands, each on
their own round-trip. There is no cross-slot issue.

---

## Metadata Registries

Two per-graph registry sets exist specifically to make metadata enumeration and
cleanup cluster-safe without global keyspace scans:

```text
G:{g}:_labels    → Set of label names
G:{g}:_reltypes  → Set of relationship-type names
G:{g}:_propidx   → Set of fully-qualified propidx keys created for the graph
```

These registries are updated on create/add-label/create-edge paths and are used
by:

- `all_label_names`
- `graph_stats`
- `drop_index`
- `delete_graph`

`G:{g}:_propidx` is different from `_labels` / `_reltypes`: it is not planner
metadata. It is a cleanup manifest used so `delete_graph` can remove propidx
sets even if non-atomic index creation or indexed writes fail part-way through.

### Important: registry membership is a hint, not blind truth

Deletes do not eagerly prune empty label or relationship-type names from the
registries. Instead, readers validate registry members against the real backing
sets:

- `SCARD G:{g}:label:{label}`
- `SCARD G:{g}:reltype:{type}`

That means a stale registry member does not become a false visible label or
relationship type in query planning.

This design trades eager cleanup for cluster-safe enumeration and simpler write
paths.

`POST /admin/graphs/{g}/audit` is the explicit Redis-only repair path for these
registries. With `repair=true`, it re-checks candidate `_labels`, `_reltypes`,
and `_propidx` members under the graph write lock before pruning them. It stays
graph-scoped and does not attempt any global scan or speculative rebuild.

## Write-Lock Semantics

`G:{g}:write_lock` is a CAS-style lease:

- acquire: `SET ... NX PX <ttl>`
- renew: compare token, then `PEXPIRE`
- release: compare token, then `DEL`

Long-running same-graph write paths renew the lease while they process large
sets or batch loops. A mutation only reports success if the final CAS release
still proves ownership; otherwise it returns `TransactionConflict` instead of a
false success.

---

## Adjacency Encoding

Out-adjacency and in-adjacency set members encode three fields in a single
string:

```
"{edge_id}:{neighbor_id}:{rel_type}"
```

- `edge_id` — the edge's numeric ID
- `neighbor_id` — the other end of the edge from the perspective of the node
  owning this adjacency set
- `rel_type` — the relationship type string

For outgoing adjacency (`G:{g}:out:{src_id}`), `neighbor_id` is the
destination node. For incoming adjacency (`G:{g}:in:{dst_id}`),
`neighbor_id` is the source node.

`rel_type` may contain colons; the decoder (`decode_adj`) splits on the
first two colons only, so `"5:10:ns:HAS:ITEM"` correctly decodes to
`edge_id=5`, `neighbor_id=10`, `rel_type="ns:HAS:ITEM"`.

### Example

```
G:{social}:out:1  contains  "3:2:KNOWS"
```

→ Edge 3 goes from node 1 to node 2 with type `KNOWS`.

```
G:{social}:in:2   contains  "3:1:KNOWS"
```

→ Edge 3 arrives at node 2 from node 1 with type `KNOWS`.

---

## Property Index Keys

Property indexes enable O(1) lookups by (label, property, value) instead of
O(N) full-label scans.

```
G:{g}:propidx:{label}\x00{property}\x00{value_key}  →  Set of node IDs
```

The null byte (`\x00`) separates the three components: `label`, `property`,
and `value_key`. For example:

```
G:{social}:propidx:Person\x00name\x00s:Alice   → Set {"1"}
G:{social}:propidx:Person\x00age\x00i:30       → Set {"1", "5"}
```

The manifest key:

```text
G:{g}:_propidx
```

stores the fully-qualified propidx keys that have been created for the graph.
This lets `delete_graph` clean up orphaned propidx sets even if index metadata
registration fails after some propidx sets were already written.

---

## Retryable Graph Deletion State

`delete_graph` is intentionally a two-phase cleanup:

1. Set `G:{g}:_deleting`
2. Delete all per-graph keys
3. Remove the graph from `__sd2_registry:graphs`
4. Clear `G:{g}:_deleting`

While `_deleting` exists:

- `graph_exists` returns `false`
- `list_graphs` hides the graph
- read operations return `GraphNotFound`
- `create_graph(g)` is rejected until cleanup finishes

This makes graph deletion retryable after partial backend failures without
allowing the same graph name to be reused over leftover keys.

The metadata audit path treats `_deleting` conservatively:

- it does **not** resume deletion
- it does **not** clear an active marker while delete-time graph roots still exist
- when those delete-time roots are already absent, audit reports
  `deletingOrphaned=true` but still leaves `_deleting` in place for operator
  intervention or an explicit `delete_graph` retry

### Security: null-byte separator is an injection boundary

Property key names must not contain null bytes (`\x00`). This character is
the **structural separator** in `key_propidx()` construction — it is not an
encoding detail.

A null byte in a caller-supplied property key name would produce a Redis key
indistinguishable from a different `(label, property)` pair. For example, a
property key `"name\x00age"` on label `"Person"` would construct:

```
G:{g}:propidx:Person\x00name\x00age\x00<value>
```

which has the same byte sequence as property key `"age"` on a label
`"Person\x00name"`. This enables reading or overwriting index entries for a
different `(label, property)` — an injection vector, not a formatting constraint.

`validate_property_key` enforces the null-byte rule before any key is
constructed. A caller receiving `ValidationError: property key must not
contain null bytes` should treat this as a rejected injection attempt.

### Value key encoding

| Value type | Prefix | Example |
|------------|--------|---------|
| Integer | `i:` | `i:42`, `i:-1` |
| Float | `f:` | `f:4614253070214989087` (bit representation) |
| String | `s:` | `s:Alice` |
| Boolean | `b:` | `b:true`, `b:false` |
| Other | `j:` | `j:Null` (Debug representation) |

Float values use their IEEE 754 bit representation for exact, deterministic
key generation (same float always produces the same key, including NaN).

The active indexes for a graph are tracked in `G:{g}:indexes` as a Set of
`"{label}\x00{property}"` strings. `list_indexes` reads this set; property
index maintenance (add/remove on write) reads it to know which indexes exist.

---

## Key Prefix

When `GQL_REDIS_KEY_PREFIX` is set, all keys (per-graph and registry) are
prefixed with that value:

```bash
GQL_REDIS_KEY_PREFIX="prod:"
```

Results in:

```
prod:G:{social}:node:1
prod:G:{social}:label:Person
prod:__sd2_registry:graphs
```

The prefix is prepended outside the hash tag, so it does not affect slot
routing. Two deployments with different prefixes sharing a Redis instance
are fully isolated.

---

## Key Uniqueness Guarantees

| Guarantee | Mechanism |
|-----------|-----------|
| Node IDs unique within a graph | `INCRBY G:{g}:node_seq 1` — monotonic, never reused |
| Edge IDs unique within a graph | `INCRBY G:{g}:edge_seq 1` — monotonic, never reused |
| Batch IDs unique | `INCRBY G:{g}:node_seq N` — allocates a contiguous block; `start = end - N + 1` |
| Cross-graph isolation | Different graph names → different hash tags → different slots |
| No label/property collision in propidx | Null-byte separator prevents `"Per\x00son"` + `"name"` colliding with `"Per"` + `"\x00son"` |
| Cluster-safe metadata enumeration | `_labels` / `_reltypes` registries + backing-set cardinality checks |
