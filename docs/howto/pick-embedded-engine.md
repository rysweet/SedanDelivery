---
title: Pick Your Embedded Engine
description: Decision guide for choosing between sedan-cypher-embedded, sedan-gql-embedded, and sedan-gremlin-embedded.
last_updated: 2026-04-18
review_schedule: as-needed
doc_type: how-to
---

# Pick Your Embedded Engine

SedanDelivery exposes the same graph engine through three Rust crates, each
backing a different query language. They all run **in-process** (no server, no
network), share the same storage trait, and can be combined in a single
application against a single store.

This page helps you pick the right one. For deeper material once you've
decided:

- [Tutorial — Embed SedanDelivery In-Process](../tutorials/embed-sedandelivery.md)
- [Reference — Embedded Engines](../reference/embedded-engines.md)
- [Example — Use all three engines from one Rust binary](../../examples/embedded-consumer)

## Decision matrix

| You want to write… | Pick | Why |
|--------------------|------|-----|
| `MATCH (a)-[:KNOWS]->(b) RETURN ...` | `sedan-cypher-embedded` | openCypher, mature, Neo4j-portable |
| `INSERT (n:Foo {x:1}) RETURN n` (modern GQL syntax) | `sedan-gql-embedded` | ISO/IEC 39075:2024 — the long-term standard |
| `g.V().hasLabel('Person').count()` | `sedan-gremlin-embedded` | Apache TinkerPop Gremlin, traversal-style |
| Multiple of the above | All three | They share storage; mutate via one, read with another |

## When to mix engines

Because all three engines accept any `GraphStore` impl, you can run them
side-by-side against the same data. Common reasons to mix:

- **Migration**: existing Cypher queries while the team learns GQL.
- **Tooling fit**: GQL for batch ETL, Gremlin for interactive exploration.
- **Polyglot teams**: different sub-teams keep their preferred surface.

The [embedded-consumer example](../../examples/embedded-consumer) shows a
single binary that writes via Cypher and reads via Gremlin.

## What you don't get from embedded mode

The embedded crates are intentionally minimal. If you need any of these,
you want the **server** flavours (`sd2-api`, `sedandelivery-bolt`,
`sedandelivery-gremlin`, or the Redis module) instead:

- network access from non-Rust clients (Bolt, Gremlin WebSocket, REST, RESP)
- multi-process write coordination on a shared store (e.g., Redis Cluster)
- Microsoft Fabric replication
- the `gql-cli` REPL (use it with the embedded crates only via subprocess)

See the [Product Surface Reference](../reference/product-surfaces.md) for the
full surface inventory and isolation guarantees.

## Quick start cheat-sheet

```toml
[dependencies]
sedan-cypher-embedded   = "0.1"   # Cypher
sedan-gql-embedded      = "0.1"   # GQL
sedan-gremlin-embedded  = "0.1"   # Gremlin
indexmap                = "2"     # for property maps
```

```rust
let engine = Engine::new();
let store  = InMemoryGraphStore::default();
store.create_graph("g")?;
let result = engine.execute(&store, "g", "<query>")?;
```

That's it. From here, follow the [tutorial](../tutorials/embed-sedandelivery.md)
for the full walkthrough.
