---
title: Tutorial: Embed SedanDelivery In-Process
description: Embedded quick start for the current in-process Cypher, GQL, and Gremlin crates.
last_updated: 2026-04-11
review_schedule: as-needed
doc_type: tutorial
---

# Tutorial: Embed SedanDelivery In-Process

This tutorial shows the current embedded experience across the three in-process crates.

## What You'll Build

A single-process Rust program that:

- creates an in-memory graph
- writes data with Cypher and GQL
- reads the same graph with Gremlin

## Prerequisites

- Rust stable
- A Rust application with access to the SedanDelivery embedded crates

## Step 1: Add the Embedded Crates

Add the crates you want to your `Cargo.toml`.

```toml
[dependencies]
sedan-cypher-embedded = "0.1.0"
sedan-gql-embedded = "0.1.0"
sedan-gremlin-embedded = "0.1.0"
```

## Step 2: Create a Shared Graph Store

All three embedded engines operate on a caller-supplied `GraphStore`. Start with the in-memory implementation.

```rust
use sedan_gql_embedded::{GraphStore, InMemoryGraphStore};

let store = InMemoryGraphStore::default();
store.create_graph("demo")?;
```

**Checkpoint**: You now have an empty graph named `demo` inside the current process.

## Step 3: Run openCypher In-Process

Use `CypherEngine` for the Cypher-only surface.

```rust
use sedan_cypher_embedded::CypherEngine;

let cypher = CypherEngine::new();

cypher.execute(
    &store,
    "demo",
    "CREATE (n:Person {name: 'Alice', age: 30}) RETURN n.name",
)?;
```

**Checkpoint**: The graph now contains one `Person` node created through the Cypher embedded engine.

## Step 4: Use `SedanGql` for Native GQL Queries

`SedanGql` is the embedded GQL surface. It accepts GQL-only keywords while preserving shared Cypher compatibility.

```rust
use sedan_gql_embedded::{SedanGql, Value};

let gql = SedanGql::new();

gql.execute(
    &store,
    "demo",
    "INSERT (n:Person {name: 'Bob', age: 25}) RETURN n.name",
)?;

let shared = gql.execute(
    &store,
    "demo",
    "MATCH (n:Person) FILTER n.age >= 25 RETURN n.name",
)?;
assert_eq!(shared.rows.len(), 2);
let unwound = gql.execute(&store, "demo", "FOR x IN [1, 2, 3] RETURN x")?;
assert_eq!(unwound.rows.len(), 3);

let mandatory = gql.execute(
    &store,
    "demo",
    "MANDATORY MATCH (n:Person {name: 'Alice'}) RETURN n.name",
)?;
assert_eq!(
    mandatory.rows[0].get("n.name"),
    Some(&Value::Str("Alice".into()))
);
```

**Checkpoint**: `SedanGql` now exposes the native embedded GQL keyword surface through `gql-engine`.

## Step 5: Query the Same Data with Gremlin

The Gremlin embedded engine works against the same store.

```rust
use sedan_gremlin_embedded::GremlinEngine;

let gremlin = GremlinEngine::new();

let result = gremlin.execute(
    &store,
    "demo",
    "g.V().has(\"name\",\"Alice\").values(\"name\")",
)?;

assert_eq!(result.rows.len(), 1);
```

**Checkpoint**: The same in-process graph is now reachable from Cypher, GQL, and Gremlin entry surfaces.

## Step 6: Put It Together

Here is the current end-to-end example in one file:

```rust
use sedan_cypher_embedded::CypherEngine;
use sedan_gql_embedded::{GraphStore, InMemoryGraphStore, SedanGql, Value};
use sedan_gremlin_embedded::GremlinEngine;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let store = InMemoryGraphStore::default();
    store.create_graph("demo")?;

    let cypher = CypherEngine::new();
    cypher.execute(
        &store,
        "demo",
        "CREATE (n:Person {name: 'Alice', age: 30}) RETURN n.name",
    )?;

    let gql = SedanGql::new();
    gql.execute(
        &store,
        "demo",
        "INSERT (n:Person {name: 'Bob', age: 25}) RETURN n.name",
    )?;

    let filtered = gql.execute(
        &store,
        "demo",
        "MATCH (n:Person) FILTER n.age >= 25 RETURN n.name",
    )?;
    assert_eq!(filtered.rows.len(), 2);

    let mandatory = gql.execute(
        &store,
        "demo",
        "MANDATORY MATCH (n:Person {name: 'Alice'}) RETURN n.name",
    )?;
    assert_eq!(
        mandatory.rows[0].get("n.name"),
        Some(&Value::Str("Alice".into()))
    );

    let gremlin = GremlinEngine::new();
    let gremlin_result = gremlin.execute(
        &store,
        "demo",
        "g.V().has(\"name\",\"Alice\").values(\"name\")",
    )?;
    assert_eq!(gremlin_result.rows.len(), 1);

    Ok(())
}
```

## Summary

This tutorial gives you a current embedded quick start for Cypher, GQL, and Gremlin over the same in-process graph.

## Next Steps

- See [Embedded Engines Reference](../reference/embedded-engines.md) for the API and configuration contract
- See [Product Surface Reference](../reference/product-surfaces.md) for the full form-factor inventory
