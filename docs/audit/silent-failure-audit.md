# Silent-Failure Audit

This document records the repository-wide audit of `unwrap_or_else(|_| ...)`
patterns in production source. The audit was prompted by the cluster pipeline
bug (#1276) where a silent error swallow in `find_nodes_by_label_and_property`
masked a 5-month outage of label+property scans in cluster mode.

## Categories

Every site is one of:

- **`config-default`** — env var or settings file fallback to a documented default. Acceptable: this is config layering, not error swallowing.
- **`infra-bind`** — server bind address / log filter default. Acceptable.
- **`infallible-expect`** — operation that cannot actually fail (e.g. `Vec<String>` serde) but written defensively. Action: replace with `.expect("invariant")` so a bug in the underlying library would panic loudly instead of silently corrupting data.
- **`logged-fallback`** — fallback that emits a structured warn/error log. Acceptable but should be reviewed periodically.
- **`silent-bug`** — eats a real error and returns empty/zero/default. **Always a bug.** Fix by propagating with `?`.
- **`panic-recovery`** — `catch_unwind` in FFI / Redis-module entry points. Required by the FFI boundary.

## Findings

| File:Line | Category | Notes |
|---|---|---|
| `cypher-core/src/config.rs:86` | config-default | env CWD fallback to `"."` |
| `cypher-core/src/config.rs:132` | config-default | csv import dir fallback |
| `sd2-redis-store/src/reads.rs:436` | **silent-bug → fixed in #1279** | label+property scan; was the #1276 trap |
| `sd2-redis-store/src/reads.rs:734` | logged-fallback | `u32::try_from(label_count)` clamp with `tracing::warn!` |
| `sd2-redis-store/src/reads.rs:742` | logged-fallback | `u32::try_from(rel_type_count)` clamp with `tracing::warn!` |
| `gql-fabric-store/src/store/mod.rs:55` | **infallible-expect → fixed in this PR** | `Vec<String>` serde "[]" fallback |
| `gql-fabric-store/src/store/mod.rs:346` | **infallible-expect → fixed in this PR** | same pattern, second site |
| `sd2-redis-module/src/lib.rs:255` | panic-recovery | `catch_unwind` for `GQL.QUERY` |
| `sd2-redis-module/src/lib.rs:301` | panic-recovery | `GQL.MUTATE` |
| `sd2-redis-module/src/lib.rs:340` | panic-recovery | `GQL.CREATE-GRAPH` |
| `sd2-redis-module/src/lib.rs:374` | panic-recovery | `GQL.DELETE-GRAPH` |
| `sd2-redis-module/src/lib.rs:408` | panic-recovery | `GQL.LIST-GRAPHS` |
| `sd2-redis-module/src/lib.rs:434` | panic-recovery | `GQL.EXPLAIN` |
| `sedandelivery-bolt/src/health.rs:57` | infra-bind | `BOLT_HEALTH_BIND_ADDR` default |
| `sedandelivery-bolt/src/health.rs:144` | logged-fallback | Prometheus response builder fallback (returns 500 with body) |
| `sedandelivery-bolt/src/main.rs:91` | infra-bind | `EnvFilter` default `info` |
| `sedandelivery-bolt/src/main.rs:147` | infra-bind | `BOLT_BIND_ADDR` default |
| `sedandelivery-bolt/src/protocol/handler/hello.rs:379` | infra-bind | `BOLT_SERVER_VERSION` default |
| `sedandelivery-gremlin/src/health.rs:43` | infra-bind | `GREMLIN_HEALTH_BIND_ADDR` default |
| `sedandelivery-gremlin/src/health.rs:46` | logged-fallback | invalid bind addr → `0.0.0.0:port` with `tracing::warn!` |
| `sedandelivery-gremlin/src/main.rs:33` | infra-bind | `EnvFilter` default |
| `sedandelivery-gremlin-server/src/main.rs:33` | infra-bind | `EnvFilter` default |
| `sedandelivery-fabric-sync/src/main.rs:43` | infra-bind | `EnvFilter` default |
| `sedandelivery-fabric-sync/src/main.rs:64` | config-default | `SD2_SYNC_CONSUMER` default |
| `sedandelivery-fabric-sync/src/main.rs:79` | config-default | `GQL_REDIS_URL` default |
| `sedandelivery-fabric-sync/src/sync_loop.rs:18` | config-default | DLQ stream name default |
| `sd2-api/src/main.rs:64` | infra-bind | `EnvFilter` default |
| `sd2-api/src/main.rs:263` | infra-bind | `0.0.0.0:3001` bind default |
| `sd2-api/src/metrics.rs:97` | logged-fallback | Prometheus response builder fallback |

**Total surveyed:** 29 sites
**Real bugs found:** 1 (already fixed in #1276 + locked down in #1279)
**Defensive-but-wrong patterns:** 2 (fixed in this PR)
**Acceptable:** 26

## Followups

- `unwrap_or_default()` (54 sites) and bare `let _ = ...` (31 sites) were not
  individually classified in this pass. They are typically benign (e.g.
  `Default::default()` for a builder pattern, or deliberate result discard
  for fire-and-forget logs), but a sample audit confirmed no obvious bugs.
  A future pass can add a clippy lint to require `// reason:` comments
  on these patterns in production crates.

- Consider adopting [`clippy::manual_let_else`] and a project lint
  policy that flags `unwrap_or_else(|_| <default>)` in PR review
  (no clippy lint covers this exact pattern as of 1.95).

## Reproducer for the historical bug

The bug fixed in #1276 only triggered in cluster mode, and was masked by:

```rust
let props_results: Vec<Option<String>> = pipe
    .query(&mut conn)
    .unwrap_or_else(|_| vec![None; node_ids.len()]);
```

The `pipe` was built via `redis::pipe()` instead of `conn.pipeline()`,
which silently returns no results against a `ClusterConnection`. The
`unwrap_or_else` then turned that empty result into "no nodes match",
which propagated up as missing edges in `MATCH ... CREATE` statements.

The regression test added in #1279 (`cluster_mode_label_property_scan_without_index_finds_match`) prevents this class of bug returning to the no-index fallback path.
