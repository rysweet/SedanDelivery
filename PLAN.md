# Rust GQL Engine over Redis: Implementation Plan

**Issue**: cloud-ecosystem-security/cybergym#431
**Goal**: Build a production-quality Rust GraphQL/Cypher query engine as a Redis module, serving as a clean-room, MIT/Apache-2.0-licensed alternative to RedisGraph/FalkorDB (SSPL).
**Date**: 2026-02-26
**Usage guide**: [docs/TUTORIAL.md](docs/TUTORIAL.md)
**Docs index**: [docs/README.md](docs/README.md)

---

## 0. Executive Summary

We build `gql-redis` — a Redis module written in Rust that implements a Cypher/GQL subset over Redis data structures. The engine exposes `GQL.QUERY` and `GQL.MUTATE` commands identical in wire format to FalkorDB's `GRAPH.QUERY`/`GRAPH.RO_QUERY`, enabling a drop-in swap via a DI registration change in the .NET application layer.

**Why clean-room Rust, not FalkorDB-core-rs?**
FalkorDB-core-rs is SSPL v1 (same license as FalkorDB). We need MIT/Apache 2.0 for a Microsoft security product that may be offered as a service.

**Why not GraphBLAS/sparse matrices?**
At CyberGym's scale (≤1M nodes, security graph topology), Redis-native adjacency lists (sorted sets) provide adequate performance with dramatically less implementation complexity. GraphBLAS would add ~8 weeks of effort for negligible benefit at this scale.

---

## 1. Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│  .NET Application Replicas                                      │
│  RustRedisGraphClient : IGraphClient                            │
│  StackExchange.Redis ExecuteAsync("GQL.QUERY", ...)             │
└──────────────────────────┬──────────────────────────────────────┘
                           │ TCP / Redis protocol
┌──────────────────────────▼──────────────────────────────────────┐
│  Redis Server (7.4+)                                            │
│  ┌────────────────────────────────────────────────────────┐     │
│  │  gql-module.so  (Redis Module, Rust)                   │     │
│  │                                                        │     │
│  │  GQL.QUERY  graphkey "MATCH (n:X) RETURN n"            │     │
│  │  GQL.MUTATE graphkey "CREATE (n:X {name:'y'})"         │     │
│  │  GQL.EXPLAIN / GQL.PROFILE / GQL.CREATE-GRAPH          │     │
│  │  GQL.DELETE-GRAPH / GQL.LIST-GRAPHS                    │     │
│  │                                                        │     │
│  │  Pipeline:                                             │     │
│  │  Input → Lexer → Parser (lalrpop) → AST               │     │
│  │       → Planner → PhysicalPlan                         │     │
│  │       → Executor ──→ GraphStore (Redis keys)           │     │
│  │       → ResultSerializer → Redis array reply           │     │
│  └────────────────────────────────────────────────────────┘     │
│                                                                  │
│  Redis Key Namespace (per graph "G"):                           │
│    G:meta          Hash  {node_count, edge_count, version}      │
│    G:node:{id}     Hash  {_labels, prop_k: prop_v, ...}         │
│    G:edge:{id}     Hash  {_type, _src, _dst, prop_k: prop_v}    │
│    G:out:{id}      Sorted Set  score=0, member="{dst}:{type}"   │
│    G:in:{id}       Sorted Set  score=0, member="{src}:{type}"   │
│    G:label:{L}     Set   {node_ids...}                          │
│    G:reltype:{T}   Set   {edge_ids...}                          │
│    G:seq:nodes     String  (auto-increment counter)             │
│    G:seq:edges     String  (auto-increment counter)             │
│    G:idx:{L}:{P}   Sorted Set  (property index, opt)            │
└─────────────────────────────────────────────────────────────────┘
                           │
                    Background sync
┌──────────────────────────▼──────────────────────────────────────┐
│  Parquet export → OneLake → Fabric Graph                        │
│  (reuses existing OneLakeGraphSync.ExportAsync() pattern)       │
└─────────────────────────────────────────────────────────────────┘
```

---

## 2. Crate Workspace Layout

```
gql-redis/                          # root workspace
├── Cargo.toml                      # workspace + shared deps
├── Cargo.lock                      # committed
├── build.rs                        # lalrpop compilation
├── rustfmt.toml
├── clippy.toml
│
├── gql-core/                       # pure library, zero Redis dependency
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── ast/                    # AST types (enums, newtypes, spans)
│       │   ├── mod.rs
│       │   ├── expr.rs             # Expr enum (recursive, Box<Expr>)
│       │   ├── clause.rs           # Clause enum: MATCH/CREATE/etc.
│       │   ├── pattern.rs          # NodePattern, RelPattern, Direction
│       │   └── span.rs             # Spanned<T> wrapper
│       ├── parser/
│       │   ├── mod.rs              # public parse_query() fn
│       │   ├── lexer.rs            # hand-written lexer (zero-copy)
│       │   └── cypher.lalrpop      # LALR(1) grammar
│       ├── planner/
│       │   ├── mod.rs              # QueryPlanner
│       │   ├── plan.rs             # PhysicalPlan, Operator enum
│       │   └── optimizer.rs        # join reorder, index pushdown
│       ├── executor/
│       │   ├── mod.rs              # QueryExecutor
│       │   ├── filter.rs           # predicate evaluation (SIMD-ready)
│       │   ├── aggregate.rs        # COUNT/SUM/AVG/COLLECT/MIN/MAX
│       │   └── traversal.rs        # BFS/DFS over GraphStore
│       ├── storage/
│       │   ├── mod.rs              # GraphStore trait
│       │   └── mem.rs              # InMemoryGraphStore (for tests)
│       ├── types/
│       │   ├── mod.rs
│       │   └── value.rs            # Value enum (no Box<dyn Any>)
│       └── error.rs                # GqlError hierarchy (thiserror)
│
├── gql-redis-store/                # Redis-backed GraphStore impl
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── store.rs                # RedisGraphStore : GraphStore
│       ├── keys.rs                 # Key formatting (G:node:{id} etc.)
│       ├── codec.rs                # Value ↔ Redis string encoding
│       └── pipeline.rs             # Redis pipeline batching
│
├── gql-module/                     # Redis module entry point
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs                  # redis_module! macro
│       ├── commands/
│       │   ├── mod.rs
│       │   ├── query.rs            # GQL.QUERY
│       │   ├── mutate.rs           # GQL.MUTATE
│       └── native_type.rs          # Redis native type (metadata)
│
├── gql-cli/                        # Standalone REPL for dev/debug
│   ├── Cargo.toml
│   └── src/main.rs
│
└── tests/
    ├── integration/                # testcontainers-rs tests
    │   ├── mod.rs
    │   ├── harness.rs              # GqlTestHarness
    │   └── conformance.rs          # openCypher TCK runner
    ├── property/                   # proptest property-based tests
    │   └── parser_roundtrip.rs
    └── features/                   # Gherkin BDD (cucumber-rs)
        ├── match/
        ├── create/
        ├── delete/
        ├── merge/
        ├── set/
        ├── return/
        ├── where/
        ├── aggregation/
        ├── path/
        └── tck/                    # Adapted openCypher TCK .feature files
```

---

## 3. Redis Storage Model

### 3.1 Key Naming Convention

All keys are prefixed with `{graphname}:` to enable multi-tenancy on a single Redis instance.

| Key | Type | Contents |
|-----|------|----------|
| `G:meta` | Hash | `node_count`, `edge_count`, `schema_version`, `created_at` |
| `G:seq:nodes` | String | Auto-increment node ID counter |
| `G:seq:edges` | String | Auto-increment edge ID counter |
| `G:node:{id}` | Hash | `_labels` (comma-sep), `_id`, and all user properties |
| `G:edge:{id}` | Hash | `_type`, `_src`, `_dst`, and all user properties |
| `G:out:{nodeId}` | Sorted Set | Members: `{edgeId}:{targetId}:{relType}`, score=0 |
| `G:in:{nodeId}` | Sorted Set | Members: `{edgeId}:{sourceId}:{relType}`, score=0 |
| `G:label:{Label}` | Set | Node IDs with this label |
| `G:reltype:{Type}` | Set | Edge IDs with this relationship type |
| `G:idx:{Label}:{Prop}` | Sorted Set | For numeric range queries: score=prop_value, member=nodeId |

### 3.2 Property Value Encoding

Properties are stored as Redis hash field values using a typed prefix:
- `i:{value}` — integer (e.g., `i:42`)
- `f:{value}` — float (e.g., `f:3.14`)
- `s:{value}` — string (e.g., `s:Alice`)
- `b:1` / `b:0` — boolean
- `n:` — null
- `l:{json}` — list (JSON-encoded, kept small)

This avoids type ambiguity at the cost of a 2-byte prefix overhead per property.

### 3.3 Pipeline Batching

Multi-hop traversals use Redis pipelining to amortize round-trip latency:
- Single-hop MATCH: 1 SMEMBERS (label index) + N HGETALL (node properties)
- Two-hop MATCH: 1 SMEMBERS + N ZRANGE (adjacency) + M HGETALL
- CREATE/MERGE: INCR (seq) + HSET (node/edge) + SADD (label/reltype) + ZADD×2 (out+in) — all in one pipeline

---

## 4. Parser Design

### 4.1 Strategy: Hand-Written Lexer + lalrpop LALR(1) Grammar

**Decision rationale:**
- `lalrpop` handles operator precedence, left-recursive expressions (property access chains like `n.a.b`), and complex pattern syntax correctly by construction
- Hand-written lexer gives zero-copy string slices back into the original query string
- Formal grammar file serves as living documentation of the supported subset

### 4.2 Full GQL / openCypher Compliance (NOT a limited subset)

> **Design Position**: The GitHub issue #431 suggests implementing only a narrow subset (MATCH, CREATE, MERGE, DELETE, SET, RETURN, WHERE). **We reject this position.** A query engine that implements an idiosyncratic subset is harder to reason about, creates a permanent maintenance burden documenting what is and isn't supported, breaks any query that touches unsupported features with cryptic errors, and forecloses future use cases. The openCypher TCK already defines the complete conformance test suite. We target full openCypher compliance from the grammar level, implementing execution incrementally. A parser that accepts the full language costs the same as one that rejects half of it — the grammar is the cheap part. Execution of rare clauses can be deferred with a clear `UnsupportedFeature` error, but the grammar should accept them.

**Phase 1 Full Clause Coverage:**
`MATCH`, `OPTIONAL MATCH`, `WHERE`, `RETURN`, `CREATE`, `MERGE`, `SET`, `REMOVE`, `DELETE`, `DETACH DELETE`, `WITH`, `UNWIND`, `ORDER BY`, `LIMIT`, `SKIP`, `CALL { }` (subquery), `FOREACH`, `UNION`, `UNION ALL`

**Full Pattern Coverage:**
- Node patterns: `(n)`, `(n:Label)`, `(n:A:B:C)` (multiple labels), `(n {prop: val})`
- Relationship patterns: `-[r]->`, `-[r:TYPE]->`, `-[r:A|B]->` (type disjunction), `-[r*]->`, `-[r*1..5]->`
- Quantified path patterns (GQL extension): `((n)-[r]->(m)){1,5}`
- Named paths: `p = (a)-[:R*]->(b)`

**Full Expression Coverage:**
- Arithmetic: `+`, `-`, `*`, `/`, `%`, `^`
- Comparison: `=`, `<>`, `<`, `>`, `<=`, `>=`, `=~` (regex)
- Logical: `AND`, `OR`, `NOT`, `XOR`
- String predicates: `STARTS WITH`, `ENDS WITH`, `CONTAINS`
- List: `IN`, `[0]`, `[1..3]` (slice), list comprehension `[x IN list WHERE ... | expr]`
- Map: `{key: expr}`, `n {.prop1, .prop2}` (projection)
- Path: `shortestPath()`, `allShortestPaths()`
- Pattern predicates: `EXISTS { MATCH ... }`, `(n)-->(m)` as predicate
- `CASE WHEN ... THEN ... ELSE ... END` (simple and searched forms)
- `IS NULL`, `IS NOT NULL`

**Full Function Coverage (openCypher spec + common extensions):**

*Aggregating*: `count()`, `sum()`, `avg()`, `min()`, `max()`, `collect()`, `stDev()`, `stDevP()`, `percentileCont()`, `percentileDisc()`

*Scalar*: `id()`, `coalesce()`, `endNode()`, `head()`, `last()`, `length()`, `properties()`, `randomUUID()`, `size()`, `startNode()`, `timestamp()`, `toBoolean()`, `toFloat()`, `toInteger()`, `toString()`, `type()`

*List*: `keys()`, `labels()`, `nodes()`, `range()`, `relationships()`, `reverse()`, `tail()`

*Math*: `abs()`, `ceil()`, `floor()`, `round()`, `sign()`, `sqrt()`, `exp()`, `log()`, `log10()`, `e()`, `pi()`, `rand()`, `sin()`, `cos()`, `tan()`, `asin()`, `acos()`, `atan()`, `atan2()`, `degrees()`, `radians()`

*String*: `left()`, `ltrim()`, `replace()`, `reverse()`, `right()`, `rtrim()`, `split()`, `substring()`, `toLower()`, `toUpper()`, `trim()`

*Temporal (ISO 8601)*: `date()`, `time()`, `datetime()`, `localtime()`, `localdatetime()`, `duration()`

*Spatial*: `point()`, `distance()` — storage as `{lat, lon, crs}` hash fields

**GQL (ISO 20030) Extensions Beyond openCypher:**
- `MATCH` path variable binding with quantifiers
- `RETURN GRAPH` clause
- Value types: `NOTHING`, `ANY`, typed null

The grammar accepts all of the above from day one. Execution engine implements them phase by phase, with `UnsupportedFeature { feature: String }` error for not-yet-implemented runtime paths — never a parse error for valid GQL.

### 4.3 AST Design Principles

```rust
// All recursive types use Box<T> to break infinite-size cycles
pub enum Expr {
    Integer(i64),
    Float(f64),
    Str(Box<str>),   // Box<str> saves 8 bytes vs String
    Bool(bool),
    Null,
    Variable(Identifier),
    Property(Box<Expr>, String),          // n.name
    BinaryOp { op: BinaryOp, left: Box<Expr>, right: Box<Expr> },
    FunctionCall { name: FunctionName, args: Vec<Expr>, distinct: bool },
    // ... etc
}

// Newtype wrappers prevent accidental mixing of IDs
pub struct NodeId(pub u32);
pub struct EdgeId(pub u32);
pub struct Identifier(pub String);
pub struct Label(pub String);
pub struct RelType(pub String);

// Every AST node carries source span for error messages
pub struct Spanned<T> { pub node: T, pub span: Span }
pub struct Span { pub start: usize, pub end: usize }
```

---

## 5. Execution Engine

### 5.1 Physical Plan Operators

```
PhysicalPlan
└── Operator (recursive tree)
    ├── NodeScan { label: Option<Label>, alias: Identifier }
    ├── IndexScan { label: Label, property: String, range: Range<Value> }
    ├── Filter { predicate: Expr, child: Box<Operator> }
    ├── Expand { direction: Direction, rel_types: Vec<RelType>, child: Box<Operator> }
    ├── VarLenExpand { min: u32, max: u32, direction: Direction, child: Box<Operator> }
    ├── NodeByIdSeek { id: Expr }
    ├── Project { items: Vec<(Expr, Option<Alias>)>, child: Box<Operator> }
    ├── Aggregate { keys: Vec<Expr>, aggregations: Vec<AggregateExpr>, child: Box<Operator> }
    ├── Sort { items: Vec<(Expr, Order)>, child: Box<Operator> }
    ├── Limit { count: Expr, child: Box<Operator> }
    ├── Skip { count: Expr, child: Box<Operator> }
    ├── CartesianProduct { left: Box<Operator>, right: Box<Operator> }
    ├── HashJoin { ... }
    ├── Create { patterns: Vec<CreatePattern> }
    ├── Merge { patterns: Vec<Pattern>, on_create: Vec<SetItem>, on_match: Vec<SetItem> }
    ├── Set { items: Vec<SetItem>, child: Box<Operator> }
    ├── Delete { exprs: Vec<Expr>, detach: bool, child: Box<Operator> }
    └── Optional { child: Box<Operator> }
```

### 5.2 GraphStore Trait (the most critical abstraction boundary)

```rust
pub trait GraphStore: Send + Sync {
    // Read
    fn get_node(&self, graph: &str, id: NodeId) -> Result<Option<NodeData>, StorageError>;
    fn get_edge(&self, graph: &str, id: EdgeId) -> Result<Option<EdgeData>, StorageError>;
    fn find_nodes_by_label(&self, graph: &str, label: &str) -> Result<Vec<NodeId>, StorageError>;
    fn find_edges_by_type(&self, graph: &str, rel_type: &str) -> Result<Vec<EdgeId>, StorageError>;
    fn out_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError>;
    fn in_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, StorageError>;
    fn all_node_ids(&self, graph: &str) -> Result<Vec<NodeId>, StorageError>;

    // Write
    fn create_node(&self, graph: &str, labels: Vec<String>, props: HashMap<String, Value>)
        -> Result<NodeId, StorageError>;
    fn create_edge(&self, graph: &str, src: NodeId, dst: NodeId, rel_type: String,
        props: HashMap<String, Value>) -> Result<EdgeId, StorageError>;
    fn set_node_props(&self, graph: &str, id: NodeId, props: HashMap<String, Value>)
        -> Result<(), StorageError>;
    fn delete_node(&self, graph: &str, id: NodeId, detach: bool) -> Result<(), StorageError>;
    fn delete_edge(&self, graph: &str, id: EdgeId) -> Result<(), StorageError>;

    // Admin
    fn create_graph(&self, name: &str) -> Result<(), StorageError>;
    fn delete_graph(&self, name: &str) -> Result<(), StorageError>;
    fn graph_exists(&self, name: &str) -> Result<bool, StorageError>;
    fn list_graphs(&self) -> Result<Vec<String>, StorageError>;
    fn graph_stats(&self, name: &str) -> Result<GraphStats, StorageError>;
}
```

### 5.3 BFS/DFS Traversal (borrow-checker-friendly)

```rust
pub struct BfsTraversal<'s> {
    store: &'s dyn GraphStore,
    graph: &'s str,
    queue: VecDeque<(NodeId, u32)>,     // (node, depth)
    visited: FxHashSet<NodeId>,
    rel_type_filter: Option<Vec<RelType>>,
    direction: Direction,
    max_depth: u32,
}

impl<'s> Iterator for BfsTraversal<'s> {
    type Item = Result<(NodeId, u32), StorageError>;
    // ...
}
```

Key insight: the traversal iterator owns all mutable state (`queue`, `visited`). The store is borrowed immutably. This pattern compiles without fighting the borrow checker.

---

## 6. Test Suite Architecture

### 6.1 Test Pyramid

```
10% E2E (Redis module loaded in testcontainers Docker, full wire protocol)
│
30% Integration (executor + InMemoryGraphStore, no Redis)
│    - All openCypher TCK scenarios
│    - CyberGym query patterns
│
60% Unit (parser, planner, value types, key formatting, codec)
     - Property-based tests with proptest (parser never panics)
     - Snapshot tests for AST (insta crate)
```

### 6.2 openCypher TCK Integration (~800 scenarios)

The openCypher TCK (Apache 2.0) provides Gherkin feature files covering the full language. Strategy:
1. Copy all ~50 `.feature` files from `opencypher/openCypher/tck/features/` into `tests/features/tck/`
2. Implement a cucumber-rs step runner that maps Gherkin steps to `InMemoryGraphStore` + executor calls
3. Initially mark failing scenarios with `@wip` tag; track pass rate toward the 80% acceptance criterion

**TCK Step Implementations Needed:**
```gherkin
Given an empty graph
Given having executed: "CREATE (n:Person {name: 'Alice'})"
When executing query: "MATCH (n) RETURN n"
Then the result should be, in any order:
  | n                             |
  | (:Person {name: 'Alice'})     |
And the side effects should be:
  | +nodes | 1 |
  | +labels | 1 |
```

### 6.3 RedisGraph Compatibility Tests

RedisGraph's own test suite (MIT) contains hundreds of Python integration tests. We adapt the most important categories:
- `test_graph_creation.py` → Rust integration tests
- `test_query_flags.py` → parser/executor unit tests
- `test_results.py` → result serialization tests
- `test_edge_cases.py` → property-based tests

### 6.4 CyberGym-Specific Tests

From the `IGraphClient` interface contract (5 methods):
- `QueryAsync(graph, cypher, params)` → returns `IEnumerable<IRecord>`
- `MutateAsync(graph, cypher, params)` → returns `MutateResult`
- `CreateGraphAsync(name)`
- `DeleteGraphAsync(name)`
- `GraphExistsAsync(name)`

Write a test matrix covering all CyberGym query patterns documented in the codebase.

### 6.5 Performance/Benchmark Tests (criterion.rs)

```toml
[[bench]]
name = "traversal"
harness = false

[[bench]]
name = "parser"
harness = false
```

Benchmarks (all run against InMemoryGraphStore for determinism):
- Parse throughput: 10K simple queries/sec minimum
- Node scan (100K nodes, label filter): <5ms
- 1-hop traversal (100K nodes, 500K edges): <5ms p99
- 2-hop traversal: <50ms p99
- Property filter on 10K nodes: <1ms

### 6.6 Test Infrastructure

```toml
[dev-dependencies]
testcontainers          = "0.23"
testcontainers-modules  = { version = "0.11", features = ["redis"] }
redis                   = "0.27"
tokio                   = { version = "1", features = ["full"] }
proptest                = "1.6"
cucumber                = "0.21"
criterion               = { version = "0.5", features = ["html_reports"] }
insta                   = "1.40"        # snapshot testing for AST
```

---

## 7. Key Technical Decisions

### 7.1 Storage Format Choice: Redis Hashes + Sets (NOT GraphBLAS)

**Decision**: Use Redis-native data structures, not sparse adjacency matrices.

**Rationale**:
- At ≤1M nodes / ≤10M edges, Redis hashes + sorted sets provide adequate performance
- GraphBLAS requires SuiteSparse (C library, complex FFI) or a pure-Rust matrix library (both immature)
- Redis-native storage makes persistence, replication, and AOF logging free (handled by Redis)
- Horizontal scaling via Redis Cluster is straightforward
- Cold start is instant (data is already in Redis, shared across replicas)

**Accepted tradeoff**: Multi-hop traversals are Redis-round-trip-bound rather than in-memory-computation-bound. For the CyberGym use case (security graphs, ≤5 hops, ≤1M nodes), this is acceptable at <50ms p99.

### 7.2 Parser Choice: Hand-Written Lexer + lalrpop

**Decision**: Hand-written lexer (zero-copy) + lalrpop LALR(1) grammar.

**Rejected**:
- `nom`: Parser becomes unreadable at Cypher's scale; no left-recursion support
- `pest`: Allocates heavily via `Pairs`; no typed AST generation
- `tree-sitter`: Runtime parsing library, not suitable for embedded use in Redis module
- Hand-written recursive descent: Excellent error messages but 3-4 weeks more work

### 7.3 Redis Module vs. Standalone Service

**Decision**: Redis module (`.so` loaded via `loadmodule`).

**Rationale**: Same wire protocol as FalkorDB. `.NET` client uses identical `StackExchange.Redis ExecuteAsync("GQL.QUERY", ...)` pattern. No additional service to operate.

**Alternative considered**: Sidecar service (separate TCP port). Rejected — adds operational complexity and removes data locality.

### 7.4 Error Handling: `thiserror` (not `anyhow`)

In a library/module context, `anyhow` erases error types. Use `thiserror` throughout with a structured error hierarchy that maps cleanly to Redis error prefix conventions (`ERR`, `WRONGTYPE`, `LIMIT`).

### 7.5 Concurrency Model: Single-Threaded (Redis Model)

Redis modules run in Redis's main thread for command handlers. Long-running queries (>100ms estimated) use `RedisModule_BlockClient` + background thread pattern. No `Arc<Mutex<...>>` on the hot path.

---

## 8. Implementation Phases

### Phase 0: Scaffolding (Week 1)
- [ ] Create Cargo workspace with all crates
- [ ] Set up CI/CD: `cargo test`, `cargo clippy --deny warnings`, `cargo fmt --check`
- [ ] Implement `Value` type and `GqlError` hierarchy
- [ ] Implement `GraphStore` trait + `InMemoryGraphStore`
- [ ] Wire up criterion benchmarks skeleton
- [ ] Set up testcontainers integration test harness

### Phase 1: Parser (Weeks 2-3)
- [ ] Implement hand-written lexer with token types
- [ ] Implement lalrpop grammar for the core subset (MATCH, WHERE, RETURN, CREATE, DELETE, SET, MERGE)
- [ ] Implement `Spanned<T>` AST nodes
- [ ] proptest: parser never panics on arbitrary input
- [ ] insta: snapshot tests for 50+ query ASTs
- [ ] Unit test coverage >90% on parser module

### Phase 2: Planner & Executor (Weeks 4-6)
- [ ] Implement all physical plan operators
- [ ] Implement BFS/DFS traversal iterators over `GraphStore`
- [ ] Implement expression evaluator (all operators and functions)
- [ ] Implement aggregation (COUNT, SUM, AVG, COLLECT, MIN, MAX, DISTINCT)
- [ ] Implement ORDER BY / LIMIT / SKIP
- [ ] Wire up openCypher TCK runner against `InMemoryGraphStore`
- [ ] Achieve >60% TCK pass rate (read scenarios first)

### Phase 3: Redis Storage (Week 7)
- [ ] Implement `RedisGraphStore` with `RedisContext` (for module use)
- [ ] Implement key formatting (`G:node:{id}`, etc.)
- [ ] Implement property encoding/decoding codec
- [ ] Implement Redis pipeline batching for multi-hop traversals
- [ ] Integration tests with testcontainers

### Phase 4: Redis Module (Week 8)
- [ ] Wire up `redis_module!` macro
- [ ] Implement `GQL.QUERY`, `GQL.MUTATE`, `GQL.EXPLAIN` command handlers
- [ ] Implement `GQL.CREATE-GRAPH`, `GQL.DELETE-GRAPH`, `GQL.LIST-GRAPHS`
- [ ] Implement RDB load/save for graph metadata native type
- [ ] Load module in integration tests; run full test suite
- [ ] Achieve >80% TCK pass rate (acceptance criterion)

### Phase 5: .NET Client (Week 9)
- [ ] Implement `RustRedisGraphClient : IGraphClient` in C#
- [ ] Result deserialization: Redis array → `IRecord` mapping
- [ ] Pass all `IGraphClient` integration tests used by `LadybugGraphClient`
- [ ] DI registration swap test (confirm drop-in replacement)

### Phase 6: Fabric Sync + Hardening (Weeks 10-11)
- [ ] Implement Parquet export from Redis key scan
- [ ] Verify identical Parquet output as LadybugDB sync
- [ ] Load test: 100K nodes, 500K edges, 1000 concurrent queries
- [ ] Achieve p99 latency targets (<5ms point query, <50ms 2-hop)
- [ ] Achieve >80% TCK write scenarios pass rate
- [ ] Security review: command injection, key escape, SSPL license scan

### Phase 7: Documentation & Release (Week 12)
- [ ] Architecture decision records (ADRs)
- [ ] Redis module deployment guide
- [ ] .NET integration guide
- [ ] Performance tuning guide

---

## 9. Acceptance Criteria

| Criterion | Target | Measurement |
|-----------|--------|-------------|
| TCK read scenarios | >80% | openCypher TCK cucumber runner |
| TCK write scenarios | >80% | openCypher TCK cucumber runner |
| IGraphClient compatibility | 100% | Integration test suite |
| Fabric sync parity | Identical Parquet | Byte-level comparison |
| Point query latency | <5ms p99 | criterion + testcontainers |
| 2-hop traversal latency | <50ms p99 | criterion + testcontainers |
| Graph size | 100K node, 500K edge | Load test |

---

## 10. Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| lalrpop grammar complexity for Cypher | Medium | High | Start with a strict subset; expand incrementally |
| Redis round-trip overhead for multi-hop | Medium | Medium | Batch pipeline; cache hot nodes in query execution context |
| redis-module-rs API instability | Low | High | Pin to specific commit; review changelog before upgrading |
| TCK pass rate <80% at deadline | Medium | High | Prioritize read scenarios; defer edge cases with `@wip` tags |
| SSPL contamination (FalkorDB-core-rs) | High | Critical | Clean-room implementation; no code reading from SSPL sources |
| Memory safety in Redis module | Low | Critical | All unsafe justified and commented; address sanitizer in CI |
| Redis Cluster key slot violations | Low | Medium | All graph keys use `{graphname}` hash tag for same slot |

---

## 11. Dependency Inventory

| Crate | Version | License | Purpose |
|-------|---------|---------|---------|
| `redis-module` | 0.24 | MIT | Redis module SDK |
| `lalrpop` + `lalrpop-util` | 0.20 | Apache-2.0 | LALR(1) parser generation |
| `thiserror` | 1.0 | MIT/Apache-2.0 | Error type derivation |
| `typed-arena` | 2.0 | MIT/Apache-2.0 | Arena allocation for query execution |
| `indexmap` | 2.0 | MIT/Apache-2.0 | Ordered maps for Cypher Map literals |
| `rustc-hash` | 2.0 | MIT/Apache-2.0 | Fast hash maps (FxHashSet/FxHashMap) |
| `wide` | 0.7 | Zlib/Apache-2.0 | SIMD property filtering (optional) |
| `petgraph` | 0.6 | MIT/Apache-2.0 | Graph algorithms in executor |
| `testcontainers-modules` | 0.11 | MIT | Redis in Docker for integration tests |
| `proptest` | 1.6 | MIT/Apache-2.0 | Property-based parser fuzzing |
| `cucumber` | 0.21 | MIT | BDD test runner for TCK |
| `criterion` | 0.5 | MIT/Apache-2.0 | Benchmarking |
| `insta` | 1.40 | MIT | Snapshot testing for AST |

**All dependencies are MIT or Apache-2.0. No GPL, LGPL, or SSPL dependencies.**

---

## 12. Resolved Design Decisions

| Decision | Resolution | Rationale |
|---|---|---|
| `CALL` subquery support | Deferred — grammar accepts it, executor returns `UnsupportedFeature` | Full grammar from day one; execution deferred cleanly per ADR-002 |
| Valkey vs. Redis version | Target Valkey 8.x (BSD-3-Clause) with `use-redismodule-api` flag for Redis 7.4 compatibility | Redis 8 is AGPLv3 — licensing risk for Microsoft products offered as services |
| Multi-tenancy model | All graphs in DB 0 with `{graphname}:` key prefixes + Redis Cluster hash tags | Avoids DB-level isolation complexity; hash tags ensure cluster slot affinity per graph |
| Persistence | RDB persistence is sufficient for Phase 1; AOF optional via Redis config | RDB provides crash recovery; AOF is an ops decision, not an engine decision |
| Cluster support | Single-instance sufficient for Phase 1; Cluster-compatible by design (hash tags) | Key naming uses `{graphname}` hash tags so Cluster upgrade requires no code changes |
| Parquet sync direction | Push model (Redis → OneLake) triggered on demand or schedule | Consistent with existing `OneLakeGraphSync.ExportAsync()` pattern in LadybugDB |

---

## 13. References

- [openCypher TCK](https://github.com/opencypher/openCypher/tree/main/tck) (Apache 2.0)
- [opengql/grammar](https://github.com/opengql/grammar) (Apache 2.0)
- [redis-module-rs](https://github.com/RedisLabsModules/redismodule-rs) (MIT)
- [RedisGraph source](https://github.com/RedisGraph/RedisGraph) (Redis Source Available License — read-only reference)
- [FalkorDB design docs](https://docs.falkordb.com/design/) (read-only reference; code is SSPL)
- [lalrpop](https://github.com/lalrpop/lalrpop) (Apache 2.0)
- [testcontainers-rs](https://github.com/testcontainers/testcontainers-rs) (MIT)
- [proptest](https://github.com/proptest-rs/proptest) (MIT)
- [cucumber-rs](https://github.com/cucumber-rs/cucumber) (MIT)
- [cybergym#431](https://github.com/cloud-ecosystem-security/cybergym/issues/431)
