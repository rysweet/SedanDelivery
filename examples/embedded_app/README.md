# embedded_app — SedanDelivery Embedded Engines Demo

A small CLI that demonstrates how to embed SedanDelivery's three graph
query engines (openCypher, GQL, Apache TinkerPop Gremlin) directly into
a Rust application, sharing a single in-memory graph store.

## Quick start

```bash
cargo run -p embedded-app -- demo
```

## What it shows

| Subcommand | Demonstrates |
|---|---|
| `demo` | Guided tour: seeds a graph, runs equivalent queries through all 3 engines |
| `seed` | Builds a sample social-network graph in `InMemoryGraphStore` |
| `query-cypher <q>` | Runs a Cypher query through `sedan-cypher-embedded` |
| `query-gremlin <q>` | Runs a Gremlin traversal through `sedan-gremlin-embedded` |
| `query-gql <q>` | Runs a GQL query through `sedan-gql-embedded` |
| `compare` | Runs equivalent queries through all 3 engines and asserts result parity |

## How to embed in your own Rust app

### Cypher

```rust
use sedan_cypher_embedded::{CypherEngine, GraphStore, InMemoryGraphStore};

let store = InMemoryGraphStore::default();
store.create_graph("g").unwrap();
let engine = CypherEngine::new();
let result = engine.execute(&store, "g", "MATCH (n) RETURN n").unwrap();
```

### GQL

```rust
use sedan_gql_embedded::{SedanGql, GraphStore, InMemoryGraphStore};

let store = InMemoryGraphStore::default();
store.create_graph("g").unwrap();
let engine = SedanGql::new();
let result = engine.execute(&store, "g", "MATCH (n) RETURN n").unwrap();
```

### Gremlin

```rust
use sedan_gremlin_embedded::{GremlinEngine, GraphStore, InMemoryGraphStore};

let store = InMemoryGraphStore::default();
store.create_graph("g").unwrap();
let engine = GremlinEngine::new();
let result = engine.execute(&store, "g", "g.V()").unwrap();
```

### Custom storage backend

All three engines accept any `&dyn GraphStore` impl. See the workspace's
`sd2-redis-store`, `gql-fabric-store`, etc. for production-grade backends.

## Comparison: when to use which engine

| Engine | Best for | Trade-offs |
|---|---|---|
| Cypher | Pattern-match queries, CRUD | Most mature; full TCK conformance |
| GQL | ISO-standard queries | Newer standard; subset of Cypher's surface |
| Gremlin | Traversal-heavy graph algorithms | Step-based; large library of built-in steps |

All three share the same underlying engine code and storage abstraction —
mutations made through one engine are immediately visible to the others
(see the `compare` subcommand, which exercises this).
