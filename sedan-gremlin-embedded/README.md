# sedan-gremlin-embedded

Embeddable, in-process Gremlin query engine. Parse and execute Apache TinkerPop
Gremlin traversals against an in-memory graph store -- no server, no network,
no configuration.

Part of the [SedanDelivery](https://github.com/cloud-ecosystem-security/SedanDelivery)
graph database engine.

## Getting started

Add the dependency:

```toml
[dependencies]
sedan-gremlin-embedded = { path = "../sedan-gremlin-embedded" }
indexmap = "2"  # needed for property maps
```

Create a graph and run your first query:

```rust
use sedan_gremlin_embedded::{GremlinEngine, GraphStore, InMemoryGraphStore, Value};

// 1. Create engine and store
let engine = GremlinEngine::new();
let store = InMemoryGraphStore::default();
store.create_graph("g").unwrap();

// 2. Add some data
use indexmap::IndexMap;
store.create_node(
    "g",
    vec!["Person".into()],
    IndexMap::from([
        ("name".into(), Value::Str("Alice".into())),
        ("age".into(), Value::Int(30)),
    ]),
).unwrap();
store.create_node(
    "g",
    vec!["Person".into()],
    IndexMap::from([
        ("name".into(), Value::Str("Bob".into())),
        ("age".into(), Value::Int(25)),
    ]),
).unwrap();

// 3. Query with Gremlin
let result = engine.execute(&store, "g", "g.V().count()").unwrap();
assert_eq!(result.rows.len(), 1); // one row with count = 2

let result = engine.execute(
    &store, "g",
    r#"g.V().has("name","Alice").values("name")"#,
).unwrap();
assert_eq!(result.rows.len(), 1); // ["Alice"]
```

## API overview

The entire public API is three items:

| Type | Purpose |
|------|---------|
| `GremlinEngine` | Stateless engine. Call `execute()` to parse, compile, and run a Gremlin traversal. |
| `GremlinError` | Error enum: `Parse`, `Compile`, or `Execution` variants. |
| Re-exports | `GraphStore`, `InMemoryGraphStore`, `Value`, `QueryResult`, `Row`, `MutationStats`, `GqlError` from cypher-core. |

### `GremlinEngine::execute`

```rust
pub fn execute(
    &self,
    store: &dyn GraphStore,
    graph: &str,
    gremlin_query: &str,
) -> Result<QueryResult, GremlinError>
```

- `store` -- Any `GraphStore` implementation (use `InMemoryGraphStore` for embedded).
- `graph` -- Name of the graph within the store (must be pre-created with `create_graph`).
- `gremlin_query` -- Gremlin traversal string (e.g., `"g.V().count()"`).

## Examples

### Filter and project

```rust
use sedan_gremlin_embedded::{GremlinEngine, GraphStore, InMemoryGraphStore};

let engine = GremlinEngine::new();
let store = InMemoryGraphStore::default();
store.create_graph("g").unwrap();

// Find people over 25 and return their names
let result = engine.execute(
    &store, "g",
    r#"g.V().hasLabel("Person").has("age", P.gt(25)).values("name")"#,
).unwrap();
```

### Mutations

```rust
use sedan_gremlin_embedded::{GremlinEngine, GraphStore, InMemoryGraphStore};

let engine = GremlinEngine::new();
let store = InMemoryGraphStore::default();
store.create_graph("g").unwrap();

// Add a vertex
engine.execute(
    &store, "g",
    r#"g.addV("Person").property("name","Charlie").property("age",35)"#,
).unwrap();

// Verify
let result = engine.execute(&store, "g", "g.V().count()").unwrap();
```

### Edge traversal

```rust
use sedan_gremlin_embedded::{GremlinEngine, GraphStore, InMemoryGraphStore};

let engine = GremlinEngine::new();
let store = InMemoryGraphStore::default();
store.create_graph("g").unwrap();

// Traverse edges
let result = engine.execute(
    &store, "g",
    r#"g.V().hasLabel("Person").out("knows").values("name")"#,
).unwrap();
```

### Aggregation

```rust
use sedan_gremlin_embedded::{GremlinEngine, GraphStore, InMemoryGraphStore};

let engine = GremlinEngine::new();
let store = InMemoryGraphStore::default();
store.create_graph("g").unwrap();

// Group by label, count each group
let result = engine.execute(
    &store, "g",
    r#"g.V().groupCount().by(label)"#,
).unwrap();
```

### Error handling

```rust
use sedan_gremlin_embedded::{GremlinEngine, GremlinError, GraphStore, InMemoryGraphStore};

let engine = GremlinEngine::new();
let store = InMemoryGraphStore::default();
store.create_graph("g").unwrap();

match engine.execute(&store, "g", "not.a.traversal()") {
    Ok(result) => println!("{} rows", result.rows.len()),
    Err(GremlinError::Parse(e)) => eprintln!("bad syntax: {e}"),
    Err(GremlinError::Compile(e)) => eprintln!("unsupported: {e}"),
    Err(GremlinError::Execution(e)) => eprintln!("runtime error: {e}"),
}
```

### Multi-graph isolation

A single store can host multiple named graphs that are fully isolated:

```rust
use sedan_gremlin_embedded::{GremlinEngine, GraphStore, InMemoryGraphStore};

let engine = GremlinEngine::new();
let store = InMemoryGraphStore::default();
store.create_graph("social").unwrap();
store.create_graph("inventory").unwrap();

// Data in "social" is invisible from "inventory"
engine.execute(&store, "social", r#"g.addV("Person").property("name","Alice")"#).unwrap();

let social_count = engine.execute(&store, "social", "g.V().count()").unwrap();
let inv_count = engine.execute(&store, "inventory", "g.V().count()").unwrap();
// social has 1 vertex, inventory has 0
```

## Supported Gremlin

This engine supports ~80 Gremlin step types including filters, traversals,
mutations, aggregations, string operations, collection operations, branching,
and path tracking. See the [gremlin-core README](../gremlin-core/README.md)
for the full step coverage table.

## Benchmarks

Run the end-to-end execution benchmarks (parse + compile + execute on a
100-vertex graph):

```bash
cargo bench -p sedan-gremlin-embedded
```

## More

- [Embedded Library Usage Guide](../docs/EMBEDDED_GUIDE.md) — cypher vs gql vs
  gremlin comparison, sophisticated patterns, storage backends.
- Runnable examples: [`examples/quickstart.rs`](examples/quickstart.rs),
  [`examples/sophisticated.rs`](examples/sophisticated.rs).
  ```bash
  cargo run --example quickstart    -p sedan-gremlin-embedded
  cargo run --example sophisticated -p sedan-gremlin-embedded
  ```

## License

MIT OR Apache-2.0
