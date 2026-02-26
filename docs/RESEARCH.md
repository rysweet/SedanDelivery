# Research Findings

*Captured from session 2026-02-26 — pre-implementation investigation phase.*

## Source Issue

[cybergym#431](https://github.com/cloud-ecosystem-security/cybergym/issues/431): "Future: Rust GQL Engine over Redis as alternative graph backend"

**Core motivation**: LadybugDB (Kuzu fork, embedded C++) has a single-writer constraint. Multi-replica writes require distributed lock coordination. A Redis-backed engine eliminates this.

---

## RedisGraph / FalkorDB Architecture

### Storage Model
- FalkorDB uses **sparse adjacency matrices** (GraphBLAS / SuiteSparse) to represent graphs
- Each relationship type gets its own dedicated matrix
- Node label membership uses a symmetric diagonal matrix
- This enables linear-algebra-based query execution (very fast for dense graphs at large scale)

### Our Decision: Redis-Native Storage (NOT GraphBLAS)
At CyberGym's scale (≤1M nodes, security graph topology), Redis-native adjacency lists provide adequate performance with dramatically less complexity:
- GraphBLAS adds ~8 weeks of FFI complexity for negligible benefit
- Redis-native storage gets persistence, replication, AOF for free
- Cold start is instant (data already in Redis, shared across replicas)

### Key Namespace (per graph `G`)
| Key | Type | Contents |
|-----|------|----------|
| `G:meta` | Hash | node_count, edge_count, schema_version |
| `G:node:{id}` | Hash | `_labels`, all properties |
| `G:edge:{id}` | Hash | `_type`, `_src`, `_dst`, properties |
| `G:out:{nodeId}` | Sorted Set | `{edgeId}:{targetId}:{relType}` |
| `G:in:{nodeId}` | Sorted Set | `{edgeId}:{sourceId}:{relType}` |
| `G:label:{L}` | Set | Node IDs with label L |
| `G:reltype:{T}` | Set | Edge IDs of type T |
| `G:seq:nodes` | String | Auto-increment counter |
| `G:seq:edges` | String | Auto-increment counter |

---

## Licensing Landscape

| Project | License | Safe? |
|---------|---------|-------|
| RedisGraph | Redis Source Available License | Read-only reference OK; cannot ship |
| FalkorDB | SSPL v1 | **Poisoned — do not read code** |
| FalkorDB-core-rs | SSPL v1 | **Poisoned — do not read code** |
| openCypher TCK | Apache 2.0 | ✅ Safe to adapt |
| opengql/grammar | Apache 2.0 | ✅ Safe to use |
| redis-module-rs | MIT | ✅ Safe |
| lalrpop | Apache 2.0 | ✅ Safe |

**Critical**: FalkorDB-core-rs (SSPL) exists but must not be read or referenced. Clean-room implementation only.

---

## Parser Strategy

**Chosen**: Hand-written lexer (zero-copy, `&'src str` tokens) + `lalrpop` LALR(1) grammar

**Rejected**:
- `nom`: Becomes unreadable at Cypher's scale; no left-recursion support
- `pest`: Allocates heavily via `Pairs`; no typed AST generation
- Hand-written recursive descent: 3-4 weeks more effort; reserved for if lalrpop proves insufficient for GQL's grammar

**Key insight**: Full GQL grammar in lalrpop is feasible. The grammar accepts everything from day one; execution defers unsupported features with `UnsupportedFeature { feature }` — never a parse error for valid GQL.

---

## Rust Crate Decisions

| Purpose | Chosen Crate | Version | Rationale |
|---------|-------------|---------|-----------|
| Redis module SDK | `redis-module` | 0.24 | Official RedisLabs crate, most production-tested |
| Parser generator | `lalrpop` + `lalrpop-util` | 0.20 | LALR(1), typed AST, operator precedence |
| Error types | `thiserror` | 1.0 | Library context: structured errors, not `anyhow` |
| Arena allocation | `typed-arena` | 2.0 | Zero-dealloc query execution contexts |
| Fast hash maps | `rustc-hash` | 2.0 | FxHashSet/FxHashMap for node ID sets |
| Ordered maps | `indexmap` | 2.0 | Cypher Map literals preserve insertion order |
| SIMD | `wide` | 0.7 | Stable (no nightly), property batch filtering |
| Graph algorithms | `petgraph` | 0.6 | Algorithms only; not primary storage |
| Integration tests | `testcontainers-modules` | 0.11 | Redis in Docker |
| Property tests | `proptest` | 1.6 | Parser fuzzing, invariant checking |
| BDD tests | `cucumber` | 0.21 | openCypher TCK runner |
| Benchmarks | `criterion` | 0.5 | Latency target validation |
| Snapshot tests | `insta` | 1.40 | AST regression testing |

---

## Key Architectural Decisions

### GraphStore Trait — The Critical Abstraction Boundary
All executor code operates against `trait GraphStore`. Two implementations:
1. `InMemoryGraphStore` — for unit/integration tests, no Redis dependency
2. `RedisGraphStore` — production, uses Redis context from module API

This means >80% of the engine can be tested without Docker or a live Redis instance.

### Value Type — No `Box<dyn Any>`
```rust
pub enum Value {
    Null, Bool(bool), Int(i64), Float(f64),
    Str(Box<str>),   // 16 bytes vs String's 24
    List(Vec<Value>),
    Map(indexmap::IndexMap<Box<str>, Value>),
    Node(NodeId), Edge(EdgeId), Path(Vec<NodeId>, Vec<EdgeId>),
}
```

### Concurrency Model
Redis is single-threaded for command handlers. No `Arc<Mutex<...>>` on hot path. Long queries use `RedisModule_BlockClient` + background thread.

### Property Encoding
Typed prefix in Redis hash fields: `i:{int}`, `f:{float}`, `s:{string}`, `b:1`/`b:0`, `n:` (null).

---

## Multi-Agent Reviews Spawned (session crashed before results collected)

Five review agents were running when the session crashed:
1. **Architecture review** — storage model, operator tree, Redis module constraints
2. **Rust expert review** — crate validation, memory safety, parser feasibility
3. **Security review** — injection vectors, DoS, SSPL audit, atomicity
4. **Test strategy review** — TCK integration, property tests, CI design
5. **Deep research** — existing Rust Cypher parsers, lalrpop grammar size, Valkey

Results were not captured. These reviews need to be re-run before implementation begins.

---

## Open Questions (Stakeholder Input Needed)

1. **Redis version**: Require 7.4+, or support 6.x?
2. **Valkey**: Should we target Valkey (Linux Foundation fork) instead of / in addition to Redis 8 (AGPLv3)?
3. **CALL subqueries**: Required for Phase 1?
4. **Multi-tenancy model**: Separate Redis DB per graph, or key prefix in DB 0?
5. **Cluster support**: Redis Cluster required in Phase 1?
6. **Parquet sync**: Push or pull model for OneLake sync?
