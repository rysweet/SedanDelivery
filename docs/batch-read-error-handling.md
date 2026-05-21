# Batch Read Error Handling — `sd2-redis-store`

> **Issue:** [#1200](https://github.com/cloud-ecosystem-security/SedanDelivery/issues/1200)

When the Redis-backed `GraphStore` reads nodes or edges in batch, every
individual entity in the batch is validated for completeness. Partially-
corrupted entries — where some hash fields are present but others are
missing — are surfaced as `StorageError::Backend` errors instead of being
silently dropped as `None`.

---

## Behavior Summary

| Hash state | Node batch result | Edge batch result |
|---|---|---|
| All fields present | `Some(NodeData)` / `Some(EdgeData)` | ← same |
| All fields `nil` (key absent) | `None` | `None` |
| Fewer than 4 fields returned | n/a (nodes have 2 fields) | `None` (short response) |
| Some fields present, some `nil` | **`Err(StorageError::Backend)`** | **`Err(StorageError::Backend)`** |
| JSON parse failure | `Err(StorageError::Serialization)` | `Err(StorageError::Serialization)` |

The distinction between "absent" and "corrupted" is critical:

- **Absent** (`None`) means the Redis key does not exist. The node/edge was
  never created, or has been deleted. Callers handle this gracefully.
- **Corrupted** (`Err`) means the key exists with incomplete data. This
  indicates a bug, a partially-applied write, or data loss. Callers must
  not silently treat this as "missing."

---

## API Reference

### `decode_node_batch_results`

```rust
pub(crate) fn decode_node_batch_results(
    graph: &str,
    ids: &[NodeId],
    raw: Vec<Vec<Option<String>>>,
) -> Result<Vec<Option<NodeData>>, StorageError>
```

Standalone `pub(crate)` function (not a method on `RedisGraphStore`). Pure
decoder for node batch reads. Takes the raw `HMGET(labels, props)` responses
from a Redis pipeline and decodes each into a `NodeData`, `None`, or an error.

**Parameters:**
- `graph` — the graph name (used in error messages only)
- `ids` — the node IDs corresponding to each pipeline response
- `raw` — one `Vec<Option<String>>` per node, each containing `[labels, props]`

**Returns:**
- `Ok(Vec<Option<NodeData>>)` — one entry per input ID, preserving order
- `Err(StorageError::Backend)` — fail-fast on the first corrupted entry

**Error behavior:** On partial data (e.g., `labels` present but `props`
absent), the function:
1. Emits a `tracing::error!` with `node_id` and `graph` fields
2. Increments the `sd2.backend_errors` counter via `record_backend_error()`
3. Returns `Err(StorageError::Backend { msg })` immediately

### `decode_edge_batch_results`

```rust
pub(crate) fn decode_edge_batch_results(
    ids: &[EdgeId],
    raw: Vec<Vec<Option<String>>>,
) -> Result<Vec<Option<EdgeData>>, StorageError>
```

Standalone `pub(crate)` function. Pure decoder for edge batch reads. Takes
the raw `HMGET(src, dst, rel_type, props)` responses from a Redis pipeline.

**Parameters:**
- `ids` — the edge IDs corresponding to each pipeline response
- `raw` — one `Vec<Option<String>>` per edge, each containing
  `[src, dst, rel_type, props]`

**Returns:**
- `Ok(Vec<Option<EdgeData>>)` — one entry per input ID, preserving order
- `Err(StorageError::Backend)` — fail-fast on the first corrupted entry

**Edge-specific rules:**
- `fields.len() < 4` → `None` (not corruption; this is the response for a
  missing key in some Redis configurations)
- All four fields `None` → `None` (absent edge)
- All four fields `Some(...)` → decode into `EdgeData`
- Mixed present/absent → `Err(StorageError::Backend)` with
  `record_backend_error()`

---

## Observability

Both decode functions integrate with the existing error-metrics system:

| Signal | Where | What |
|---|---|---|
| `tracing::error!` | Structured log | Node errors include `node_id` and `graph`; edge errors include `edge_id` only (no `graph` parameter) |
| `record_backend_error()` | Counter metric | Increments `sd2.backend_errors` — use this to alert on sustained corruption |
| `StorageError::Backend` | Return value | Propagated through `?` to the caller; surfaces in query error responses |

### Monitoring recommendation

Set an alert on `sd2.backend_errors` increasing by more than 0 in a
5-minute window. A single increment during normal operation indicates
data corruption that should be investigated immediately.

---

## Caller Impact

Callers that use `get_nodes_batch_impl` or `get_edges_batch_impl` already
propagate `Result` via `?`. The change from `None` to `Err` for corrupted
entries affects callers that previously pattern-matched on `None` to mean
"node/edge does not exist." After this fix:

- `None` → the entity genuinely does not exist
- `Err(StorageError::Backend)` → the entity's data is corrupted

No public API signature changed. The `GraphStore` trait already declares
batch reads as returning `Result<Vec<Option<T>>, StorageError>`, so callers
already handle the `Err` case.

---

## Testing

Both decoders are pure functions with no Redis dependency, tested with
in-memory data:

### Node batch decode tests

| Test | Scenario |
|---|---|
| `decode_node_batch_partial_labels_only` | Labels present, props absent → `Backend` error |
| `decode_node_batch_partial_props_only` | Props present, labels absent → `Backend` error |
| `decode_node_batch_empty_input` | Zero IDs → empty `Ok(vec![])` |
| `decode_node_batch_multi_node_ordering` | Multiple valid nodes → correct order preserved |
| `decode_node_batch_bad_json` | Malformed JSON in labels → `Serialization` error |
| `decode_node_batch_missing_hash` | All-nil fields → `None` (not error) |

### Edge batch decode tests

| Test | Scenario |
|---|---|
| `decode_edge_batch_returns_none_for_missing_hash` | All-nil → `None` |
| `decode_edge_batch_partial_src_only` | Only `src` present → `Backend` error |
| `decode_edge_batch_partial_missing_props` | `src`+`dst`+`rel_type` present, `props` absent → `Backend` error |
| `decode_edge_batch_all_nil_returns_none` | All four fields `None` → `None` (not error) |

---

## Design Decisions

1. **Fail-fast on first corruption.** When a batch contains a corrupted
   entry, the entire batch returns `Err` immediately. This is consistent
   with the single-entity `read_node` / `read_edge` behavior, which also
   returns `Err` on partial data. Rationale: partial corruption in one
   entity likely indicates a systemic issue; continuing to decode further
   entries risks propagating incorrect data.

2. **Pure decoder extraction.** `decode_node_batch_results` and
   `decode_edge_batch_results` are pure functions taking raw pipeline
   responses. This enables unit testing without a Redis connection and
   keeps the pipeline query separate from result interpretation.

3. **`fields.len() < 4` for edges is `None`, not an error.** Redis returns
   fewer fields when the key is completely absent in certain pipeline
   modes. This matches the "absent" semantics rather than "corrupted."

---

## Related Documentation

- [Silent-Failure Audit](audit/silent-failure-audit.md) — Repository-wide
  audit of error-swallowing patterns; the batch read issue is catalogued there
- [Redis Key Schema](redis-key-schema.md) — Hash field layout for node and
  edge keys
- [Gremlin Error Handling](gremlin-error-handling.md) — Analogous
  no-silent-failure contract in the Gremlin pipeline
- [Storage Correctness](storage-correctness.md) — WAL-first ordering and
  entity integrity guarantees
