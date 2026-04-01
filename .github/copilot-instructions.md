# SedanDelivery -- Copilot Instructions

## Project Overview

SedanDelivery is a multi-language Rust graph database engine supporting openCypher, Apache TinkerPop Gremlin, and GQL (ISO/IEC 39075). It provides four network interfaces: REST API, Neo4j Bolt v4.4, Gremlin WebSocket, and Redis module commands (`GQL.QUERY`, `GQL.MUTATE`). Storage backends include Redis/Valkey, in-memory with WAL persistence, write-through cache, and Microsoft Fabric.

## Workspace Structure

Cargo workspace with 17 crates:

| Crate | Type | Purpose |
|-------|------|---------|
| `cypher-core` | lib | openCypher parser, planner, executor, `GraphStore` trait, `Value` types |
| `gremlin-core` | lib | Gremlin bytecode compiler + text parser (compiles to cypher-core operators) |
| `gql-engine` | lib | GQL (ISO/IEC 39075) parser + planner |
| `sd2-redis-store` | lib | Redis/Valkey-backed `GraphStore` + `FanoutStore` |
| `gql-fabric-store` | lib | Microsoft Fabric-backed `GraphStore` implementation |
| `sd2-redis-module` | cdylib | Redis module entry point (`redis_module!` macro), `ModuleRedisStore` |
| `gql-cli` | bin | Standalone REPL for development and debugging |
| `sd2-api` | bin | HTTP REST API server |
| `sedandelivery-bolt` | bin | Neo4j Bolt v4.4 protocol server (compatible with all Neo4j drivers) |
| `sedandelivery-gremlin` | bin | Gremlin WebSocket server (bytecode protocol) |
| `sedandelivery-gremlin-server` | bin | Gremlin WebSocket server (text eval protocol) |
| `sedandelivery-fabric-sync` | bin | Fabric replication sidecar (streams changelog to Microsoft Fabric) |
| `sedan-cypher-embedded` | lib | Embeddable in-process openCypher engine |
| `sedan-gql-embedded` | lib | Embeddable in-process GQL/Cypher engine |
| `sedan-gremlin-embedded` | lib | Embeddable in-process Gremlin engine |
| `tests/loadtest` | bin | Load testing harness |
| `tests/gremlin-tck` | bin | Gremlin TCK-equivalent test suite (1672 scenarios) |

## Core Abstractions

- **`GraphStore` trait** (`cypher-core::storage`): The central abstraction. All storage backends implement this trait. Use `InMemoryGraphStore` for tests.
- **`Value` enum** (`cypher-core::types::Value`): The unified value type for all property values, query parameters, and results. Do not introduce `Box<dyn Any>` or custom value wrappers.
- **Serialization**: Use `cypher-core::serde_value` for shared serialization logic between storage backends.

## Coding Conventions

### Build Quality

- `cargo clippy -- -D warnings` on `cypher-core` (enforced in CI)
- `cargo fmt --check` (enforced in CI)
- Zero warnings policy -- treat all clippy lints as actionable

### Style

- Follow standard Rust naming: `snake_case` for functions/variables, `PascalCase` for types, `SCREAMING_SNAKE_CASE` for constants
- Use `thiserror` for error types (not `anyhow` -- this is library code)
- Prefer `Box<str>` over `String` for immutable owned strings in AST nodes
- Use newtype wrappers (`NodeId`, `EdgeId`, `Label`, `RelType`) to prevent ID/type confusion
- Keep `unsafe` to the absolute minimum; justify every usage with a comment

### Testing

- **TCK conformance**: `cargo test --test tck` runs openCypher Technology Compatibility Kit tests
- **Unit tests**: `cargo test -p cypher-core --lib`
- **Integration tests**: `cargo test -p cypher-core --test end_to_end`
- Test the contract (inputs/outputs), not implementation details
- Use `InMemoryGraphStore` for deterministic executor tests

### Redis Module Development

- `sd2-redis-module` is a `cdylib` -- it produces `libsd2_redis_module.so` loaded via `loadmodule`
- Command handlers run on Redis's main thread; keep them fast
- libclang is required at build time for `redis-module` bindgen
- The Dockerfile builds the module in a multi-stage build and packages it with Valkey 8.1

## Dependencies

All dependencies are MIT or Apache-2.0. No GPL, LGPL, or SSPL dependencies are permitted. Shared dependency versions are pinned in the workspace `Cargo.toml` under `[workspace.dependencies]`.

## What NOT to Do

- Do not reference or copy code from FalkorDB (SSPL) or RedisGraph (RSAL) -- this is a clean-room implementation
- Do not introduce `anyhow` in library crates
- Do not add `unsafe` without a justifying comment
- Do not suppress clippy warnings with `#[allow(...)]` without a justifying comment
- Do not use `Box<dyn Any>` for property values -- use `Value` enum
