---
title: Embedded Engines Reference
description: Current public API and wiring for the SedanDelivery embedded crates.
last_updated: 2026-04-11
review_schedule: as-needed
doc_type: reference
---

# Embedded Engines Reference

## Overview

SedanDelivery exposes three in-process libraries:

| Crate | Language surface | Current wiring | Shared storage contract |
| --- | --- | --- | --- |
| `sedan-cypher-embedded` | openCypher | `cypher-core` parser -> planner -> executor | `GraphStore` |
| `sedan-gql-embedded` | GQL/Cypher | `gql-engine` -> `cypher-core` | `GraphStore` |
| `sedan-gremlin-embedded` | Gremlin text traversals | `gremlin-core` -> `cypher-core` | `GraphStore` |

For the broader product inventory and the non-embedded comparison baselines, see the [Product Surface Reference](./product-surfaces.md).

## Common Types

The embedded crates use the same store and result vocabulary as the server and module runtimes.

| Type | Meaning |
| --- | --- |
| `GraphStore` | Storage abstraction implemented by in-memory, Redis/Valkey, cached, and Fabric-backed stores |
| `InMemoryGraphStore` | Default in-process store for tests, demos, and single-process embedding |
| `QueryResult` | Result metadata, columns, rows, and mutation statistics |
| `Row` | Single result row |
| `MutationStats` | Counts for created/updated/deleted graph elements |
| `Value` | Unified scalar/list/map value type used across all query surfaces |

## Public API

### `sedan_cypher_embedded`

#### `CypherEngine::new() -> CypherEngine`

Creates a new in-process openCypher engine.

#### `CypherEngine::execute(&self, store: &dyn GraphStore, graph: &str, query: &str) -> Result<QueryResult, cypher_core::error::GqlError>`

Parses, plans, and executes an openCypher query against an existing graph through `cypher-core`.

### `sedan_gql_embedded`

#### `SedanGql::new() -> SedanGql`

Creates a new in-process GQL engine.

#### `SedanGql::execute(&self, store: &dyn GraphStore, graph: &str, query: &str) -> Result<QueryResult, gql_engine::GqlError>`

Parses, plans, and executes a GQL or Cypher-compatible query through `gql-engine`.

#### `sedan_gql_embedded::GqlError`

This is a re-export of `gql_engine::GqlError`.

| Variant | Source |
| --- | --- |
| `Parse` | GQL parser failures |
| `Plan` | GQL planning failures |
| `Execution` | Shared executor/runtime failures |

### `sedan_gremlin_embedded`

#### `GremlinEngine::new() -> GremlinEngine`

Creates a new in-process Gremlin engine.

#### `GremlinEngine::execute(&self, store: &dyn GraphStore, graph: &str, gremlin_query: &str) -> Result<QueryResult, GremlinError>`

Parses a Gremlin traversal string, compiles it through `gremlin-core`, and executes it with the shared executor.

#### `GremlinError`

| Variant | Source |
| --- | --- |
| `Parse` | Gremlin text parsing failures |
| `Compile` | Gremlin-to-operator compilation failures |
| `Execution` | Shared executor/runtime failures |

## Behavior Contract

| Query shape | `CypherEngine` | `SedanGql` | `GremlinEngine` |
| --- | --- | --- | --- |
| openCypher `CREATE` / `MATCH` / `RETURN` | Yes | Yes | No |
| GQL `INSERT` | No | Yes | No |
| GQL `FILTER` / `FILTER WHERE` | No | Yes | No |
| GQL `MANDATORY MATCH` | No | Yes | No |
| GQL `FOR x IN [...] RETURN x` | No | Yes | No |
| Gremlin `g.V().has(...).values(...)` | No | No | Yes |

`SedanGql` follows `gql-engine` behavior, including current support for GQL-only keywords and current `gql-engine` limitations such as incomplete path-mode enforcement.

## Configuration

### Required setup

1. Construct a `GraphStore`.
2. Create the target graph before running queries.
3. Reuse the store across engine calls when you want shared state inside the process.

```rust
use sedan_gql_embedded::{GraphStore, InMemoryGraphStore};

let store = InMemoryGraphStore::default();
store.create_graph("app").unwrap();
```

### Runtime configuration model

Embedded crates do not require server-specific deployment configuration, but they do inherit the process-wide engine limits defined in `cypher-core`.

| Topic | Embedded behavior |
| --- | --- |
| Environment variables | No server-specific env vars are required, but engine limit env vars such as `GQL_QUERY_TIMEOUT_MS`, `GQL_QUERY_MAX_ROWS`, `GQL_PLAN_CACHE_SIZE`, and `GQL_BUDGET_CHECK_INTERVAL` still affect the process |
| Ports and listeners | None |
| Background tasks | None |
| Authentication | Caller-owned concern |
| Storage backend choice | Determined by the `GraphStore` implementation you pass in |

Server-only variables such as `BOLT_BIND_ADDR` and `GQL_API_BIND_ADDR` belong to the runtime form factors, not the embedded library surface.

## Packaging and Runtime Presence

| Concern | Current state |
| --- | --- |
| Cargo packaging | Library crates in the main workspace |
| Docker images | Not packaged as standalone container entrypoints |
| Compose profiles | Not started directly by `docker-compose*.yml` |
| GitHub deploy workflows | Not deployed as independent services |
| CI and local validation | Covered by dedicated embedded-wrapper and native-GQL contract commands in `ci.yml` and `scripts/validate.sh` |

## Cross-Form-Factor Baselines

| Embedded surface | Baseline used to detect drift |
| --- | --- |
| `sedan-cypher-embedded` | `gql-cli`, `sd2-api`, `sedandelivery-bolt`, `sd2-redis-module` |
| `sedan-gql-embedded` | `gql-engine`, plus shared Cypher-contract behavior exercised by `gql-cli`, `sd2-api`, `sedandelivery-bolt`, and `sd2-redis-module` |
| `sedan-gremlin-embedded` | `sedandelivery-gremlin`, `sedandelivery-gremlin-server` |

## Validation

```bash
cargo check --workspace
cargo test -p cypher-core --test end_to_end
cargo test -p sedan-cypher-embedded -p sedan-gql-embedded -p sedan-gremlin-embedded
cargo test -p gql-engine --test outside_in
```

## Examples

### Cypher

```rust
use sedan_cypher_embedded::{CypherEngine, GraphStore, InMemoryGraphStore, Value};

let engine = CypherEngine::new();
let store = InMemoryGraphStore::default();
store.create_graph("g").unwrap();

let result = engine
    .execute(&store, "g", "RETURN 42 AS answer")
    .unwrap();

assert_eq!(result.rows[0].get("answer"), Some(&Value::Int(42)));
```

### GQL

```rust
use sedan_gql_embedded::{GraphStore, InMemoryGraphStore, SedanGql, Value};

let engine = SedanGql::new();
let store = InMemoryGraphStore::default();
store.create_graph("g").unwrap();

engine
    .execute(&store, "g", "INSERT (n:City {name: 'London', pop: 9000000})")
    .unwrap();

let result = engine
    .execute(&store, "g", "MATCH (n:City) FILTER n.pop > 1000000 RETURN n.name")
    .unwrap();

assert_eq!(result.rows[0].get("n.name"), Some(&Value::Str("London".into())));
```

### Gremlin

```rust
use sedan_gremlin_embedded::{GraphStore, GremlinEngine, InMemoryGraphStore};

let engine = GremlinEngine::new();
let store = InMemoryGraphStore::default();
store.create_graph("g").unwrap();

let result = engine
    .execute(&store, "g", "g.V().count()")
    .unwrap();

assert_eq!(result.rows.len(), 1);
```

## See Also

- [Product Surface Reference](./product-surfaces.md)
- [Tutorial: Embed SedanDelivery In-Process](../tutorials/embed-sedandelivery.md)
