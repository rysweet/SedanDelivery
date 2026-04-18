# cypher-core

Pure Rust openCypher/GQL query engine with no external database dependency.
Provides parsing, planning, execution, and pluggable storage for the
[SedanDelivery](https://github.com/rysweet/SedanDelivery) graph database.

## Architecture

```text
Query string
    |
    v
 ┌─────────┐     ┌─────────┐     ┌──────────┐     ┌───────────┐
 │  Parser  │ --> │   AST   │ --> │ Planner  │ --> │ Executor  │
 │(lexer +  │     │(typed   │     │(validator│     │(operator  │
 │ recursive│     │ nodes)  │     │ + plan   │     │ tree eval)│
 │ descent) │     │         │     │ builder) │     │           │
 └─────────┘     └─────────┘     └──────────┘     └───────────┘
                                       |                 |
                                       v                 v
                                  ┌──────────────────────────┐
                                  │      GraphStore trait     │
                                  │  (InMemory / Persistent / │
                                  │   Cached / Redis / ...)   │
                                  └──────────────────────────┘
```

**Parser** (`parser/`) -- Hand-written recursive descent (not a parser generator).
Chosen because Cypher's `|` character is context-dependent (label disjunction,
rel-type disjunction, list comprehension pipe). Two modes: strict (`lex_query`)
and lenient (`tokenize`) for IDE tooling.

**AST** (`ast/`) -- Typed node tree with `Span` source positions for error
reporting. Uses `SmolStr` for labels/rel-types (inline < 22 bytes) and
`Box<str>` for string values (16 bytes vs String's 24).

**Planner** (`planner/`) -- Semantic validation (undefined variables, aggregation
scope, write-after-read) then physical plan construction. 33+ operator types
covering scans, pattern matching, joins, aggregation, mutations, and Gremlin
traversal steps.

**Executor** (`executor/`) -- Tree-walking interpreter with execution budget
(wall-clock timeout, row limits, depth limits). 70+ built-in functions. Supports
temporal arithmetic, spatial point operations, and Gremlin side-effects.

**Storage** (`storage/`) -- `GraphStore` trait with three implementations:
`InMemoryGraphStore` (for embedding/testing), `PersistentGraphStore` (durable),
`CachedGraphStore` (write-through cache around any backend).

## Quick start

```rust
use cypher_core::parser::parse_query;
use cypher_core::planner::QueryPlanner;
use cypher_core::executor::{QueryExecutor, ExecutionBudget};
use cypher_core::storage::{GraphStore, InMemoryGraphStore};

fn main() {
    let store = InMemoryGraphStore::default();
    store.create_graph("g").unwrap();

    // Parse
    let ast = parse_query("CREATE (n:Person {name: 'Alice'}) RETURN n.name").unwrap();

    // Plan
    let planner = QueryPlanner::new(&store, "g");
    let plan = planner.plan(&ast).unwrap();

    // Execute
    let mut budget = ExecutionBudget::default_budget();
    let result = QueryExecutor::new(&store).execute("g", &plan, &mut budget).unwrap();

    assert_eq!(result.rows.len(), 1);
    println!("{:?}", result.rows[0]); // {"n.name": Str("Alice")}
}
```

For a simpler API, use [`sedan-cypher-embedded`](../sedan-cypher-embedded/) which
wraps all three steps into a single `CypherEngine::execute` call.

## Module reference

| Module | Purpose |
|--------|---------|
| `ast` | AST node types: `Query`, `Clause`, `Expr`, `Identifier`, `Label`, `Span` |
| `parser` | `parse_query()`, `lex_query()`, `tokenize()` |
| `planner` | `QueryPlanner`, `Operator` (33+ physical plan operators), semantic `Validator` |
| `executor` | `QueryExecutor`, `QueryResult` (incl. `trailing_error`), `Row`, `MutationStats`, `ExecutionBudget` |
| `storage` | `GraphStore` trait, `InMemoryGraphStore`, `PersistentGraphStore` (WAL-based), `CachedGraphStore` (write-through) |
| `types` | `Value` enum (17 variants), `CypherDuration`, `GeoPoint` |
| `error` | `GqlError`, `ParseError`, `PlanError`, `ExecutionError`, `StorageError`, `TypeError` |
| `config` | `EngineConfig` -- env-based configuration (timeouts, limits, cache sizes) |
| `plan_cache` | Query plan cache with epoch-based eviction, keyed by query string hash |
| `procedure` | Procedure registry for CALL/YIELD |
| `serde_value` | Tagged JSON encoding for `Value` round-trip fidelity |
| `changelog` | Cross-backend synchronization types (Fabric sync) |

## Storage backends

### InMemoryGraphStore

Zero-dependency in-process storage. Suitable for embedding, unit tests, and
single-process applications.

```rust
use cypher_core::storage::{GraphStore, InMemoryGraphStore};

let store = InMemoryGraphStore::default();
store.create_graph("mydb").unwrap();
```

### PersistentGraphStore

WAL-based durable storage. All reads go through the in-memory store (zero disk
I/O for reads); writes append to a write-ahead log and periodically snapshot.

```rust
use cypher_core::storage::{PersistentGraphStore, PersistentConfig, SyncMode};
use std::time::Duration;

let config = PersistentConfig {
    data_dir: "/tmp/mydb".into(),
    snapshot_interval: Duration::from_secs(60),
    sync_mode: SyncMode::Periodic(Duration::from_secs(5)),
};
let store = PersistentGraphStore::open(config).unwrap();
store.create_graph("g").unwrap();
```

### CachedGraphStore

Write-through cache wrapper around any `GraphStore` backend. Lazy-loads entire
graphs into an in-memory cache on first access; invalidates on write.

```rust
use std::sync::Arc;
use cypher_core::storage::{CachedGraphStore, InMemoryGraphStore, GraphStore};

let backend: Arc<dyn GraphStore> = Arc::new(InMemoryGraphStore::default());
let store = CachedGraphStore::new(backend);
store.create_graph("g").unwrap();
```

### Implementing a custom backend

Implement the `GraphStore` trait:

```rust
use cypher_core::storage::{GraphStore, NodeId, EdgeId, NodeData, EdgeData, AdjEntry, GraphStats};
use cypher_core::error::StorageError;

struct MyStore { /* ... */ }

impl GraphStore for MyStore {
    fn create_graph(&self, name: &str) -> Result<(), StorageError> { todo!() }
    fn create_node(&self, graph: &str, labels: Vec<String>,
                   props: indexmap::IndexMap<String, cypher_core::types::Value>)
                   -> Result<NodeId, StorageError> { todo!() }
    // ... remaining methods
}
```

Key methods: `create_graph`, `create_node`, `create_edge`, `delete_node`,
`delete_edge`, `get_node`, `get_edge`, `scan_nodes`, `out_edges`, `in_edges`,
`query_nodes` (index lookup), `graph_stats`, `create_index`, `drop_index`.

## Configuration

All settings are controlled via environment variables with sensible defaults:

| Variable | Default | Purpose |
|----------|---------|---------|
| `GQL_QUERY_TIMEOUT_MS` | 30000 | Wall-clock query timeout |
| `GQL_QUERY_MAX_ROWS` | unlimited | Max intermediate rows |
| `GQL_VARLEN_MAX_DEPTH` | 30 | Max variable-length path depth |
| `GQL_MAX_QUERY_SIZE` | 67 MB | Max query string size |
| `GQL_PARSER_MAX_DEPTH` | 256 | Expression nesting limit |
| `GQL_PLAN_CACHE_SIZE` | 1024 | Plan cache capacity |

See `config::EngineConfig::from_env()` for the full list.

## Error types

All errors derive from `GqlError`:

```rust
use cypher_core::error::GqlError;

match result {
    Err(GqlError::Parse(e))     => { /* syntax error with byte offset */ }
    Err(GqlError::Plan(e))      => { /* undefined variable, scope error */ }
    Err(GqlError::Execution(e)) => { /* type mismatch, timeout, division by zero */ }
    Err(GqlError::Storage(e))   => { /* graph not found, node not found */ }
    Err(GqlError::Type(e))      => { /* type coercion failure */ }
    Ok(result) => { /* QueryResult with columns, rows, stats */ }
}
```

### Partial results (streaming semantics)

`QueryResult` includes a `trailing_error: Option<GqlError>` field. When a
per-row error occurs during execution (e.g. division by zero on one row of an
`UNWIND`), the successfully-computed rows are returned in `rows` and the error
that terminated evaluation is stored in `trailing_error`. This enables
Bolt-level streaming: emit RECORD messages for partial rows, then FAILURE for
the trailing error.

## Benchmarks

Four Criterion benchmark suites:

```bash
cargo bench -p cypher-core --bench parser       # tokenization + parsing throughput
cargo bench -p cypher-core --bench executor      # query execution
cargo bench -p cypher-core --bench scale         # large graph operations
cargo bench -p cypher-core --bench property_index # index lookup performance
```

## Testing

```bash
cargo test -p cypher-core                     # unit + integration tests
cargo test -p cypher-core --test tck           # openCypher TCK (Gherkin features)
cargo test -p cypher-core --test ldbc          # LDBC benchmark suite
cargo test -p cypher-core --test adversarial   # edge cases and stress
cargo test -p cypher-core --test data_integrity # concurrent modification safety
```

## Supported openCypher features

**Clauses:** MATCH, OPTIONAL MATCH, CREATE, DELETE, DETACH DELETE, SET, REMOVE,
RETURN (DISTINCT, ORDER BY, SKIP, LIMIT), WITH, UNWIND, CALL (YIELD), MERGE
(ON CREATE SET, ON MATCH SET), FOREACH, LOAD CSV, UNION / UNION ALL.

**Patterns:** Node `(n:Label {k: v})`, relationship `[r:TYPE *min..max {k: v}]`,
shortest path, all shortest paths, undirected/directed.

**Expressions:** All arithmetic/comparison/logical/string operators, CASE,
list comprehensions, reduce, EXISTS subqueries, parameters, subscripts, slices.

**Functions (70+):** Math (abs, ceil, floor, round, sqrt, log, trig...), strings
(size, toUpper, toLower, trim, replace, split...), collections (head, last,
tail, reverse, range, keys, values...), temporal (date, time, datetime,
duration, truncate, between...), spatial (point, distance), graph (id, labels,
type, nodes, relationships).

**Aggregations:** count, sum, avg, min, max, collect, stdev, stdevp,
percentileCont, percentileDisc.

**Types:** Bool, Int (i64), Float (f64), String, Null, List, Map, Date, Time,
LocalTime, DateTime, LocalDateTime, Duration, Point (4 CRS variants), Bytes,
Node, Edge, Path.

## Feature flags

| Flag | Effect |
|------|--------|
| `simd` | Enable SIMD-accelerated operations via the `wide` crate |
| `test-utils` | Expose `ExecutionBudget::unlimited()` in non-test code |

## License

MIT OR Apache-2.0
