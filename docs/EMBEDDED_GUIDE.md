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

## 30-line quick starts (one per engine)

Each block is a complete `src/main.rs` plus `Cargo.toml` excerpt: open an
in-memory store, insert three nodes, run a read query, print the rows.

### Cypher

```toml
# Cargo.toml
[dependencies]
sedan-cypher-embedded = { git = "https://github.com/cloud-ecosystem-security/SedanDelivery" }
```

```rust,no_run
// src/main.rs
use sedan_cypher_embedded::{CypherEngine, GraphStore, InMemoryGraphStore, Value};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let engine = CypherEngine::new();
    let store = InMemoryGraphStore::default();
    store.create_graph("g")?;

    engine.execute(&store, "g",
        "CREATE (:Person {name: 'Alice', age: 30}), \
                 (:Person {name: 'Bob',   age: 25}), \
                 (:Person {name: 'Carol', age: 41})")?;

    let r = engine.execute(&store, "g",
        "MATCH (p:Person) RETURN p.name AS name, p.age AS age ORDER BY p.age")?;

    for row in &r.rows {
        println!("{:?} {:?}", row.get("name"), row.get("age"));
    }
    Ok(())
}
```

### GQL

```toml
# Cargo.toml
[dependencies]
sedan-gql-embedded = { git = "https://github.com/cloud-ecosystem-security/SedanDelivery" }
```

```rust,no_run
// src/main.rs
use sedan_gql_embedded::prelude::*;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let engine = SedanGql::new();
    let store = InMemoryGraphStore::default();
    store.create_graph("g")?;

    engine.execute(&store, "g",
        "INSERT (:Person {name: 'Alice', age: 30}), \
                 (:Person {name: 'Bob',   age: 25}), \
                 (:Person {name: 'Carol', age: 41})")?;

    let r = engine.execute(&store, "g",
        "MATCH (p:Person) RETURN p.name AS name, p.age AS age ORDER BY p.age")?;

    for row in &r.rows {
        println!("{:?} {:?}", row.get("name"), row.get("age"));
    }
    Ok(())
}
```

### Gremlin

```toml
# Cargo.toml
[dependencies]
sedan-gremlin-embedded = { git = "https://github.com/cloud-ecosystem-security/SedanDelivery" }
indexmap = "2"
```

```rust,no_run
// src/main.rs
use indexmap::IndexMap;
use sedan_gremlin_embedded::{GraphStore, GremlinEngine, InMemoryGraphStore, Value};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let engine = GremlinEngine::new();
    let store = InMemoryGraphStore::default();
    store.create_graph("g")?;

    for (name, age) in [("Alice", 30_i64), ("Bob", 25), ("Carol", 41)] {
        store.create_node("g", vec!["Person".into()], IndexMap::from([
            ("name".into(), Value::Str(name.into())),
            ("age".into(),  Value::Int(age)),
        ]))?;
    }

    let r = engine.execute(&store, "g",
        r#"g.V().hasLabel("Person").order().by("age").values("name")"#)?;

    for row in &r.rows {
        println!("{:?}", row.iter().next().map(|(_, v)| v));
    }
    Ok(())
}
```

> **Tip** — a runnable, three-engine version of the above lives in
> [`examples/embedded_quickstart`](../examples/embedded_quickstart). Run it
> with `cargo run -p embedded_quickstart`.

---

## Storage backends

`cypher-core` defines the `GraphStore` trait. Anything that implements it
plugs into all three embedded engines unchanged.

| Backend | Type | Crate | Use when |
|---|---|---|---|
| In-memory | `InMemoryGraphStore` | `cypher-core` (re-exported) | Tests, ephemeral graphs, sub-second startup |
| In-memory + WAL | `PersistentGraphStore` | `cypher-core` | Embedded app needing crash-safe durability |
| Cached | `CachedGraphStore` | `cypher-core` | Wrap a slow backend with an LRU node/edge cache |
| Redis / Valkey | `RedisGraphStore` | `sd2-redis-store` | Multi-process or networked deployments |
| Microsoft Fabric | `FabricGraphStore` | `gql-fabric-store` | Stream changelog into a Fabric workspace |

### In-memory (default)

```rust,no_run
use sedan_cypher_embedded::InMemoryGraphStore;

let store = InMemoryGraphStore::default();
store.create_graph("g").unwrap();
```

`InMemoryGraphStore` holds the entire graph in process memory. It is fast
and `Send + Sync`, but the graph evaporates when the process exits.

### Persistent (WAL + snapshot)

```rust,no_run
use std::path::PathBuf;
use std::time::Duration;
use cypher_core::storage::{PersistentConfig, PersistentGraphStore, SyncMode};

let store = PersistentGraphStore::open(PersistentConfig {
    data_dir: PathBuf::from("./graph_data"),
    snapshot_interval: Duration::from_secs(60),
    sync_mode: SyncMode::Periodic(Duration::from_millis(500)),
}).unwrap();
```

Recovery on `open()` replays the snapshot and any trailing WAL segments. A
background thread takes periodic snapshots and truncates the WAL. Pass
`SyncMode::Always` for synchronous fsync per write (safest, slowest) or
`SyncMode::None` to leave durability to the OS (fastest, least safe).

### How to choose

| If you need… | Use |
|---|---|
| Fastest startup, no disk | `InMemoryGraphStore` |
| Survive process restart, single-writer | `PersistentGraphStore` |
| Multi-process or remote clients | `sd2-redis-store::RedisGraphStore` (over Valkey) |
| Auditable changelog | `gql-fabric-store::FabricGraphStore` |

The embedded crates do **not** pull in Redis or Fabric backends; add those
crates explicitly when you need them.

---

## Budgets and timeouts

Every query runs against an `ExecutionBudget` that bounds wall-clock time
and the maximum number of intermediate rows. The default budget is generous
but finite — supply your own when you need stricter limits (e.g. on a
public-facing endpoint).

```rust,no_run
use std::time::Duration;
use sedan_gremlin_embedded::{ExecutionBudget, GremlinEngine, GraphStore, InMemoryGraphStore};

let engine = GremlinEngine::new();
let store = InMemoryGraphStore::default();
store.create_graph("g").unwrap();

let mut budget = ExecutionBudget::new(Duration::from_secs(2), 50_000);
let result = engine
    .execute_with_budget(&store, "g", "g.V().count()", &mut budget)
    .unwrap();
```

| API | Effect |
|---|---|
| `ExecutionBudget::default_budget()` | Reads timeout/row-cap from `EngineConfig` defaults |
| `ExecutionBudget::new(timeout, max_rows)` | Explicit limits |
| `ExecutionBudget::unlimited()` | No limits — only for trusted, internal use |
| `ExecutionBudget::set_request_id(id)` | Tags the budget for tracing/logs |

For Cypher, `CypherEngine::execute` and `execute_with_parameters` use
`ExecutionBudget::default_budget()` internally. To use a custom budget,
drop down to `cypher_core::executor::QueryExecutor::execute(...)` and pass
your own `&mut ExecutionBudget`.

For GQL, `SedanGql::execute*` likewise uses the default budget; for stricter
limits use `gql_engine::execute_with_context` directly.

When the budget is exhausted you get `GqlError::Execution(_)` (Cypher / GQL)
or `GremlinError::Execution(GqlError::Execution(_))` (Gremlin) carrying a
"timeout" or "row limit" message.

---

## Parameters and value types

Never interpolate user input into a query string. Use parameters: `$name`
in the query text, plus a `HashMap<String, Value>` of bindings.

```rust,no_run
use std::collections::HashMap;
use sedan_cypher_embedded::{
    CypherEngine, GraphStore, InMemoryGraphStore, ProcedureRegistry, Value,
};

let engine = CypherEngine::new();
let store = InMemoryGraphStore::default();
store.create_graph("g").unwrap();

let mut params = HashMap::new();
params.insert("min_age".into(), Value::Int(28));
params.insert("labels".into(), Value::List(vec![
    Value::Str("Person".into()), Value::Str("Employee".into()),
]));

engine.execute_with_parameters(
    &store, "g",
    "MATCH (p:Person) WHERE p.age >= $min_age RETURN p.name",
    &params,
    &ProcedureRegistry::new(),
).unwrap();
```

The same shape works for GQL via `SedanGql::execute_with_context`.

### `Value` cheat-sheet

`Value` (re-exported by every embedded crate) is the universal property and
parameter type:

| Cypher / GQL literal | Rust constructor |
|---|---|
| `42` | `Value::Int(42)` |
| `3.14` | `Value::Float(3.14)` |
| `'alice'` | `Value::Str("alice".into())` |
| `true` | `Value::Bool(true)` |
| `null` | `Value::Null` |
| `[1, 2, 3]` | `Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)])` |
| `{k: 'v'}` | `Value::Map(IndexMap::from([("k".into(), Value::Str("v".into()))]))` |
| `date('2024-01-15')` | `Value::Date(...)` (via `jiff`) |
| `point({x:1, y:2})` | `Value::Point(...)` |

Read values back from result rows with `row.get("col")` returning
`Option<&Value>`, then pattern-match on the variant you expect.

> **Gremlin parameters.** `GremlinEngine::execute` does not accept
> parameters today. For parameterised Gremlin work, drop down to
> `gremlin_core::parse_traversal(...)` and supply a binding callback, then
> compile and execute via the engine.

---

## Error handling

Every engine returns a structured error enum. Match on the variant to
distinguish user errors (parse, type) from operational errors (storage,
budget exhaustion).

```rust,no_run
use sedan_cypher_embedded::{CypherEngine, GqlError, GraphStore, InMemoryGraphStore};

let engine = CypherEngine::new();
let store = InMemoryGraphStore::default();
store.create_graph("g").unwrap();

match engine.execute(&store, "g", "MATCH (n) RETURN n") {
    Ok(result) => { /* use result.rows */ }
    Err(GqlError::Parse(e))     => eprintln!("syntax error: {e}"),    // 4xx — caller's fault
    Err(GqlError::Plan(e))      => eprintln!("plan error: {e}"),      // 4xx — unknown fn / bad type
    Err(GqlError::Type(e))      => eprintln!("type error: {e}"),      // 4xx — bad coercion
    Err(GqlError::Execution(e)) => eprintln!("runtime: {e}"),         // 5xx OR 408 (budget)
    Err(GqlError::Storage(e))   => eprintln!("storage: {e}"),         // 5xx — backend failure
}
```

| Variant | Typical cause | Recovery |
|---|---|---|
| `Parse` | Bad query syntax | Reject the request; surface message to user |
| `Plan` | Unknown function, undefined variable | Same as `Parse` |
| `Type` | Comparing incompatible types, bad cast | Same as `Parse` |
| `Execution` | Timeout, row-cap, divide by zero | Retry with bigger budget, or escalate |
| `Storage` | Missing graph, disk full, write conflict | Recreate graph, free space, back off + retry |

`GremlinEngine` returns `GremlinError::{Parse, Compile, Execution(GqlError)}` —
unwrap the `Execution` variant to reach the same `GqlError` taxonomy.

---

## Concurrency and thread safety

| Type | `Send` | `Sync` | Recommended sharing pattern |
|---|---|---|---|
| `CypherEngine` | ✅ | ✅ | One per process, share via `Arc<CypherEngine>` |
| `SedanGql` | ✅ | ✅ | Same |
| `GremlinEngine` | ✅ | ✅ | Same |
| `InMemoryGraphStore` | ✅ | ✅ | One per logical graph-set, share via `Arc<InMemoryGraphStore>` |
| `PersistentGraphStore` | ✅ | ✅ | One per data directory; **never** open twice on the same dir |

Reads are concurrent (RwLock); writes are serialised at the graph level.
Many threads may issue `engine.execute(...)` against the same store
simultaneously.

```rust,no_run
use std::sync::Arc;
use std::thread;
use sedan_cypher_embedded::{CypherEngine, GraphStore, InMemoryGraphStore};

let engine = Arc::new(CypherEngine::new());
let store  = Arc::new(InMemoryGraphStore::default());
store.create_graph("g").unwrap();

let handles: Vec<_> = (0..8).map(|i| {
    let engine = Arc::clone(&engine);
    let store  = Arc::clone(&store);
    thread::spawn(move || {
        let q = format!("CREATE (:Worker {{id: {i}}})");
        engine.execute(&*store, "g", &q).unwrap();
    })
}).collect();
for h in handles { h.join().unwrap(); }
```

The plan cache inside `CypherEngine` is thread-safe and is the reason a
single shared engine is preferred — every thread benefits from the same
cached plans.

---

## Performance tips

1. **Always benchmark with `--release`.** Debug builds are 10-50× slower
   for executor-heavy workloads.
2. **Share one engine.** `CypherEngine`'s plan cache only helps when many
   queries hit the same instance.
3. **Bind parameters; do not concat strings.** Concatenated query strings
   defeat the plan cache and risk injection.
4. **Batch writes.** A single multi-statement `CREATE` (or `INSERT`) is far
   cheaper than N round-trips through `execute`.
5. **Pre-compile hot Gremlin traversals.**
   ```rust,no_run
   # use sedan_gremlin_embedded::{GremlinEngine, GraphStore, InMemoryGraphStore};
   # let engine = GremlinEngine::new();
   # let store = InMemoryGraphStore::default(); store.create_graph("g").unwrap();
   let q = engine.compile(r#"g.V().has("name","Alice").values("name")"#).unwrap();
   for _ in 0..1_000 {
       let _ = engine.execute_compiled(&store, "g", &q).unwrap();
   }
   ```
6. **Right-size your budget.** A tiny `ExecutionBudget` cuts off runaway
   queries early without hurting fast queries.
7. **Reach for `PersistentGraphStore` only when you need durability.** WAL
   sync is the dominant cost on write-heavy workloads.
8. **Drop `CachedGraphStore` in front of slow backends** if you are using
   Redis or Fabric storage and have a working set that fits in memory.

---

## Migration: cypher ↔ gql ↔ gremlin

The same in-memory graph can be queried by all three engines. The table
below shows the most common transformations. Wherever the syntax differs,
the result is identical (same `Row`s, same `Value`s).

| Intent | Cypher | GQL | Gremlin |
|---|---|---|---|
| **Insert one node** | `CREATE (:Person {name:'Alice'})` | `INSERT (:Person {name:'Alice'})` | `g.addV('Person').property('name','Alice')` |
| **Insert with edge** | `CREATE (a:Person)-[:KNOWS]->(b:Person)` | `INSERT (a:Person)-[:KNOWS]->(b:Person)` | `g.addV('Person').as('a').addV('Person').as('b').addE('KNOWS').from('a').to('b')` |
| **Match by label** | `MATCH (p:Person) RETURN p` | `MATCH (p:Person) RETURN p` | `g.V().hasLabel('Person')` |
| **Filter property** | `MATCH (p:Person) WHERE p.age > 25 RETURN p` | `MATCH (p:Person) FILTER p.age > 25 RETURN p` | `g.V().hasLabel('Person').has('age', P.gt(25))` |
| **Project** | `RETURN p.name, p.age` | `RETURN p.name, p.age` | `.values('name','age')` (one row per value) |
| **Order + limit** | `ORDER BY p.age LIMIT 10` | `ORDER BY p.age LIMIT 10` | `.order().by('age').limit(10)` |
| **Count** | `RETURN count(*) AS n` | `RETURN count(*) AS n` | `.count()` |
| **Aggregate** | `RETURN avg(p.age)` | `RETURN avg(p.age)` | `.values('age').mean()` |
| **Traverse out-edges** | `MATCH (a)-[:KNOWS]->(b) RETURN b` | `MATCH (a)-[:KNOWS]->(b) RETURN b` | `g.V().out('KNOWS')` |
| **Variable-length path** | `MATCH (a)-[:KNOWS*1..3]->(b)` | `MATCH (a)-[:KNOWS*1..3]->(b)` | `g.V().repeat(out('KNOWS')).times(3)` |
| **Upsert** | `MERGE (p:Person {name:'Alice'})` | _(use Cypher; GQL `MERGE` is partial)_ | `g.V().has('Person','name','Alice').fold().coalesce(unfold(), addV('Person').property('name','Alice'))` |
| **Delete one** | `MATCH (p {id:1}) DELETE p` | `MATCH (p {id:1}) DELETE p` | `g.V().has('id',1).drop()` |
| **Detach delete** | `MATCH (p) DETACH DELETE p` | `MATCH (p) DETACH DELETE p` | `g.V().drop()` (drops adjacent edges too) |
| **Parameter** | `WHERE p.age > $min` (Cypher uses `$`) | `WHERE p.age > $min` | not exposed by `GremlinEngine` (use `gremlin-core` directly) |

### Side-by-side: top-3 oldest people

```rust,no_run
// Cypher
engine.execute(&store, "g",
    "MATCH (p:Person) RETURN p.name AS name ORDER BY p.age DESC LIMIT 3")?;

// GQL
engine.execute(&store, "g",
    "MATCH (p:Person) RETURN p.name AS name ORDER BY p.age DESC LIMIT 3")?;

// Gremlin
engine.execute(&store, "g",
    "g.V().hasLabel('Person').order().by('age', desc).limit(3).values('name')")?;
```

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
- [Performance Benchmarking](PERFORMANCE.md) — criterion bench suite for
  each embedded crate and how to track regressions over time
- Per-crate README files: [cypher](../sedan-cypher-embedded/README.md),
  [gql](../sedan-gql-embedded/README.md),
  [gremlin](../sedan-gremlin-embedded/README.md)
- Runnable examples:
  [`sedan-cypher-embedded/examples/`](../sedan-cypher-embedded/examples),
  [`sedan-gql-embedded/examples/`](../sedan-gql-embedded/examples),
  [`sedan-gremlin-embedded/examples/`](../sedan-gremlin-embedded/examples)
