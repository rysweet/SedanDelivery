# SedanDelivery -- Copilot Instructions

## Project Overview

SedanDelivery is a Rust GQL/Cypher query engine that runs as a Redis/Valkey module. It provides `GQL.QUERY` and `GQL.MUTATE` commands for graph operations over Redis-native data structures.

## Workspace Structure

Cargo workspace with 5 crates:

| Crate | Type | Purpose |
|-------|------|---------|
| `gql-core` | lib | Parser, planner, executor, `GraphStore` trait, `Value` types |
| `gql-redis-store` | lib | Redis-backed `GraphStore` implementation |
| `gql-fabric-store` | lib | Microsoft Fabric-backed `GraphStore` implementation |
| `gql-module` | cdylib | Redis module entry point (`redis_module!` macro) |
| `gql-cli` | bin | Standalone REPL for development and debugging |

## Core Abstractions

- **`GraphStore` trait** (`gql-core::storage`): The central abstraction. All storage backends implement this trait. Use `InMemoryGraphStore` for tests.
- **`Value` enum** (`gql-core::types::Value`): The unified value type for all property values, query parameters, and results. Do not introduce `Box<dyn Any>` or custom value wrappers.
- **Serialization**: Use `gql-core::serde_value` for shared serialization logic between storage backends.

## Coding Conventions

### Build Quality

- `cargo clippy -- -D warnings` on `gql-core` (enforced in CI)
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
- **Unit tests**: `cargo test -p gql-core --lib`
- **Integration tests**: `cargo test -p gql-core --test end_to_end`
- Test the contract (inputs/outputs), not implementation details
- Use `InMemoryGraphStore` for deterministic executor tests

### Redis Module Development

- `gql-module` is a `cdylib` -- it produces `libgql_module.so` loaded via `loadmodule`
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
