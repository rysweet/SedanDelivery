# ADR-001: Redis-Native Storage vs. GraphBLAS Sparse Matrices

**Status**: Accepted
**Date**: 2026-02-26

## Context

FalkorDB uses GraphBLAS (SuiteSparse) sparse adjacency matrices for graph storage and linear-algebra-based query execution. This is very fast for large dense graphs but requires:
- C FFI to SuiteSparse (complex, large binary dependency)
- Or a pure-Rust sparse matrix library (immature ecosystem)

## Decision

Use Redis-native data structures (hashes for nodes/edges, sorted sets for adjacency, sets for label/type indexes) instead of sparse matrices.

## Rationale

At CyberGym's target scale (≤1M nodes, ≤10M edges, security graph topology with ≤5-hop traversals), Redis-native storage provides adequate performance. Measured against acceptance criteria:
- Point query <5ms p99 → achievable with 1 HGETALL
- 2-hop traversal <50ms p99 → achievable with 2× ZRANGE pipeline + HGETALL batch

Benefits:
- Redis handles persistence (RDB/AOF), replication, and Cluster routing for free
- Cold start is instant — data is already in the shared Redis instance
- No C FFI, no unsafe matrix math, no SuiteSparse dependency
- ~8 weeks of saved implementation effort

## Consequences

Multi-hop traversals are Redis-round-trip-bound, not in-memory-computation-bound. For graphs >10M edges or queries requiring >5-hop traversals at low latency, this model would need revisiting. A future migration path: implement a `GraphBLASGraphStore` that satisfies the same `GraphStore` trait without touching the executor.
