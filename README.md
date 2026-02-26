# SedanDelivery

**A Rust GQL/Cypher query engine over Redis** — a clean-room, MIT/Apache-2.0-licensed replacement for RedisGraph/FalkorDB.

> **Status**: Design & Planning phase. See [PLAN.md](PLAN.md) and [docs/](docs/) for architecture details.

## Why

- [FalkorDB](https://github.com/FalkorDB/FalkorDB) is SSPL v1 — a licensing risk for Microsoft products offered as services.
- [RedisGraph](https://github.com/RedisGraph/RedisGraph) is EOL (archived Jan 2024).
- We need a graph database that: shares state across replicas (no single-writer constraint), cold-starts in <1s, and runs under MIT/Apache-2.0.

## What

A Redis module (`gql-module.so`) that exposes:

```
GQL.QUERY  <graph> "<cypher query>"
GQL.MUTATE <graph> "<cypher mutation>"
GQL.EXPLAIN <graph> "<query>"
GQL.CREATE-GRAPH <graph>
GQL.DELETE-GRAPH <graph>
GQL.LIST-GRAPHS
```

Full openCypher + ISO 20030 GQL compliance. Wire-protocol-compatible with FalkorDB for `.NET` clients using `StackExchange.Redis`.

## Architecture

```
.NET replicas → StackExchange.Redis → GQL.QUERY → Redis Module (Rust)
                                                       ├── Lexer + lalrpop Parser
                                                       ├── Planner (operator tree)
                                                       ├── Executor (BFS/DFS traversal)
                                                       └── Storage (Redis hashes + sorted sets)
```

See [PLAN.md](PLAN.md) for the full architecture, storage model, and implementation roadmap.

## Workspace Layout

```
gql-core/          Pure library — AST, parser, planner, executor, GraphStore trait
gql-redis-store/   Redis-backed GraphStore implementation
gql-module/        Redis module entry point (compiles to .so)
gql-cli/           Standalone REPL for development
tests/             Integration tests (testcontainers), TCK runner, benchmarks
docs/              Architecture docs, ADRs, design decisions
```

## Test Strategy

- **~800 openCypher TCK scenarios** (Apache 2.0) via `cucumber-rs`
- **Property-based tests** via `proptest` (parser never panics)
- **Integration tests** via `testcontainers-rs` (live Redis in Docker)
- **Benchmarks** via `criterion` (<5ms point query, <50ms 2-hop at 100K nodes)

Acceptance criteria: >80% TCK read + write pass rate.

## License

MIT OR Apache-2.0. **No SSPL. No GPL. No LGPL.**

## Related

- [cybergym#431](https://github.com/cloud-ecosystem-security/cybergym/issues/431) — originating issue
- [openCypher TCK](https://github.com/opencypher/openCypher/tree/main/tck)
- [redis-module-rs](https://github.com/RedisLabsModules/redismodule-rs)
