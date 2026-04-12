---
title: Tutorial: Embed SedanDelivery In-Process
description: Embedded quick start for the current in-process Cypher, GQL, and Gremlin crates, including isolation and public error behavior.
last_updated: 2026-04-12
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
- keeps data isolated across separate stores and graph names
- handles representative public errors without relying on internal details

## Prerequisites

- Rust stable
- A Rust application with access to the SedanDelivery embedded crates

## Step 1: Add the Embedded Crates

If your application lives in this repository, add path dependencies for the embedded crates you want. Adjust the relative paths to match your crate location.

```toml
[dependencies]
sedan-cypher-embedded = { path = "../sedan-cypher-embedded" }
sedan-gql-embedded = { path = "../sedan-gql-embedded" }
sedan-gremlin-embedded = { path = "../sedan-gremlin-embedded" }
```

These crates inherit the workspace version from this repository, so repo-local examples should avoid hard-coded version numbers.

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

## Step 6: Use Separate Stores When You Need Hard Isolation

Different `GraphStore` instances are different storage universes.

```rust
use sedan_cypher_embedded::CypherEngine;
use sedan_gql_embedded::{GraphStore, InMemoryGraphStore};

let cypher = CypherEngine::new();
let store_a = InMemoryGraphStore::default();
let store_b = InMemoryGraphStore::default();
store_a.create_graph("demo")?;
store_b.create_graph("demo")?;

cypher.execute(&store_a, "demo", "CREATE (n:Person {name: 'Alice'})")?;
let isolated = cypher.execute(&store_b, "demo", "MATCH (n:Person) RETURN n.name")?;
assert_eq!(isolated.rows.len(), 0);
```

**Checkpoint**: Data written to `store_a` is not visible in `store_b`.

## Step 7: Use Graph Names as Namespaces Inside One Store

One store can host multiple independent graphs. The graph name is part of the public API, not a test-only detail.

```rust
use sedan_gql_embedded::{GraphStore, InMemoryGraphStore, SedanGql};
use sedan_gremlin_embedded::GremlinEngine;

let gql = SedanGql::new();
let gremlin = GremlinEngine::new();
let store = InMemoryGraphStore::default();
store.create_graph("alpha")?;
store.create_graph("beta")?;

gql.execute(&store, "alpha", "INSERT (n:City {name: 'London'})")?;

let beta = gql.execute(&store, "beta", "MATCH (n:City) RETURN n.name")?;
assert_eq!(beta.rows.len(), 0);

let alpha_vertices = gremlin.execute(&store, "alpha", "g.V().hasLabel(\"City\")")?;
let beta_vertices = gremlin.execute(&store, "beta", "g.V().hasLabel(\"City\")")?;
assert_eq!(alpha_vertices.rows.len(), 1);
assert_eq!(beta_vertices.rows.len(), 0);
```

**Checkpoint**: Graph `alpha` and graph `beta` stay independent even though they share the same backing store.

## Step 8: Handle Representative Public Errors

The stable contract is the externally observable failure shape: prefixes, categories, and durable message fragments.

```rust
let missing_graph = cypher.execute(&store, "missing", "RETURN 1 AS n").unwrap_err();
assert!(missing_graph.to_string().contains("storage error:"));
assert!(missing_graph.to_string().contains("does not exist"));

let mandatory = gql
    .execute(&store, "alpha", "MANDATORY MATCH (n:Ghost) RETURN n")
    .unwrap_err();
assert!(mandatory.to_string().to_lowercase().contains("mandatory"));

let invalid_traversal = gremlin.execute(&store, "alpha", "not.a.traversal()").unwrap_err();
assert!(invalid_traversal.to_string().starts_with("parse error:"));
```

**Checkpoint**: You are asserting only public behavior, not internal error variants or IDs.

## Step 9: Put It Together

Here is the current end-to-end example in one file:

```rust
use sedan_cypher_embedded::CypherEngine;
use sedan_gql_embedded::{GraphStore, InMemoryGraphStore, SedanGql};
use sedan_gremlin_embedded::GremlinEngine;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let isolated_a = InMemoryGraphStore::default();
    let isolated_b = InMemoryGraphStore::default();
    isolated_a.create_graph("demo")?;
    isolated_b.create_graph("demo")?;

    let shared = InMemoryGraphStore::default();
    shared.create_graph("alpha")?;
    shared.create_graph("beta")?;

    let cypher = CypherEngine::new();
    cypher.execute(
        &isolated_a,
        "demo",
        "CREATE (n:Person {name: 'Alice', age: 30})",
    )?;

    let isolated = cypher.execute(&isolated_b, "demo", "MATCH (n:Person) RETURN n.name")?;
    assert_eq!(isolated.rows.len(), 0);

    let gql = SedanGql::new();
    gql.execute(&shared, "alpha", "INSERT (n:City {name: 'London'})")?;

    let beta = gql.execute(&shared, "beta", "MATCH (n:City) RETURN n.name")?;
    assert_eq!(beta.rows.len(), 0);

    let gremlin = GremlinEngine::new();
    let alpha = gremlin.execute(&shared, "alpha", "g.V().hasLabel(\"City\")")?;
    let beta = gremlin.execute(&shared, "beta", "g.V().hasLabel(\"City\")")?;
    assert_eq!(alpha.rows.len(), 1);
    assert_eq!(beta.rows.len(), 0);

    let missing_graph = cypher.execute(&shared, "missing", "RETURN 1 AS n");
    assert!(missing_graph.is_err());

    let mandatory = gql.execute(&shared, "alpha", "MANDATORY MATCH (n:Ghost) RETURN n");
    assert!(mandatory.is_err());

    let invalid_traversal = gremlin.execute(&shared, "alpha", "not.a.traversal()");
    assert!(invalid_traversal.is_err());

    Ok(())
}
```

## Summary

This tutorial gives you a current embedded quick start for Cypher, GQL, and Gremlin, plus the isolation and representative error behavior that stays stable across the embedded surface matrix.

## Next Steps

- See [Embedded Engines Reference](../reference/embedded-engines.md) for the API and configuration contract
- See [Product Surface Reference](../reference/product-surfaces.md) for the full form-factor inventory
