# SedanDelivery

**A Rust GQL/Cypher query engine** — MIT/Apache-2.0-licensed replacement for RedisGraph/FalkorDB.

> **Status**: Working engine — parser, planner, executor, and TCK runner all functional.
> **253 tests pass** (237 unit/integration + 6 openCypher TCK cucumber scenarios).

## Quick Start

```bash
# Run the interactive demo (no Redis needed)
cargo run -p gql-cli

# Run all tests
cargo test

# Run the openCypher TCK scenarios
cargo test --test tck
```

## Why

- [FalkorDB](https://github.com/FalkorDB/FalkorDB) is SSPL v1 — a licensing risk for Microsoft products offered as services.
- [RedisGraph](https://github.com/RedisGraph/RedisGraph) is EOL (archived Jan 2024).
- We need a graph database that: shares state across replicas (no single-writer constraint), cold-starts in <1s, and runs under MIT/Apache-2.0.

## What

A Valkey/Redis module (`gql-module.so`) exposing:

```
GQL.QUERY  <graph> "<cypher query>"
GQL.MUTATE <graph> "<cypher mutation>"
GQL.EXPLAIN <graph> "<query>"
GQL.CREATE-GRAPH <graph>
GQL.DELETE-GRAPH <graph>
GQL.LIST-GRAPHS
```

Full openCypher + ISO 20030 GQL compliance target. Wire-protocol-compatible with FalkorDB for `.NET` clients using `StackExchange.Redis`.

## Architecture

```
.NET replicas → StackExchange.Redis → GQL.QUERY → Valkey/Redis Module (Rust)
                                                       ├── Hand-written lexer (zero-copy)
                                                       ├── Recursive descent parser
                                                       ├── Query planner (operator tree)
                                                       ├── Executor (11 operators)
                                                       └── GraphStore trait
                                                           ├── InMemoryGraphStore (tests)
                                                           └── RedisGraphStore (Phase 3)
```

See [PLAN.md](PLAN.md) for the full architecture, storage model, and implementation roadmap.

## Getting Started

→ **[docs/TUTORIAL.md](docs/TUTORIAL.md)** — usage guide with code examples

The tutorial covers:
- Creating graphs and adding nodes/edges
- Querying with the full pipeline (parse → plan → execute)
- Traversal, property predicates, aggregation
- Execution budgets (DoS prevention)
- Property indexes
- DETACH DELETE

## Workspace Layout

```
gql-core/          Pure library — AST, parser, planner, executor, GraphStore trait
                   ├── src/parser/   — hand-written lexer + recursive descent parser
                   ├── src/planner/  — QueryPlanner (AST → Operator tree)
                   ├── src/executor/ — QueryExecutor (11 operators)
                   ├── src/storage/  — GraphStore trait + InMemoryGraphStore
                   ├── tests/        — integration tests + openCypher TCK runner
                   └── benches/      — criterion benchmarks

gql-redis-store/   Valkey/Redis-backed GraphStore (Phase 3)
gql-module/        Valkey module entry point (compiles to .so) (Phase 3)
gql-cli/           Interactive demo: cargo run -p gql-cli
docs/              Architecture docs, ADRs, TUTORIAL.md, research notes
```

## Test Results

```
cargo test -p gql-core --lib        308 tests pass (0 failures)
cargo test -p gql-core --test end_to_end   10 tests pass (0 failures)
cargo test --test tck               3225/3806 TCK scenarios pass (84.5%)
cargo clippy -p gql-core -- -D warnings    0 errors, 0 warnings
```

**Current TCK pass rate**: 3225/3806 (84.5%) — up from 82.7% baseline.

Supported aggregate functions: `count()`, `sum()`, `avg()`, `min()`, `max()`,
`collect()`, `stdev()`, `stdevp()`, `percentileDisc()`, `percentileCont()`.

## Security Properties

All queries require an `ExecutionBudget` (wall-clock deadline + row limit). This prevents
unbounded `MATCH (n)-[*]->(m)` traversals from hanging the process.

Graph names are validated against a whitelist (`[a-zA-Z0-9_.-]`, max 64 chars) to prevent
Redis key namespace injection and cluster hash-tag corruption.

Node/edge IDs use `u64` (not `u32`) to prevent silent overflow at 4.29 billion entities.

See [docs/RESEARCH.md](docs/RESEARCH.md) for the full security analysis.

## Key Design Decisions

| Decision | Rationale | ADR |
|---|---|---|
| Hand-written recursive descent parser | lalrpop can't handle Cypher's `\|` ambiguity (LALR(1) showstopper) | [ADR-003](docs/ADR-003-parser-strategy.md) |
| Redis-native storage (not GraphBLAS) | 8 weeks saved; adequate for ≤1M nodes | [ADR-001](docs/ADR-001-storage-model.md) |
| Full GQL compliance, not a subset | Grammar is cheap; execution defers gracefully | [ADR-002](docs/ADR-002-full-gql-compliance.md) |
| `valkey-module` (not `redis-module`) | Redis 8 is AGPLv3; Valkey is BSD-3-Clause | docs/RESEARCH.md |
| `u64` IDs | `u32` wraps silently at 4.29B → data corruption | docs/RESEARCH.md |

## License

MIT OR Apache-2.0. **No SSPL. No GPL. No LGPL.**

All dependencies are MIT or Apache-2.0. See [PLAN.md §11](PLAN.md#11-dependency-inventory) for the full inventory.

## Related

- [cybergym#431](https://github.com/cloud-ecosystem-security/cybergym/issues/431) — originating issue
- [openCypher TCK](https://github.com/opencypher/openCypher/tree/main/tck) — conformance test suite
- [PLAN.md](PLAN.md) — full implementation plan
- [docs/TUTORIAL.md](docs/TUTORIAL.md) — usage tutorial
