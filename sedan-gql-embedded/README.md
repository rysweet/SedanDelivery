# sedan-gql-embedded

Embeddable GQL/Cypher query engine for in-process use. No Redis, no Bolt,
no network -- just GQL parse, plan, and execute in your Rust process.

This crate is a thin convenience wrapper around [`gql-engine`](../gql-engine),
providing the `SedanGql` entry point and re-exporting the types you need.

## Quick start

```rust
use sedan_gql_embedded::{SedanGql, GraphStore, InMemoryGraphStore, Value};

let engine = SedanGql::new();
let store = InMemoryGraphStore::default();
store.create_graph("g").unwrap();

// GQL INSERT (equivalent to Cypher CREATE)
let result = engine
    .execute(&store, "g", "INSERT (n:Person {name: 'Alice'}) RETURN n.name")
    .unwrap();
assert_eq!(result.rows[0].get("n.name"), Some(&Value::Str("Alice".into())));
```

## When to use this crate

Use `sedan-gql-embedded` when you want to:

- Run GQL/Cypher queries entirely in-process without any server
- Embed a graph query engine in a CLI tool, test harness, or library
- Share an `InMemoryGraphStore` across Cypher, GQL, and Gremlin engines in one process

If you need the lower-level parser/planner/executor modules, depend on
`gql-engine` directly instead.

## API

### `SedanGql`

```rust
use sedan_gql_embedded::{SedanGql, GqlError, QueryResult, GraphStore};

let engine = SedanGql::new();  // or SedanGql::default()

// Parse, plan, and execute in one call
let result: Result<QueryResult, GqlError> = engine.execute(&store, "graph_name", "RETURN 1");
```

The `execute` method accepts any GQL or Cypher query string. GQL-specific
keywords (`INSERT`, `FILTER`, `MANDATORY MATCH`, `FOR`, `LET`) are
handled transparently.

### Parameterised queries

Use `execute_with_context` for `$param` placeholders or custom procedures:

```rust
use std::collections::HashMap;
use sedan_gql_embedded::prelude::*;

let engine = SedanGql::new();
let store = InMemoryGraphStore::default();
store.create_graph("g").unwrap();

let mut params = HashMap::new();
params.insert("name".into(), Value::Str("Alice".into()));

let result = engine
    .execute_with_context(
        &store, "g",
        "INSERT (n:Person {name: $name}) RETURN n.name",
        &ProcedureRegistry::default(),
        params,
    )
    .unwrap();
assert_eq!(result.rows[0].get("n.name"), Some(&Value::Str("Alice".into())));
```

### Prelude

Import everything you need in one line:

```rust
use sedan_gql_embedded::prelude::*;
```

### Re-exported types

All essential types are re-exported so you only need this one dependency:

| Type | Description |
|---|---|
| `GraphStore` | Storage trait (implement for custom backends) |
| `InMemoryGraphStore` | In-memory storage (good for testing and embedded use) |
| `QueryResult` | Query result with `columns` and `rows` |
| `Row` | A single result row (get values by column name) |
| `Value` | Property value enum (scalars, collections, temporals, spatial, graph elements) |
| `MutationStats` | Counts of nodes/relationships created/deleted |
| `GqlError` | Top-level error enum (`Parse`, `Plan`, `Execution`) |
| `ProcedureRegistry` | Procedure registry for `execute_with_context` |

## GQL keyword support

| GQL syntax | What it does |
|---|---|
| `INSERT (n:Label {k: v})` | Create nodes/relationships |
| `MATCH (n) FILTER pred` | Filter results (like `WHERE`) |
| `FILTER WHERE pred` | Alternate filter syntax |
| `MANDATORY MATCH (n)` | Error if no results found |
| `FOR x IN [1,2,3]` | Unwind a list |
| `LET x = expr` | Bind a computed value |
| `MATCH TRAIL/WALK/SIMPLE/ACYCLIC` | Path mode (metadata extracted) |
| `MATCH ANY SHORTEST` | Path search (metadata extracted) |

Standard Cypher (`CREATE`, `MATCH`, `WHERE`, `RETURN`, `WITH`, `UNWIND`,
`DELETE`, `SET`, `REMOVE`, `ORDER BY`, `LIMIT`, `SKIP`) works unchanged.

## Multi-engine example

All three embedded engines share the same `GraphStore`:

```rust
use sedan_cypher_embedded::CypherEngine;
use sedan_gql_embedded::{SedanGql, GraphStore, InMemoryGraphStore, Value};
use sedan_gremlin_embedded::GremlinEngine;

let store = InMemoryGraphStore::default();
store.create_graph("demo").unwrap();

// Write with Cypher
let cypher = CypherEngine::new();
cypher.execute(&store, "demo", "CREATE (n:Person {name: 'Alice'})").unwrap();

// Read with GQL
let gql = SedanGql::new();
let result = gql.execute(&store, "demo", "MATCH (n:Person) FILTER n.name = 'Alice' RETURN n.name").unwrap();
assert_eq!(result.rows.len(), 1);

// Read with Gremlin
let gremlin = GremlinEngine::new();
let result = gremlin.execute(&store, "demo", "g.V().has(\"name\",\"Alice\").values(\"name\")").unwrap();
assert_eq!(result.rows.len(), 1);
```

See [Tutorial: Embed SedanDelivery In-Process](../docs/tutorials/embed-sedandelivery.md)
for the full walkthrough including isolation and error handling.

## Error handling

```rust
use sedan_gql_embedded::{SedanGql, GqlError, GraphStore, InMemoryGraphStore};

let engine = SedanGql::new();
let store = InMemoryGraphStore::default();
store.create_graph("g").unwrap();

// Parse errors
assert!(matches!(
    engine.execute(&store, "g", "NOT VALID !!!"),
    Err(GqlError::Parse(_))
));

// MANDATORY MATCH with no data
assert!(matches!(
    engine.execute(&store, "g", "MANDATORY MATCH (n:Ghost) RETURN n"),
    Err(GqlError::Execution(_))
));
```

## More

- [Embedded Library Usage Guide](../docs/EMBEDDED_GUIDE.md) — cypher vs gql vs
  gremlin comparison, sophisticated patterns, storage backends.
- Runnable examples: [`examples/quickstart.rs`](examples/quickstart.rs),
  [`examples/sophisticated.rs`](examples/sophisticated.rs).
  ```bash
  cargo run --example quickstart    -p sedan-gql-embedded
  cargo run --example sophisticated -p sedan-gql-embedded
  ```

## License

MIT OR Apache-2.0
