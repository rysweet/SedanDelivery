# Storage Correctness — `PersistentGraphStore` & `InMemoryGraphStore`

> **Scope**: This document describes the **target** correctness guarantees
> that will be enforced after the changes in issue #1201 land. Until those
> changes merge, the current code does not yet implement: WAL-first ordering
> (§1), constraint WAL/snapshot persistence (§2), `read_snapshot()`-based
> atomic snapshots (§3), or `_sd2_id`/`_sd2_eid` collision detection (§4).
> For the pre-existing crash-recovery contract, see
> [recovery_contract.md](recovery_contract.md).

This document describes the correctness guarantees enforced by the
`cypher-core` storage layer. It covers WAL ordering, constraint persistence,
snapshot atomicity, and entity collision detection.

---

## 1. Write-Ahead Logging (WAL-first semantics)

### Guarantee

Every mutation to `PersistentGraphStore` writes its WAL entry **before** the
corresponding in-memory state change is applied. If the process crashes after
the WAL write but before the in-memory mutation, the WAL entry is replayed on
recovery and the mutation is re-applied. If the process crashes before the WAL
write, no partial state is visible.

### Ordering protocol

| Operation type | WAL-first sequence |
|---|---|
| **Create node** | `allocate_node_id()` → `append_wal(CreateNode { id })` → `create_node_with_id(id)` |
| **Create edge** | `allocate_edge_id()` → `append_wal(CreateEdge { id })` → `create_edge_with_id(id)` |
| **Property mutation** | `append_wal(SetNodeProperties / ReplaceNodeProperties / …)` → in-memory mutation |
| **Label mutation** | `append_wal(AddNodeLabels / RemoveNodeLabels)` → in-memory mutation |
| **Delete node** | Read edge IDs → `append_wal(DeleteEdge × N)` → `append_wal(DeleteNode)` → in-memory `delete_node(detach=true)` |
| **Delete edge** | `append_wal(DeleteEdge)` → in-memory `delete_edge()` |
| **Create/drop index** | `append_wal(CreateIndex / DropIndex)` → in-memory mutation |
| **Create/drop constraint** | `append_wal(CreateConstraint / DropConstraint)` → in-memory mutation |
| **Create/delete graph** | `append_wal(CreateGraph / DeleteGraph)` → in-memory mutation |

### ID pre-allocation

Create operations require the entity ID before the WAL entry can be written.
`InMemoryGraphStore` exposes two `pub(crate)` methods for this:

```rust
// Allocate a node ID without creating the node.
store.allocate_node_id(graph) -> Result<NodeId, StorageError>

// Allocate an edge ID without creating the edge.
store.allocate_edge_id(graph) -> Result<EdgeId, StorageError>
```

These methods take the write lock, increment the shared sequence counter, and
return the ID. The subsequent `create_node_with_id` / `create_edge_with_id`
uses the pre-allocated ID verbatim. If the WAL write fails, the ID is "leaked"
(a harmless gap in monotonic IDs).

### Detach-delete ordering

When deleting a node with `detach=true`, the persistent store:

1. Reads all out-edge and in-edge IDs (read-only, no state change).
2. Writes `DeleteEdge` WAL entries for every attached edge.
3. Writes the `DeleteNode` WAL entry.
4. Calls `delete_node(graph, id, true)` on the in-memory store.

This ensures all edge deletions are WAL-durable before any in-memory state
changes. On crash-recovery, replaying the WAL re-deletes edges that may
already be gone (idempotent via `skip_if_not_found`).

---

## 2. Constraint persistence

### Guarantee

Uniqueness constraints (and future constraint types) survive process restarts.
They are recorded in the WAL and included in snapshots.

### WAL entry types

```json
{"op": "CreateConstraint", "graph": "g", "label": "Person", "property": "email", "constraint_type": "Unique"}
{"op": "DropConstraint", "graph": "g", "label": "Person", "property": "email"}
```

The `constraint_type` field is the `serde`-serialized form of `ConstraintType`.
Currently only `"Unique"` is defined; the schema is forward-compatible for
future constraint types.

### Replay idempotency

| WAL entry | Replay behavior |
|---|---|
| `CreateConstraint` | Check `list_constraints()` first. If a matching constraint (same label, property, type) already exists, skip. Otherwise call `create_constraint()`. |
| `DropConstraint` | Call `drop_constraint()`. If the constraint does not exist (returns `StorageError::Backend`), swallow the error silently. Other errors propagate. |

This handles the snapshot-plus-WAL overlap case: a constraint created between
two snapshots appears in both the snapshot and the WAL. Replaying both is
harmless.

### Snapshot serialization

Constraints are serialized in the snapshot file between indexes and nodes:

```
CreateGraph → CreateIndex × N → CreateConstraint × N → CreateNode × N → CreateEdge × N
```

Each line uses CRC32C framing (issue #1702).

### Usage

Constraints are managed via openCypher DDL or the `GraphStore` trait:

```cypher
-- Create a uniqueness constraint
CREATE CONSTRAINT ON (p:Person) ASSERT p.email IS UNIQUE;

-- Drop a constraint
DROP CONSTRAINT ON (p:Person) ASSERT p.email IS UNIQUE;
```

```rust
use cypher_core::storage::{GraphStore, ConstraintType};

// Create
store.create_constraint("g", "Person", "email", ConstraintType::Unique)?;

// List
let constraints = store.list_constraints("g")?;
// Returns Vec<ConstraintDescriptor>

// Drop
store.drop_constraint("g", "Person", "email")?;
```

After a restart, `list_constraints` returns the same set that existed before
shutdown.

---

## 3. Atomic snapshots

### Guarantee

Snapshot serialization captures a consistent point-in-time view of all graphs.
Concurrent writers cannot produce a snapshot with partially-applied mutations
(e.g., a node present but its edges missing, or an index without its
corresponding nodes).

### Mechanism

`write_snapshot()` calls `inner.read_snapshot()` exactly once. This method
acquires the `RwLock` read guard, clones the `Arc<HashMap<String, GraphInner>>`
(a cheap refcount bump due to `Arc` copy-on-write), and releases the lock.

The snapshot writer then iterates the frozen `GraphInner` fields directly:

```
for (graph_name, graph_inner) in snapshot.iter() {
    // Write CreateGraph
    // Write indexes from graph_inner.indexes
    // Write constraints from graph_inner.constraints
    // Write nodes from graph_inner.nodes
    // Write edges from graph_inner.edges
}
```

No `GraphStore` trait method calls are made during serialization, so no
further lock acquisitions occur. The `Arc` clone guarantees that concurrent
writers (which call `Arc::make_mut` and get their own copy) do not modify the
snapshot's view.

### Implications

- **Zero writer blocking**: snapshot serialization holds no write lock. Writers
  proceed concurrently without contention from the snapshot thread.
- **Bounded memory**: the snapshot holds one extra `Arc` reference to the
  pre-mutation state. Memory is released when serialization completes and the
  `Arc` is dropped.
- **Deterministic output**: because the snapshot iterates in-memory data
  structures (not live `GraphStore` calls), the output is a faithful image of
  a single point in time.

---

## 4. Entity collision detection

### Guarantee

Creating a node or edge via `InMemoryGraphStore::create_node` /
`InMemoryGraphStore::create_edge` with an explicit `_sd2_id` / `_sd2_eid`
property that collides with an existing entity returns
`StorageError::Backend` instead of silently overwriting the existing entity
and its index entries.

### Scope

| Code path | Collision check? | Rationale |
|---|---|---|
| `create_node` (with `_sd2_id` property) | **Yes** — returns error | User-facing path; silent overwrite = data loss |
| `create_edge` (with `_sd2_eid` property) | **Yes** — returns error | User-facing path; silent overwrite = data loss |
| `create_node_with_id` | **No** — intentionally idempotent | WAL replay path; must be a no-op on re-apply |
| `create_edge_with_id` | **No** — intentionally idempotent | WAL replay path; must be a no-op on re-apply |

The distinction is critical: the `_with_id` variants are the WAL replay
primitives and must remain idempotent (issue #1701). The collision check
applies only to the user-facing `create_node` / `create_edge` methods when
the caller injects an explicit numeric ID via the `_sd2_id` / `_sd2_eid`
property.

### Error format

```
StorageError::Backend {
    msg: "node with _sd2_id=42 already exists in graph 'g'"
}
```

```
StorageError::Backend {
    msg: "edge with _sd2_eid=7 already exists in graph 'g'"
}
```

The error message includes the numeric ID for diagnostics but does not include
property values (PII risk).

### Auto-allocated IDs

When `_sd2_id` / `_sd2_eid` is not present in the properties map, the ID is
auto-allocated from the monotonic sequence counter. Auto-allocated IDs are
guaranteed unique by construction (the counter is `checked_add` with overflow
detection), so no collision check is needed.

---

## Configuration

No new configuration options are introduced. The existing `PersistentConfig`
controls WAL sync behavior and snapshot intervals:

```rust
use cypher_core::storage::persistent::{PersistentConfig, SyncMode};
use std::time::Duration;

let config = PersistentConfig {
    data_dir: "/var/lib/sedandelivery/data".into(),
    snapshot_interval: Duration::from_secs(300),  // 5-minute snapshots
    sync_mode: SyncMode::Immediate,               // fsync every WAL write
};
```

| `SyncMode` | Durability | Performance |
|---|---|---|
| `Immediate` | Every WAL entry is `fsync`'d. No data loss on crash. | Slowest (1 syscall per write) |
| `Periodic(d)` | WAL is flushed every `d`. Up to `d` seconds of data loss on crash. | Balanced |
| `None` | OS page cache only. Data loss window = OS flush interval. | Fastest |

For production deployments that require constraint persistence and WAL-first
ordering guarantees, `SyncMode::Immediate` or `SyncMode::Periodic` with a
short interval (e.g., 1 second) is recommended.

---

## WAL format reference

### Entry types

| `op` | Fields | Since |
|---|---|---|
| `CreateGraph` | `name` | v1 |
| `DeleteGraph` | `name` | v1 |
| `CreateNode` | `graph`, `labels`, `properties`, `id` | v1 |
| `CreateEdge` | `graph`, `src`, `dst`, `rel_type`, `properties`, `id` | v1 |
| `SetNodeProperties` | `graph`, `id`, `properties` | v1 |
| `ReplaceNodeProperties` | `graph`, `id`, `properties` | v1 |
| `SetEdgeProperties` | `graph`, `id`, `properties` | v1 |
| `ReplaceEdgeProperties` | `graph`, `id`, `properties` | v1 |
| `DeleteNode` | `graph`, `id` | v1 |
| `DeleteEdge` | `graph`, `id` | v1 |
| `AddNodeLabels` | `graph`, `id`, `labels` | v1 |
| `RemoveNodeLabels` | `graph`, `id`, `labels` | v1 |
| `RemoveNodeProperty` | `graph`, `id`, `key` | v1 |
| `RemoveEdgeProperty` | `graph`, `id`, `key` | v1 |
| `CreateIndex` | `graph`, `label`, `property` | v1 |
| `DropIndex` | `graph`, `label`, `property` | v1 |
| `CreateConstraint` | `graph`, `label`, `property`, `constraint_type` | v2 (this PR) |
| `DropConstraint` | `graph`, `label`, `property` | v2 (this PR) |

### Framing

Each WAL line uses CRC32C framing (issue #1702):

```
CRC:<hex8> {"op":"CreateNode",...}\n
```

Legacy (v1) lines without the `CRC:` prefix are accepted during replay with a
one-time warning. New lines are always written with the CRC32C prefix.

### Backward compatibility

- Old WAL files (v1, without `CreateConstraint`/`DropConstraint`) replay
  cleanly on new code. Constraints simply won't exist until created.
- New WAL files replayed on old code will fail to parse the unknown `op` tags.
  This is a forward-breaking change: downgrading after creating constraints
  requires WAL truncation via snapshot.

---

## Snapshot format reference

Snapshot files use the same JSON-line format as the WAL. The ordering within
each graph is:

```
CreateGraph
CreateIndex × N       (indexes first — so property indexes are in place before nodes)
CreateConstraint × N  (constraints after indexes, before nodes)
CreateNode × N        (nodes with full properties + labels)
CreateEdge × N        (edges with full properties)
```

All lines use CRC32C framing. Snapshots are written atomically via
write-to-temp-file + `rename(2)`.

---

## Testing

### Constraint persistence tests

| Test | Verifies |
|---|---|
| `constraint_survives_reopen` | Create constraint → drop store → reopen → `list_constraints` returns it |
| `constraint_survives_snapshot_reopen` | Create constraint → `force_snapshot` → write more → reopen → constraint present |
| `drop_constraint_survives_reopen` | Create constraint → drop it → reopen → constraint gone |

### Collision detection tests

| Test | Verifies |
|---|---|
| `create_node_duplicate_sd2_id_rejected` | `create_node` with `_sd2_id=5` twice → second call returns `StorageError::Backend` |
| `create_edge_duplicate_sd2_eid_rejected` | `create_edge` with `_sd2_eid=7` twice → second call returns `StorageError::Backend` |

### Existing tests (unchanged)

All pre-existing tests in `cypher-core/src/storage/persistent/tests.rs`
continue to pass. The WAL-first reordering and snapshot atomicity changes are
transparent to callers because the `GraphStore` trait contract is unchanged.

---

## Migration

### From pre-v2 WAL format

No migration is needed. The WAL format is additive:

- Old snapshots and WAL files replay on new code without modification.
- Constraints simply won't exist until explicitly created.
- CRC32C-framed lines coexist with legacy lines in the same file.

### Forcing a clean snapshot

To eliminate legacy WAL entries and ensure all on-disk state uses the v2
format, force a snapshot:

```rust
// In test or admin context:
store.force_snapshot()?;
```

Or restart the process — the background snapshot thread writes a clean
snapshot at the configured interval and truncates the WAL.

---

## Security considerations

| Concern | Mitigation |
|---|---|
| Silent entity overwrite via `_sd2_id` injection | Collision check returns error; old entity and indexes are preserved |
| PII in error messages | Collision errors include only the numeric ID, never property values |
| WAL bit-flip corruption | CRC32C framing detects single-bit errors; replay fails fast with a clear error |
| Inconsistent snapshot under concurrent writes | `Arc` CoW snapshot guarantees atomicity without holding write locks |
| ID counter overflow | `checked_add` with `StorageError::IdOverflow` at `u64::MAX` |
