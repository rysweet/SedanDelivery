# gql-engine

ISO/IEC 39075 (GQL) query processor for SedanDelivery. Parses GQL source,
plans it through the shared `cypher-core` operator tree, and executes with
GQL-specific semantics layered on top.

## Quick start

```rust
use gql_engine::{execute, GraphStore, InMemoryGraphStore, Value};

let store = InMemoryGraphStore::new();
store.create_graph("g").unwrap();

// GQL uses INSERT instead of CREATE
let qr = execute(
    "INSERT (n:Person {name: 'Alice', age: 30}) RETURN n.name",
    &store,
    "g",
).unwrap();
assert_eq!(qr.rows[0].get("n.name"), Some(&Value::Str("Alice".into())));

// GQL uses FILTER instead of WHERE
let older = execute(
    "MATCH (n:Person) FILTER n.age >= 25 RETURN n.name",
    &store,
    "g",
).unwrap();
assert_eq!(older.rows.len(), 1);
```

## GQL keyword surface

The engine accepts the following GQL-specific constructs on top of standard
openCypher syntax:

| GQL syntax | openCypher equivalent | Status |
|---|---|---|
| `INSERT` | `CREATE` | Fully supported |
| `FILTER <pred>` | `WHERE <pred>` | Fully supported |
| `FILTER WHERE <pred>` | `WHERE <pred>` | Fully supported |
| `MANDATORY MATCH` | *(no equivalent)* | Errors on zero rows |
| `FOR x IN list` | `UNWIND list AS x` | Fully supported |
| `LET x = expr` | `WITH *, expr AS x` | Parse-level rewrite |
| `MATCH WALK/TRAIL/SIMPLE/ACYCLIC` | *(no equivalent)* | Metadata extracted; filtering planned |
| `MATCH ANY SHORTEST / ALL SHORTEST` | *(no equivalent)* | Metadata extracted; filtering planned |

Standard openCypher queries pass through unchanged.

## Architecture

```text
GQL source
  |
  v
parser::parse_gql()     -- keyword rewriting + metadata extraction
  |
  v
planner::plan_gql()     -- delegates to cypher-core QueryPlanner + GQL flags
  |
  v
executor::execute_gql() -- delegates to cypher-core QueryExecutor + GQL checks
  |
  v
QueryResult
```

### Modules

- **`parser`** -- GQL-to-Cypher source-level rewriting. Tokenises the input,
  substitutes GQL keywords to their Cypher equivalents, and extracts GQL
  metadata (mandatory match, path modes, path search modifiers) that has no
  representation in the cypher-core AST.

- **`planner`** -- Delegates to `cypher_core::planner::QueryPlanner` and wraps
  the resulting operator tree with GQL-specific plan flags (mandatory match,
  path mode).

- **`executor`** -- Delegates to `cypher_core::executor::QueryExecutor` and
  enforces GQL semantics post-execution (e.g. MANDATORY MATCH empty-result
  error).

## Advanced usage: parameters and procedures

```rust
use std::collections::HashMap;
use gql_engine::{execute_with_context, GraphStore, InMemoryGraphStore, Value};
use cypher_core::procedure::ProcedureRegistry;

let store = InMemoryGraphStore::new();
store.create_graph("g").unwrap();

let registry = ProcedureRegistry::default();
let mut params = HashMap::new();
params.insert("name".into(), Value::Str("Alice".into()));

let qr = execute_with_context(
    "INSERT (n:Person {name: $name}) RETURN n.name",
    &store,
    "g",
    &registry,
    params,
).unwrap();
assert_eq!(qr.rows[0].get("n.name"), Some(&Value::Str("Alice".into())));
```

## Error handling

All public functions return `Result<QueryResult, GqlError>`. The error enum
has three variants:

- `GqlError::Parse` -- syntax error in the GQL source
- `GqlError::Plan` -- planning failure (e.g. missing graph)
- `GqlError::Execution` -- runtime failure (e.g. MANDATORY MATCH empty)

```rust
use gql_engine::{execute, GqlError, GraphStore, InMemoryGraphStore};

let store = InMemoryGraphStore::new();
store.create_graph("g").unwrap();

match execute("NOT VALID !!!", &store, "g") {
    Err(GqlError::Parse(_)) => { /* expected */ }
    other => panic!("expected parse error, got: {other:?}"),
}
```

## Re-exports

The crate re-exports the types most consumers need from `cypher-core`:

- `GraphStore`, `InMemoryGraphStore` -- storage trait and in-memory impl
- `QueryResult`, `Row`, `MutationStats` -- query result types
- `Value` -- the property value enum (scalars, collections, temporals, spatial, graph elements)

## License

MIT OR Apache-2.0
