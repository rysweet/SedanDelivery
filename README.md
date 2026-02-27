# SedanDelivery

**A Rust GQL/Cypher query engine** — a clean-room, MIT/Apache-2.0-licensed replacement for RedisGraph/FalkorDB.

> **Status**: Working engine — parser, planner, executor, and TCK runner all functional.
> **325 unit tests + 3150/3806 openCypher TCK scenarios (82.8%)**.

**[Documentation Site](https://cloud-ecosystem-security.github.io/SedanDelivery/)**

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
                                                           ├── InMemoryGraphStore (default, tests)
                                                           ├── RedisGraphStore (external Redis/Valkey)
                                                           ├── FabricGraphStore (Microsoft Fabric)
                                                           └── FanoutStore (multi-backend writes)
```

## Getting Started

> **[docs/TUTORIAL.md](docs/TUTORIAL.md)** — usage guide with code examples

The tutorial covers:
- Creating graphs and adding nodes/edges
- Querying with the full pipeline (parse -> plan -> execute)
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

gql-redis-store/   Valkey/Redis-backed GraphStore + FanoutStore (multi-backend writes)
gql-fabric-store/  Microsoft Fabric Graph Database backend (FabricGraphStore)
gql-module/        Valkey module entry point (compiles to .so)
gql-cli/           Interactive demo: cargo run -p gql-cli
docs/              Architecture docs, ADRs, TUTORIAL.md, research notes
infra/             Azure deployment Bicep templates and parameter files
.github/workflows/ CI, deploy, undeploy, and GitHub Pages workflows
```

## Deploying to Azure

SedanDelivery deploys to Azure Container Apps via GitHub Actions.

### Per-PR Environments

1. Add the `deploy` label to any PR to provision an isolated Azure environment
2. The workflow builds a Docker image and pushes it to Azure Container Registry
3. A Container Apps Environment is created with persistent storage (Azure Files)
4. A Valkey container with the GQL module loaded is deployed
5. A bot comment posts the Redis endpoint when deployment completes

### Production

Push to `main` deploys to the production environment automatically.

### Infrastructure

| Resource | Purpose |
|----------|---------|
| Azure Container Registry | Stores the Docker image |
| Azure Storage Account + File Share | Persistent data volume for Valkey RDB/AOF |
| Container Apps Environment | Managed container hosting |
| Container App (Valkey + GQL module) | The deployed Redis with graph query engine |

### Cleanup

Remove the `deploy` label from the PR, or run the **Undeploy** workflow manually.

See the [full deployment documentation](https://cloud-ecosystem-security.github.io/SedanDelivery/) for details.

## Test Results

```
cargo test                325 unit/integration tests (0 failures)
cargo test --test tck     3150/3806 TCK scenarios pass (82.8%)
cargo clippy -- -D warnings   0 errors, 0 warnings
cargo fmt --check         clean
```

**Current TCK pass rate**: 3150/3806 scenarios (82.8%).

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
| `u64` IDs | `u32` wraps silently at 4.29B -> data corruption | docs/RESEARCH.md |

## License

MIT OR Apache-2.0. **No SSPL. No GPL. No LGPL.**

All dependencies are MIT or Apache-2.0.

## Related

- [cybergym#431](https://github.com/cloud-ecosystem-security/cybergym/issues/431) — originating issue
- [openCypher TCK](https://github.com/opencypher/openCypher/tree/main/tck) — conformance test suite
- [Documentation Site](https://cloud-ecosystem-security.github.io/SedanDelivery/) — full docs with deployment guide
- [docs/TUTORIAL.md](docs/TUTORIAL.md) — usage tutorial
