# sedan-cypher-embedded

Embeddable openCypher query engine for Rust. Wraps [`cypher-core`](../cypher-core/)
into a single `CypherEngine` struct with one method: `execute`.

## Quick start

Add the dependency:

```toml
[dependencies]
sedan-cypher-embedded = { path = "../sedan-cypher-embedded" }
# or, from crates.io once published:
# sedan-cypher-embedded = "0.1"
```

Run your first query:

```rust
use sedan_cypher_embedded::{CypherEngine, GraphStore, InMemoryGraphStore, Value};

fn main() {
    let engine = CypherEngine::new();
    let store = InMemoryGraphStore::default();
    store.create_graph("g").unwrap();

    // Create nodes and relationships
    engine.execute(&store, "g",
        "CREATE (a:Person {name: 'Alice', age: 30})-[:KNOWS]->(b:Person {name: 'Bob', age: 25})"
    ).unwrap();

    // Query them back
    let result = engine.execute(&store, "g",
        "MATCH (p:Person) WHERE p.age > 20 RETURN p.name, p.age ORDER BY p.age"
    ).unwrap();

    for row in &result.rows {
        println!("{}: {}", row["p.name"], row["p.age"]);
    }
    // Output:
    //   Bob: 25
    //   Alice: 30
}
```

## API overview

### Types re-exported from `cypher-core`

| Type | Description |
|------|-------------|
| `CypherEngine` | Query engine with internal LRU plan cache; parse, plan, and execute in one call |
| `InMemoryGraphStore` | In-process graph storage (multiple named graphs) |
| `GraphStore` | Trait for pluggable storage backends |
| `QueryResult` | Columns, rows, mutation statistics, and optional `trailing_error` |
| `Row` | `HashMap<String, Value>` |
| `Value` | Cypher value: `Int`, `Float`, `Str`, `Bool`, `Null`, `List`, `Map`, temporal, spatial, graph refs |
| `MutationStats` | Counts: `nodes_created`, `nodes_deleted`, `edges_created`, `edges_deleted`, `properties_set`, `labels_added`, `labels_removed` |
| `GqlError` | Structured error: `Parse`, `Plan`, `Execution`, `Storage`, `Type` |

### `CypherEngine`

```rust
let engine = CypherEngine::new(); // or CypherEngine::default()

let result: Result<QueryResult, GqlError> =
    engine.execute(&store, "graph_name", "MATCH (n) RETURN n");
```

The engine maintains an internal thread-safe plan cache (default capacity
128, override with `CypherEngine::with_cache_capacity`). It is `Send + Sync`
and is intended to be shared across threads via `Arc`. `CypherEngine` also
implements `Default`.

## Advanced usage

### Multiple named graphs

```rust
let store = InMemoryGraphStore::default();
store.create_graph("social").unwrap();
store.create_graph("commerce").unwrap();

engine.execute(&store, "social",
    "CREATE (:Person {name: 'Alice'})"
).unwrap();

engine.execute(&store, "commerce",
    "CREATE (:Product {name: 'Widget', price: 9.99})"
).unwrap();

// Graphs are fully isolated
let result = engine.execute(&store, "social",
    "MATCH (n) RETURN count(n) AS cnt"
).unwrap();
assert_eq!(result.rows[0]["cnt"], Value::Int(1));
```

### Reading mutation statistics

```rust
let result = engine.execute(&store, "g",
    "CREATE (a:Person {name: 'Alice'})-[:KNOWS]->(b:Person {name: 'Bob'})"
).unwrap();

assert_eq!(result.stats.nodes_created, 2);
assert_eq!(result.stats.edges_created, 1);
assert_eq!(result.stats.properties_set, 2);
```

### Error handling

```rust
use sedan_cypher_embedded::GqlError;

match engine.execute(&store, "g", "INVALID QUERY") {
    Ok(result) => { /* use result */ }
    Err(GqlError::Parse(e)) => eprintln!("Syntax error: {e}"),
    Err(GqlError::Plan(e))  => eprintln!("Planning error: {e}"),
    Err(GqlError::Execution(e)) => eprintln!("Runtime error: {e}"),
    Err(GqlError::Storage(e)) => eprintln!("Storage error: {e}"),
    Err(GqlError::Type(e)) => eprintln!("Type error: {e}"),
}
```

### Aggregation and projection

```rust
let result = engine.execute(&store, "g",
    "MATCH (p:Person)
     RETURN avg(p.age) AS avg_age,
            count(p) AS total,
            collect(p.name) AS names"
).unwrap();

let row = &result.rows[0];
println!("Average age: {}", row["avg_age"]);
println!("Total people: {}", row["total"]);
```

### Variable-length paths

```rust
engine.execute(&store, "g",
    "MATCH path = (a:Person {name: 'Alice'})-[:KNOWS*1..3]->(b:Person)
     RETURN b.name, length(path) AS distance"
).unwrap();
```

### MERGE (upsert)

```rust
engine.execute(&store, "g",
    "MERGE (p:Person {name: 'Alice'})
     ON CREATE SET p.created = true
     ON MATCH SET p.seen = p.seen + 1
     RETURN p"
).unwrap();
```

## Supported openCypher features

**Clauses:** MATCH, OPTIONAL MATCH, CREATE, DELETE, DETACH DELETE, SET, REMOVE,
RETURN (DISTINCT, ORDER BY, SKIP, LIMIT), WITH, UNWIND, CALL (YIELD), MERGE
(ON CREATE/ON MATCH), FOREACH, LOAD CSV, UNION / UNION ALL.

**Expressions:** Arithmetic, comparison, logical, string operators, CASE, list
comprehensions, reduce, EXISTS subqueries, parameters (`$name`), subscripts, slices.

**Functions:** 70+ built-ins covering math, strings, collections, temporal
(date/time/duration), spatial (point/distance), and graph introspection (id,
labels, type, nodes, relationships).

**Types:** Bool, Int, Float, String, Null, List, Map, Date, Time, LocalTime,
DateTime, LocalDateTime, Duration, Point (WGS-84/Cartesian 2D/3D), Bytes,
Node, Edge, Path.

## More

- [Embedded Library Usage Guide](../docs/EMBEDDED_GUIDE.md) — cypher vs gql vs
  gremlin comparison, sophisticated patterns, storage backends.
- Runnable examples: [`examples/quickstart.rs`](examples/quickstart.rs),
  [`examples/sophisticated.rs`](examples/sophisticated.rs).
  ```bash
  cargo run --example quickstart    -p sedan-cypher-embedded
  cargo run --example sophisticated -p sedan-cypher-embedded
  ```

## License

MIT OR Apache-2.0
