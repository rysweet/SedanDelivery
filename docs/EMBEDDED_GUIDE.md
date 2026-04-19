# Embedded Library Usage Guide

SedanDelivery ships **three** embeddable, in-process query engines as ordinary
Rust crates. Drop one (or more) into your `Cargo.toml`, point it at an
`InMemoryGraphStore`, and you have a graph database in your binary — no
sidecar service, no network hop.

| Crate | Query language | Spec | Use when |
|---|---|---|---|
| [`sedan-cypher-embedded`](../sedan-cypher-embedded) | openCypher | Neo4j-style `MATCH`/`CREATE`/`MERGE` | You already know Cypher or are porting from Neo4j |
| [`sedan-gql-embedded`](../sedan-gql-embedded) | GQL (ISO/IEC 39075) | International standard, lowercase keywords, `INSERT` | You want a vendor-neutral standard query language |
| [`sedan-gremlin-embedded`](../sedan-gremlin-embedded) | Apache TinkerPop Gremlin | Functional traversal DSL | You're porting from JanusGraph / Neptune / TinkerPop, or prefer a fluent step API |

All three share the same storage layer (`cypher-core`'s `GraphStore` trait)
and value model (`Value` enum), so you can mix engines against the same
in-memory graph if you want.

---

## Choosing a crate

| Question | Pick |
|---|---|
| "I want `CREATE` / `MATCH`." | **cypher** |
| "I need ISO-standard `INSERT` semantics." | **gql** |
| "I think in `g.V().has(...).out(...)`." | **gremlin** |
| "I just want the smallest possible dependency surface." | **cypher** (one transitive crate) |
| "I want compile-once, execute-many." | **gremlin** (`compile` / `execute_compiled`) or **cypher** (built-in plan cache) |

You can mix-and-match: all three crates execute against the same
`InMemoryGraphStore`, so a sophisticated app can ingest with Gremlin and
analyse with Cypher against one graph.

---

## Adding the dependency

Pick **one** of the three. While the workspace is unpublished, use a `path`
or `git` dependency.

```bash
# From inside the workspace
cargo add --path ./sedan-cypher-embedded
cargo add --path ./sedan-gql-embedded
cargo add --path ./sedan-gremlin-embedded
```

`Cargo.toml` snippet:

```toml
[dependencies]
sedan-cypher-embedded  = { git = "https://github.com/cloud-ecosystem-security/SedanDelivery" }
# or
sedan-gql-embedded     = { git = "https://github.com/cloud-ecosystem-security/SedanDelivery" }
# or
sedan-gremlin-embedded = { git = "https://github.com/cloud-ecosystem-security/SedanDelivery" }

# Gremlin examples need indexmap to seed nodes through the storage API
indexmap = "2"
```

---

## Minimal example

The pattern is the same across all three crates:

1. Build the engine.
2. Build an `InMemoryGraphStore` and call `create_graph(name)`.
3. Run queries with `engine.execute(&store, graph_name, query_text)`.

```rust
// sedan-cypher-embedded
use sedan_cypher_embedded::{CypherEngine, GraphStore, InMemoryGraphStore, Value};

let engine = CypherEngine::new();
let store = InMemoryGraphStore::default();
store.create_graph("g").unwrap();

engine.execute(&store, "g",
    "CREATE (:Person {name: 'Alice', age: 30}), (:Person {name: 'Bob', age: 25})"
).unwrap();

let r = engine.execute(&store, "g",
    "MATCH (p:Person) RETURN count(p) AS n, avg(p.age) AS avg_age"
).unwrap();
assert_eq!(r.rows[0]["n"], Value::Int(2));
```

GQL is essentially the same with `SedanGql` and `INSERT`:

```rust
use sedan_gql_embedded::prelude::*;
let engine = SedanGql::new();
// store / create_graph as above
engine.execute(&store, "g",
    "INSERT (:Person {name: 'Alice'}), (:Person {name: 'Bob'})"
).unwrap();
```

Gremlin uses traversal strings and (typically) seeds via the storage API:

```rust
use indexmap::IndexMap;
use sedan_gremlin_embedded::prelude::*;

let engine = GremlinEngine::new();
let store = InMemoryGraphStore::default();
store.create_graph("g").unwrap();
store.create_node(
    "g",
    vec!["Person".into()],
    IndexMap::from([("name".into(), Value::Str("Alice".into()))]),
).unwrap();

let r = engine.execute(&store, "g", "g.V().count()").unwrap();
```

---

## Sophisticated patterns

The runnable examples in each crate's `examples/` directory exercise these
patterns end-to-end. Run any of them with:

```bash
cargo run --example quickstart    -p sedan-cypher-embedded
cargo run --example sophisticated -p sedan-cypher-embedded
cargo run --example quickstart    -p sedan-gql-embedded
cargo run --example sophisticated -p sedan-gql-embedded
cargo run --example quickstart    -p sedan-gremlin-embedded
cargo run --example sophisticated -p sedan-gremlin-embedded
```

### Parameter binding

Never interpolate user input into a query string. Use parameters (`$name`):

```rust
// Cypher / GQL
let mut params = HashMap::new();
params.insert("min_age".into(), Value::Int(28));

// Cypher
engine.execute_with_parameters(&store, "g",
    "MATCH (p:Person) WHERE p.age >= $min_age RETURN p.name",
    &params, &ProcedureRegistry::new())?;

// GQL
engine.execute_with_context(&store, "g",
    "MATCH (p:Person) WHERE p.age >= $min_age RETURN p.name",
    &ProcedureRegistry::default(), params)?;
```

Gremlin parameter substitution is currently done by passing a
`|name| -> Option<GremlinValue>` callback to `gremlin-core::parse_traversal`;
the high-level `GremlinEngine::execute` does not yet expose this hook, so for
parameterised Gremlin work today, prefer the lower-level `gremlin-core` API.

### Error handling

Every engine returns a structured error enum:

```rust
match engine.execute(&store, "g", query) {
    Ok(result)                          => use_rows(result.rows),
    Err(GqlError::Parse(e))             => /* syntax error */,
    Err(GqlError::Plan(e))              => /* unknown function, type mismatch */,
    Err(GqlError::Execution(e))         => /* runtime: budget, division by zero */,
    Err(GqlError::Storage(e))           => /* missing graph, conflicting write */,
    Err(GqlError::Type(e))              => /* unsupported value coercion */,
}
```

`GremlinEngine` wraps these in its own enum (`GremlinError::{Parse, Compile,
Execution}`) — `Execution` carries the inner `GqlError`.

### Query budgets

All three engines accept (or default to) an `ExecutionBudget` that bounds
wall-clock time and row count:

```rust
use std::time::Duration;
use sedan_gremlin_embedded::ExecutionBudget;

let mut budget = ExecutionBudget::new(Duration::from_secs(5), 10_000);
engine.execute_with_budget(&store, "g", "g.V().count()", &mut budget)?;
```

For Cypher, use `cypher_core::executor::ExecutionBudget` directly when
calling `QueryExecutor` — the embedded `CypherEngine::execute` uses
`ExecutionBudget::default_budget()` automatically.

### Plan caching

`CypherEngine` caches parsed/planned queries by query text (default
capacity 128). Cached plans automatically observe fresh storage state, so
write queries (`CREATE`, `SET`, `DELETE`, `MERGE`) work correctly. Call
`engine.clear_cache()` after schema-changing DDL (e.g. `CREATE INDEX`).

`GremlinEngine` exposes `compile()` for explicit "parse-once, execute-many":

```rust
let q = engine.compile("g.V().has(\"name\",\"Alice\").values(\"name\")")?;
let r1 = engine.execute_compiled(&store, "g", &q)?;
let r2 = engine.execute_compiled(&store, "g", &q)?;
```

### Multiple named graphs

`InMemoryGraphStore` is a multi-graph container. Call `create_graph(name)`
for each one and pass the name to `execute(...)`. Data is isolated per
graph. There is no cross-graph query syntax — keep them in separate stores
or merge them into one before querying.

---

## Storage backends

`InMemoryGraphStore` is the default and the only one bundled with the
embedded crates. It is `Send + Sync`, holds the entire graph in memory,
and supports the full `GraphStore` trait. For richer storage:

| Backend | Crate | Use when |
|---|---|---|
| In-memory (default) | `cypher-core::storage::InMemoryGraphStore` | Tests, embedded apps, ephemeral graphs |
| In-memory + WAL persistence | `cypher-core::storage::PersistentGraphStore` | Embedded app that needs durability across restarts |
| Redis / Valkey | `sd2-redis-store::RedisGraphStore` | Multi-process apps, network-shared graph |
| Microsoft Fabric | `gql-fabric-store::FabricGraphStore` | Streaming changelog to a Fabric workspace |

Any type that implements `GraphStore` plugs into `engine.execute(&store, …)`
unchanged. The embedded crates don't pull in the Redis or Fabric backends —
add those crates as direct dependencies if you need them.

---

## Testing your integration

Examples and tests in each crate use the same pattern: build a fresh
`InMemoryGraphStore` per test, seed via `engine.execute(...)`, assert on
returned rows.

```bash
cargo test -p sedan-cypher-embedded     # unit + doc tests
cargo test -p sedan-gql-embedded
cargo test -p sedan-gremlin-embedded

# Conformance suites (slow):
cargo test -p sedan-cypher-embedded --test tck_via_embedded   # openCypher TCK
cargo test -p sedan-gremlin-embedded                          # Gremlin TCK via gremlin-tck
```

---

## Further reading

- [Embedded Engines Reference](reference/embedded-engines.md) — full public
  API surface for each crate
- [Embed SedanDelivery In-Process](tutorials/embed-sedandelivery.md) — an
  alternative quick start written from the consumer's perspective
- [Pick Your Embedded Engine](howto/pick-embedded-engine.md) — short
  decision guide and `Cargo.toml` cheat sheet
- [Architecture Overview](architecture.md) — how the embedded crates fit
  into the wider workspace (parser, planner, executor, storage)
- Per-crate README files: [cypher](../sedan-cypher-embedded/README.md),
  [gql](../sedan-gql-embedded/README.md),
  [gremlin](../sedan-gremlin-embedded/README.md)
- Runnable examples:
  [`sedan-cypher-embedded/examples/`](../sedan-cypher-embedded/examples),
  [`sedan-gql-embedded/examples/`](../sedan-gql-embedded/examples),
  [`sedan-gremlin-embedded/examples/`](../sedan-gremlin-embedded/examples)
