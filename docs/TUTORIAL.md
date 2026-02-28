# Getting Started with gql-core

This tutorial shows how to use the `gql-core` library directly in Rust code.
No Redis instance needed — the `InMemoryGraphStore` provides a full in-process implementation.

## Add to your Cargo.toml

```toml
[dependencies]
gql-core = { path = "../gql-core" }  # or from crates.io once published
```

## 1. Create a Graph and Add Data

```rust
use gql_core::{
    storage::{GraphStore, InMemoryGraphStore, NodeId, EdgeId, validate_graph_name},
    types::Value,
};
use std::collections::HashMap;

fn main() {
    let store = InMemoryGraphStore::default();

    // Validate graph name before using it
    // (enforced at API boundary: no ':', no '{}', max 64 chars)
    validate_graph_name("social").expect("invalid graph name");
    store.create_graph("social").expect("create_graph failed");

    // Create nodes with labels and properties
    let alice = store.create_node(
        "social",
        vec!["Person".into()],
        HashMap::from([
            ("name".into(), Value::Str("Alice".into())),
            ("age".into(), Value::Int(30)),
        ]),
    ).unwrap();

    let bob = store.create_node(
        "social",
        vec!["Person".into()],
        HashMap::from([
            ("name".into(), Value::Str("Bob".into())),
            ("age".into(), Value::Int(25)),
        ]),
    ).unwrap();

    // Create a relationship
    let _knows = store.create_edge(
        "social",
        alice,
        bob,
        "KNOWS".into(),
        HashMap::from([("since".into(), Value::Int(2020))]),
    ).unwrap();

    println!("Graph created: {} nodes", store.graph_stats("social").unwrap().node_count);
}
```

## 2. Query the Graph

```rust
// Find all nodes with a specific label
let people = store.find_nodes_by_label("social", "Person").unwrap();
for id in people {
    let node = store.get_node("social", id).unwrap().unwrap();
    println!("{}: {:?}", id.0, node.properties);
}

// Traverse relationships
let outgoing = store.out_edges("social", alice).unwrap();
for edge in outgoing {
    println!("  -[:{rel}]->(id={dst})",
        rel = edge.rel_type,
        dst = edge.neighbor_id.0);
}

// Type-filtered traversal (efficient — no client-side filter)
let knows_edges = store.out_edges_by_type("social", alice, &["KNOWS"]).unwrap();

// Batch fetch multiple nodes at once (O(1) pass through the store)
let batch = store.get_nodes_batch("social", &[alice, bob]).unwrap();
```

## 3. Property Predicates

```rust
use gql_core::storage::RangeQuery;

// Find all people older than 28
let older = store.find_nodes_by_label_and_property(
    "social",
    "Person",
    "age",
    RangeQuery::Gt(&Value::Int(28)),
).unwrap();
println!("People over 28: {}", older.len());  // 1 (Alice, age 30)

// Find all people named "Alice"
let alices = store.find_nodes_by_label_and_property(
    "social",
    "Person",
    "name",
    RangeQuery::Eq(&Value::Str("Alice".into())),
).unwrap();
```

## 4. Expression Evaluation

```rust
use gql_core::{
    ast::{Expr, BinaryOp, Identifier},
    executor::eval_expr,
    types::Value,
};
use std::collections::HashMap;

// Evaluate a Cypher expression against a row context
let row: HashMap<String, Value> = HashMap::from([
    ("n.age".into(), Value::Int(30)),
    ("n.name".into(), Value::Str("Alice".into())),
]);

// Evaluates: n.age > 25 AND n.name STARTS WITH "Al"
let age_check = Expr::BinaryOp {
    op: BinaryOp::Gt,
    left: Box::new(Expr::Variable(Identifier("n.age".into()))),
    right: Box::new(Expr::Integer(25)),
};
let result = eval_expr(&age_check, &row).unwrap();
assert_eq!(result, Value::Bool(true));
```

## 5. Execution Budget (DoS Prevention)

Every query execution must have a budget. This prevents unbounded `MATCH (n)-[*]->(m)` from hanging the process:

```rust
use gql_core::executor::ExecutionBudget;
use std::time::Duration;

// Production: 5-second deadline, 1M row limit
let budget = ExecutionBudget::new(Duration::from_secs(5), 1_000_000);

// For tests only: no limits
#[cfg(test)]
let budget = ExecutionBudget::unlimited();
```

## 6. Tokenize a Cypher Query

```rust
use gql_core::parser::lex_query;

let tokens = lex_query("MATCH (n:Person {name: 'Alice'})-[:KNOWS]->(m) RETURN m.name")
    .expect("lex error");

for token in &tokens {
    println!("{:?} '{}'", token.kind, token.text);
}
// Output:
// Match "MATCH"
// LParen "("
// Identifier "n"
// Colon ":"
// Identifier "Person"
// ... etc
```

## 7. Graph Administration

```rust
// List all graphs on this store
let graphs = store.list_graphs().unwrap();
println!("Graphs: {:?}", graphs);

// Check existence
if store.graph_exists("social").unwrap() {
    let stats = store.graph_stats("social").unwrap();
    println!("social: {} nodes, {} edges", stats.node_count, stats.edge_count);
}

// Delete a graph (removes all data)
store.delete_graph("social").unwrap();
```

## 8. DETACH DELETE

```rust
// DELETE without DETACH fails if node has relationships
let result = store.delete_node("social", alice, false);
assert!(result.is_err());  // Alice has KNOWS edge

// DETACH DELETE removes all incident edges then the node
store.delete_node("social", alice, true).unwrap();
// alice's edges are gone too
assert!(store.out_edge_ids("social", alice).unwrap().is_empty());
```

## 9. Property Indexes

The planner uses indexes to choose `IndexScan` over `NodeScan + Filter`:

```rust
// Create a property index on Person.age
store.create_index("social", "Person", "age").unwrap();

// Planner can now choose IndexScan for queries like:
// MATCH (n:Person) WHERE n.age > 25 RETURN n

// List existing indexes
let indexes = store.list_indexes("social").unwrap();
for idx in indexes {
    println!("Index: {}:{}", idx.label, idx.property);
}

// Drop an index
store.drop_index("social", "Person", "age").unwrap();
```

## Supported Aggregate Functions

The executor supports the full set of openCypher aggregate functions:

| Function | Description | Example |
|---|---|---|
| `count(*)` | Count all rows | `RETURN count(*)` |
| `count(expr)` | Count non-null values | `RETURN count(n.age)` |
| `sum(expr)` | Sum numeric values | `RETURN sum(n.salary)` |
| `avg(expr)` | Average numeric values | `RETURN avg(n.age)` |
| `min(expr)` | Minimum value | `RETURN min(n.score)` |
| `max(expr)` | Maximum value | `RETURN max(n.score)` |
| `collect(expr)` | Gather into list (nulls skipped) | `RETURN collect(n.name)` |
| `stdev(expr)` | Sample standard deviation | `RETURN stdev(n.age)` |
| `stdevp(expr)` | Population standard deviation | `RETURN stdevp(n.age)` |
| `percentileDisc(expr, p)` | Discrete percentile (nearest rank) | `RETURN percentileDisc(n.age, 0.5)` |
| `percentileCont(expr, p)` | Continuous percentile (interpolated) | `RETURN percentileCont(n.age, 0.5)` |

All aggregates work in `WITH` and `RETURN` clauses, including inside expressions:

```cypher
-- collect() inside size()
MATCH (n) RETURN size(collect(n.name)) AS count

-- percentile as median
MATCH (n:Person) RETURN percentileDisc(n.age, 0.5) AS median

-- grouped aggregation
MATCH (n:Person) RETURN n.city, avg(n.age) AS avgAge, collect(n.name) AS names
```

## UNION / UNION ALL

```cypher
RETURN 1 AS x UNION RETURN 2 AS x      -- deduplicates
RETURN 1 AS x UNION ALL RETURN 1 AS x  -- keeps duplicates
```

## OPTIONAL MATCH

```cypher
-- m is null when no match found for n
MATCH (n:Person)
OPTIONAL MATCH (n)-[:KNOWS]->(m:Person)
RETURN n.name, m.name
```

## Phase 1 Preview: Full Query Execution

Phase 1 (in progress) will enable:

```rust
// Parse → Plan → Execute
use gql_core::parser::parse_query;

let ast = parse_query("MATCH (p:Person) WHERE p.age > 25 RETURN p.name ORDER BY p.name")
    .expect("parse error");

let planner = QueryPlanner::new(&store, "social");
let plan = planner.plan(&ast).expect("plan error");

let mut budget = ExecutionBudget::default_budget();
let result = QueryExecutor::new(&store)
    .execute("social", &plan, &mut budget)
    .expect("execution error");

for row in result.rows {
    println!("{}", row["p.name"]);
}
```

## 10. EXISTS { subquery } and Pattern Predicates

`EXISTS { ... }` returns `true` if the inner pattern matches at least one row in the graph.
Bare pattern expressions (`(n)-[:REL]->()`) used directly in `WHERE` behave identically.

```cypher
-- Filter: only nodes that have an outgoing KNOWS edge
MATCH (n) WHERE EXISTS { (n)-[:KNOWS]->() } RETURN n

-- Full MATCH form with WHERE inside the subquery
MATCH (n) WHERE EXISTS { MATCH (n)-[:KNOWS]->(m) WHERE m.age < 30 } RETURN n

-- Use EXISTS as a projection (returns Bool)
MATCH (n:Person) RETURN n.name, EXISTS { (n)-[:KNOWS]->() } AS hasKnows

-- Negation
MATCH (n) WHERE NOT EXISTS { (n)-[:KNOWS]->() } RETURN n

-- Bare pattern predicate (equivalent to EXISTS { ... })
MATCH (n) WHERE (n)-[:KNOWS]->() RETURN n

-- Pattern predicate with two conditions (conjunction)
MATCH (n) WHERE (n)-[:KNOWS]-() AND (n)-[:LIKES]-() RETURN n
```

In Rust:
```rust
let result = executor.execute("social",
    &planner.plan(&parse_query(
        "MATCH (n) WHERE EXISTS { (n)-[:KNOWS]->() } RETURN n"
    ).unwrap()).unwrap(),
    &mut budget).unwrap();

// rows contains only nodes with outgoing KNOWS edges
println!("{} matches", result.rows.len());
```

## Running the Demo

```bash
cargo run -p gql-cli
```

This exercises the storage layer, lexer, and security validation interactively.

## Using the Client SDKs

The examples above use the low-level `gql-core` Rust library directly.
For application code that talks to a running SedanDelivery server over Redis
protocol, use one of the client SDKs instead. Python is shown as the primary
example below; the .NET and Rust SDKs expose the same operations with
language-idiomatic APIs (see [sdk/README.md](../sdk/README.md) for all three).

### Connect and create a graph

```python
from sedandelivery import SedanDB

db = SedanDB(host="localhost", port=6379, password="secret")
g = db.select_graph("social")
g.create()
```

### Add nodes and relationships

```python
# Parameterized mutations -- $name is replaced with the escaped value
g.mutate(
    "CREATE (a:Person {name: $name, age: $age})"
    "-[:KNOWS]->"
    "(b:Person {name: $friend})",
    name="Alice", age=30, friend="Bob",
)

g.mutate(
    "CREATE (:Company {name: $co})",
    co="Acme",
)

g.mutate(
    "MATCH (a:Person {name: $who}), (c:Company {name: $co}) "
    "CREATE (a)-[:WORKS_AT {since: $yr}]->(c)",
    who="Alice", co="Acme", yr=2022,
)
```

### Query with structured results

```python
for record in g.query("MATCH (p:Person)-[:KNOWS]->(f) RETURN p.name, f.name"):
    print(record["p.name"], "knows", record["f.name"])

# Aggregation
for record in g.query("MATCH (p:Person) RETURN avg(p.age) AS avg_age, count(p) AS total"):
    print(f"Average age: {record['avg_age']}, total: {record['total']}")
```

### Inspect the execution plan

```python
plan = g.explain("MATCH (p:Person) WHERE p.age > 25 RETURN p.name")
print(plan)
```

### Check mutation stats

```python
result = g.mutate("CREATE (:Company {name: $co})", co="NewCo")
print(result.stats.nodes_created)   # 1
print(result.stats.properties_set)  # 1
```

### Clean up

```python
g.delete()       # drop the graph
db.close()       # close the connection
```

### .NET and Rust

The .NET SDK (`SedanDelivery.Client`) and Rust SDK (`sedan-client`) provide
the same `SedanDB -> Graph -> query/mutate` workflow with typed results, async
support (.NET), and parameter substitution. See the
[full SDK documentation](../sdk/README.md) for installation, API reference, and
type mapping tables.

## Architecture Reference

See [PLAN.md](../PLAN.md) for the full architecture and [docs/](.) for ADRs and research notes.
