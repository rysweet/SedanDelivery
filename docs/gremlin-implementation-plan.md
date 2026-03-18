# Apache TinkerPop Gremlin Implementation Plan for SedanDelivery

> **Status**: Architecture Decision Record — Multi-Agent Debate Output  
> **Date**: 2025  
> **Participants**: Architect 1 (Language/Parser), Architect 2 (Storage/Execution), Architect 3 (Protocol/Integration)  
> **Consensus**: 3/3 convergence on 15/17 major decisions; 2/3 on remaining 2  

---

## Executive Summary

Gremlin support will be added as a **new protocol frontend** that compiles Gremlin bytecode to SedanDelivery's existing `Operator` execution tree, reusing ~80% of current infrastructure. The implementation requires:

- **`gql-core/src/gremlin/`** — Bytecode deserializer, Step IR, and Compiler (inside existing crate)
- **`sedandelivery-gremlin/`** — New crate for WebSocket protocol server on port 8182
- **5 new Operator variants** — `Repeat`, `Choose`, `Coalesce`, `SideEffect`, `Cap`
- **Traversers encoded as Rows** — using `__gremlin_head__`, `__gremlin_path__`, `__gremlin_sack__` keys
- **4-phase rollout** — 14 weeks to production-ready Gremlin support

The execution pipeline:

```
TinkerPop Driver → WebSocket (port 8182)
  → GraphSON v3 decode → GremlinRequest
  → Bytecode deserialize → Vec<GremlinStep>
  → GremlinCompiler → Operator tree
  → QueryExecutor::exec_operator() ← SHARED WITH CYPHER
  → Vec<Row> → GremlinValue → GraphSON v3 encode
  → WebSocket response (200/206)
```

---

## Table of Contents

1. [Debate Summary & Consensus](#1-debate-summary--consensus)
2. [Phased Roadmap](#2-phased-roadmap)
3. [Crate & Module Structure](#3-crate--module-structure)
4. [Type Architecture](#4-type-architecture)
5. [Step Compilation Matrix](#5-step-compilation-matrix)
6. [New Operators](#6-new-operators)
7. [Protocol Design](#7-protocol-design)
8. [Value Type Mapping](#8-value-type-mapping)
9. [Risk Register](#9-risk-register)

---

## 1. Debate Summary & Consensus

### Three Architect Perspectives

| Architect | Focus | Key Contribution |
|-----------|-------|-----------------|
| **A1: Language/Parser** | Gremlin IR, bytecode parsing, step compilation strategy | Designed the `GremlinStep` IR, proved most Gremlin steps map to existing Operators, proposed auto-parameterization for plan cache |
| **A2: Storage/Execution** | Traverser semantics, path tracking, side effects, new operators | Designed `TraversalPath` type, `SideEffectState`, proved traversers can be encoded as Rows, identified path memory risks |
| **A3: Protocol/Integration** | WebSocket protocol, GraphSON serialization, driver compatibility | Designed `GremlinConnection` (stateless multiplexer vs Bolt's state machine), value type mapping, testing strategy |

### Consensus Scorecard

| Decision | A1 | A2 | A3 | Final | Confidence |
|----------|:--:|:--:|:--:|-------|:----------:|
| Bytecode-first (no eval Phase 1) | ✅ | — | ✅ | **Bytecode only, eval Phase 3** | Very High |
| Gremlin IR in gql-core module | ✅ | — | ❌ | **gql-core/src/gremlin/** | High |
| Protocol server as separate crate | ❌ | — | ✅ | **sedandelivery-gremlin/** | High |
| Compile to existing Operator tree | ✅ | ❌ | — | **Yes + 5 new operators** | High |
| Traverser as Row (not new type) | ✅ | ✅ | — | **Row with `__gremlin_*` keys** | Very High |
| Path tracking compile-time flag | ✅ | ✅ | — | **TraversalFlags.needs_path** | Very High |
| Auto-parameterize for plan cache | ✅ | — | — | **Canonical bytecode hash** | High |
| GraphSON v3 first | — | — | ✅ | **GraphSON v3, GraphBinary Phase 3** | High |
| Separate port 8182 | — | — | ✅ | **Yes (WebSocket conflict w/ Bolt)** | Very High |
| Reject gremlin-groovy | — | — | ✅ | **Error 597, matches Neptune** | Very High |
| Mutations in Phase 2 | ✅ | ❌ | — | **Phase 2 (maps to existing ops)** | High |
| VarLenExpand fast-path for repeat | ✅ | — | — | **Yes, for simple out().times(N)** | High |
| Lambda steps rejected | ✅ | — | ✅ | **Error 597** | Very High |
| SideEffect state in ExecContext | — | ✅ | — | **Optional gremlin_side_effects** | High |
| Bulk optimization deferred | — | ✅ | — | **Phase 4 (incompatible w/ path)** | High |
| Multi-label: first label exposed | — | — | ✅ | **First label + __labels__ prop** | Medium |
| Session TX via UUID scope | — | — | ✅ | **Phase 3 (maps to tx_snapshot)** | Medium |

### Key Conflicts Resolved

**Conflict: 13 new operators (A2) vs 4 new operators (A1)**  
Resolution: **5 new operators**. A2's `GremlinEdgeScan`, `GremlinEdgeVertex`, `GremlinMap`, `GremlinSelect`, `GremlinGroup`, `GremlinPath`, `GremlinBarrier`, `GremlinSack` all map to existing operators (`Expand`, `Project`, `Aggregate`, etc.). Only `Repeat`, `Choose`, `Coalesce`, `SideEffect`, and `Cap` are genuinely new computational patterns.

**Conflict: Gremlin frontend in gql-core (A1) vs new crate (A3)**  
Resolution: **Both are right, for different components**. The compiler must live in `gql-core` (needs direct `Operator` construction). The protocol server mirrors `sedandelivery-bolt/` as a separate crate. This follows the existing architecture exactly.

**Conflict: Mutations in Phase 2 (A1) vs Phase 4 (A2)**  
Resolution: **Phase 2**. Gremlin mutations (`addV`, `addE`, `drop`, `property`) map directly to existing `Create`, `Delete`, and `Set` operators. No new execution complexity.

---

## 2. Phased Roadmap

### Phase 0: Foundation (2 weeks)

**Goal**: Crate skeleton, type definitions, bytecode deserialization — no execution.

| Deliverable | Location | Description |
|-------------|----------|-------------|
| Gremlin IR types | `gql-core/src/gremlin/ir.rs` | `GremlinStep` enum (~65 variants), `TraversalFlags`, `HasArgs`, `Predicate`, `ByModulator` |
| Bytecode deserializer | `gql-core/src/gremlin/bytecode.rs` | GraphSON v3 bytecode object → `Vec<GremlinStep>`, modulator absorption pass |
| Value conversion | `sedandelivery-gremlin/src/bridge/types.rs` | `GremlinValue ↔ gql_core::Value` bidirectional |
| GraphSON v3 codec | `sedandelivery-gremlin/src/codec/graphson.rs` | Request decode, response encode (serde_json) |
| WebSocket listener | `sedandelivery-gremlin/src/main.rs` | Accept connections on port 8182, return error for all requests |

**Exit criteria**: `cargo test -p gql-core --lib` passes; `sedandelivery-gremlin` binds port 8182; bytecode round-trip test passes.

---

### Phase 1: Read-Only Traversals (4 weeks)

**Goal**: End-to-end execution of 20 read-only Gremlin steps via bytecode over WebSocket.

**Steps covered (20)**:
```
Source:  V(), E(), V(id), E(id)
Filter:  has(), hasLabel(), hasId(), hasNot(), where(), is(), not(), and(), or(), limit(), range(), dedup()
Map:     values(), valueMap(), id(), label(), count(), select()
Traversal: out(), in(), both(), outE(), inE(), bothE(), inV(), outV(), bothV(), otherV()
Modifier: as()
```

**Step → Operator Compilation Examples**:

```
g.V()                           → NodeScan { label: None, alias: __g0__ }
g.V(1)                          → NodeByIdSeek { id_expr: Int(1), alias: __g0__ }
g.V().has('name', 'marko')      → Filter { predicate: Eq(__g0__.name, 'marko'), child: NodeScan }
g.V().out('knows')              → Expand { dir: Out, types: [knows], src: __g0__, dst: __g1__, child: NodeScan }
g.V().out('knows').values('name') → Project { items: [(__g1__.name, "name")], child: Expand { ... } }
g.V().outE('knows').inV()       → Expand { dir: Out, types: [knows], src: __g0__, rel: __e0__, dst: __g1__ }
g.V().count()                   → Aggregate { keys: [], aggs: [(Count, ...)], child: NodeScan }
g.V().has('name','x').limit(10) → Limit { count: 10, child: Filter { ... } }
g.V().as('a').out().select('a') → Project { items: [(a, "a")], child: Expand { ... } }
```

**Exit criteria**: gremlinpython sends `g.V().has('name','Alice').out('knows').values('name')` as bytecode, receives correct JSON result. ≥50 integration tests pass.

---

### Phase 2: Complex Traversals + Mutations (5 weeks)

**Goal**: Advanced Gremlin semantics and write operations.

**New Operator variants added (5)**:
- `Repeat { body, until, emit, times, child }`
- `Choose { predicate, if_true, if_false, child }`
- `Coalesce { alternatives, child }`
- `SideEffect { channel, operation, value_expr, child }`
- `Cap { channels, child }`

**Steps covered (25 additional)**:
```
Branch:     repeat(), until(), emit(), times(), loops(), choose(), coalesce(), optional(), union()
Map:        path(), fold(), unfold(), order(), by(), project(), math()
SideEffect: aggregate(), store(), cap(), group(), groupCount()
Mutate:     addV(), addE(), property(), drop(), from(), to()
```

**Key implementation details**:
- `repeat(out()).times(3)` fast-path → `VarLenExpand { min: 3, max: 3 }` when body is simple traversal
- Complex repeat bodies → new `Repeat` operator with iterative execution
- `ExecContext` extended with `gremlin_side_effects: Option<&mut SideEffectState>`
- Path tracking: `__gremlin_path__` populated only when `TraversalFlags.needs_path` is set

**Exit criteria**: `repeat(out('knows')).times(3).path()` returns correct paths. `g.addV('Person').property('name','Alice')` creates nodes. ≥100 additional tests.

---

### Phase 3: Protocol Hardening + Compatibility (3 weeks)

**Goal**: Production-ready protocol, driver compatibility, text parsing.

| Feature | Description |
|---------|-------------|
| GraphBinary v1 codec | Binary wire format (~3x faster than GraphSON) |
| gremlin-lang text parser | Hand-written recursive descent for Gremlin Console support |
| TLS support | Reuse `sedandelivery-bolt`'s tokio-rustls setup |
| Session transactions | UUID-scoped, maps to `tx_snapshot()`/`tx_commit()` |
| Authentication | SASL PLAIN, shares mechanism with Bolt |
| Metrics | `gremlin_queries_total`, `gremlin_query_duration_seconds`, etc. |
| Docker Compose | Port 8182 exposed, health check on 8183 |
| Driver testing | gremlinpython full suite, Gremlin.NET basic, selective TCK |

**Exit criteria**: gremlinpython passes ≥90% non-lambda tests. GraphBinary round-trips all types. TLS works. gremlin-lang text parses and executes.

---

### Phase 4: Optimization (Ongoing)

- Bulk optimization (collapse identical traversers, flag-gated, incompatible with path tracking)
- `subgraph()` step
- `profile()` step
- Index-aware compilation (use `IndexScan` when `has()` matches indexed property)
- Gremlin-specific query plan optimization

---

### Timeline

| Phase | Duration | Cumulative | Milestone |
|-------|:--------:|:----------:|-----------|
| Phase 0 | 2 weeks | Week 2 | Crate compiles, bytecode round-trips |
| Phase 1 | 4 weeks | Week 6 | **First gremlinpython demo** |
| Phase 2 | 5 weeks | Week 11 | Full traversal + mutations |
| Phase 3 | 3 weeks | Week 14 | Production-ready |
| Phase 4 | Ongoing | — | Performance optimization |

---

## 3. Crate & Module Structure

```
SedanDelivery/
├── gql-core/
│   └── src/
│       ├── lib.rs                          # add: pub mod gremlin;
│       ├── gremlin/                        # NEW — Gremlin language frontend
│       │   ├── mod.rs                      # Public API: compile_bytecode()
│       │   ├── ir.rs                       # GremlinStep enum, TraversalFlags
│       │   ├── bytecode.rs                 # GraphSON bytecode → Vec<GremlinStep>
│       │   ├── compiler.rs                 # Vec<GremlinStep> → Operator tree
│       │   ├── parameterize.rs             # Auto-extract literals for plan cache
│       │   └── text.rs                     # gremlin-lang text parser (Phase 3)
│       ├── planner/
│       │   └── mod.rs                      # Operator enum (+5 new variants)
│       ├── executor/
│       │   └── engine/
│       │       ├── gremlin.rs              # NEW — Repeat/Choose/Coalesce/SideEffect/Cap execution
│       │       └── mod.rs                  # ExecContext extension
│       └── ...existing unchanged...
│
├── sedandelivery-gremlin/                  # NEW CRATE (mirrors sedandelivery-bolt/)
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs                         # Tokio server entry point
│       ├── connection/
│       │   ├── mod.rs                      # WebSocket listener
│       │   ├── handler.rs                  # Per-connection request multiplexer
│       │   └── session.rs                  # GremlinSession (optional TX scope)
│       ├── codec/
│       │   ├── mod.rs                      # GremlinCodec trait
│       │   ├── graphson.rs                 # GraphSON v3 (serde_json)
│       │   └── graphbinary.rs              # GraphBinary v1 (Phase 3)
│       ├── bridge/
│       │   ├── executor.rs                 # Bytecode → gql-core pipeline
│       │   └── types.rs                    # GremlinValue ↔ Value conversion
│       ├── message.rs                      # Request/Response types
│       ├── tls.rs                          # TLS (reuse bolt's pattern)
│       ├── metrics.rs                      # Prometheus counters
│       └── health.rs                       # Health check endpoint
│
├── Cargo.toml                              # Add sedandelivery-gremlin to members
└── ...existing crates unchanged...
```

**Dependency graph**:
```
sedandelivery-gremlin
├── gql-core          (bytecode, compiler, executor)
├── gql-redis-store   (storage backend)
├── tokio + tokio-tungstenite (async + WebSocket)
├── serde_json        (GraphSON v3)
├── uuid              (request IDs)
└── workspace deps    (tracing, metrics, thiserror, jiff, indexmap)
```

---

## 4. Type Architecture

### 4a. Gremlin Step IR

```rust
// gql-core/src/gremlin/ir.rs

/// A single Gremlin step after bytecode deserialization.
/// Modulators (.by(), .from(), .to(), .as()) are absorbed into parent steps
/// during a normalization pass — they are NOT separate steps in the IR.
#[derive(Debug, Clone)]
pub enum GremlinStep {
    // Source steps
    V(Option<Vec<Value>>),           // g.V() or g.V(1, 2, 3)
    E(Option<Vec<Value>>),           // g.E()

    // Traversal steps
    Out(Vec<Box<str>>),              // out('knows', 'likes')
    In(Vec<Box<str>>),
    Both(Vec<Box<str>>),
    OutE(Vec<Box<str>>),
    InE(Vec<Box<str>>),
    BothE(Vec<Box<str>>),
    InV, OutV, BothV, OtherV,

    // Filter steps
    Has(HasArgs),
    HasLabel(Vec<Box<str>>),
    HasId(Vec<Value>),
    HasNot(Box<str>),
    Where(WherePredicate),
    Is(Predicate),
    Not(Vec<GremlinStep>),
    And(Vec<Vec<GremlinStep>>),
    Or(Vec<Vec<GremlinStep>>),
    Limit(i64),
    Range(i64, i64),
    Dedup(Vec<Box<str>>),

    // Map steps
    Values(Vec<Box<str>>),
    ValueMap(bool, Vec<Box<str>>),   // include_tokens, keys
    Id, Label, Count, Sum, Min, Max, Mean,
    Fold, Unfold, Path,
    Select(Vec<Box<str>>),
    Project(Vec<Box<str>>),
    Order, Math(Box<str>), Constant(Value),

    // Branch steps
    Repeat(Vec<GremlinStep>),
    Until(Vec<GremlinStep>),
    Emit(Option<Vec<GremlinStep>>),
    Times(i64), Loops,
    Choose(ChooseArgs),
    Coalesce(Vec<Vec<GremlinStep>>),
    Optional(Vec<GremlinStep>),
    Union(Vec<Vec<GremlinStep>>),

    // Side-effect steps
    Aggregate(Box<str>), Store(Box<str>), Cap(Vec<Box<str>>),
    Group, GroupCount,

    // Mutation steps
    AddV(Option<Box<str>>),
    AddE(Box<str>),
    Property(PropertyArgs),
    Drop,

    // Modulators (absorbed during normalization)
    As(Box<str>), By(ByModulator), From(StepOrLabel), To(StepOrLabel),
}

/// Compile-time analysis flags — determines which traverser state to track.
#[derive(Debug, Default)]
pub struct TraversalFlags {
    pub needs_path: bool,           // path() or simplePath()/cyclicPath() used
    pub needs_sack: bool,           // sack() modulator present
    pub step_labels: Vec<Box<str>>, // all .as() labels
}
```

### 4b. Traverser-as-Row Convention

```rust
// No new Traverser struct. Row IS the traverser.

pub const GREMLIN_HEAD: &str = "__gremlin_head__";  // Current traverser value
pub const GREMLIN_PATH: &str = "__gremlin_path__";  // Traversal history (when needs_path)
pub const GREMLIN_SACK: &str = "__gremlin_sack__";  // Per-traverser accumulator (when needs_sack)

// Step labels stored directly:  row["a"] = Value::Node(42)
// No collision: __ prefix is rejected by Cypher parser, step labels are user strings
```

### 4c. Side Effect State

```rust
// gql-core/src/gremlin/side_effect.rs

pub struct SideEffectState {
    channels: HashMap<String, Vec<Value>>,
}

impl SideEffectState {
    pub fn aggregate(&mut self, channel: &str, value: Value) { /* push */ }
    pub fn cap(&mut self, channel: &str) -> Option<Vec<Value>> { /* take */ }
    pub fn get(&self, channel: &str) -> Option<&Vec<Value>> { /* peek */ }
}

// Integrated via optional field in ExecContext:
pub struct ExecContext<'a> {
    pub graph: &'a str,
    pub budget: &'a mut ExecutionBudget,
    pub outer_rows: &'a [Row],
    pub stats: &'a mut MutationStats,
    pub gremlin_side_effects: Option<&'a mut SideEffectState>, // NEW
}
```

### 4d. Path Tracking

```rust
// gql-core/src/gremlin/path.rs

#[derive(Debug, Clone)]
pub struct PathEntry {
    pub value: Value,
    pub labels: SmallVec<[Box<str>; 1]>,
}

pub struct TraversalPath {
    entries: Vec<PathEntry>,
}

impl TraversalPath {
    pub fn push(&mut self, value: Value, labels: &[&str]) { /* append */ }
    pub fn get_by_label(&self, label: &str) -> Option<&Value> { /* reverse search */ }
    pub fn is_simple(&self) -> bool { /* check for repeated graph elements */ }
    pub fn to_value(&self) -> Value { /* serialize to Value::List for Row transport */ }
    pub fn from_value(val: &Value) -> Option<Self> { /* deserialize */ }
}
```

---

## 5. Step Compilation Matrix

### Steps mapping to EXISTING Operators (no new code needed)

| Gremlin Step | Operator | Notes |
|:-------------|:---------|:------|
| `V()` | `NodeScan { label: None }` | |
| `V(id)` | `NodeByIdSeek` | |
| `V().hasLabel('X')` | `NodeScan { label: Some("X") }` | |
| `has('prop', val)` | `Filter { predicate: Eq(prop, val) }` | |
| `has('L','p','v')` | `IndexScan` when index exists | 3-arg form |
| `hasNot('prop')` | `Filter { predicate: IsNull(prop) }` | |
| `out('knows')` | `Expand { dir: Out, types: [knows] }` | |
| `in('knows')` | `Expand { dir: In, types: [knows] }` | |
| `both()` | `Expand { dir: Undirected }` | |
| `outE().inV()` | `Expand` (fused, with rel_alias) | Compiler fuses 2 steps |
| `values('name')` | `Project` | Property extraction |
| `valueMap()` | `Project` | All properties |
| `id()` / `label()` | `Project` | Token extraction |
| `count()` | `Aggregate { func: Count }` | |
| `sum()` / `min()` / `max()` / `mean()` | `Aggregate` | Existing AggFunction |
| `group().by().by()` | `Aggregate { keys + Collect }` | |
| `groupCount()` | `Aggregate { keys + Count }` | |
| `dedup()` | `Distinct` | |
| `order().by()` | `Sort` | .by() absorbed |
| `limit(n)` | `Limit` | |
| `skip(n)` / `range(lo,hi)` | `Skip` + `Limit` | |
| `fold()` | `Aggregate { func: Collect }` | Collect to list |
| `unfold()` | `Unwind` | |
| `union(t1, t2)` | `Union` | |
| `optional(t)` | `LeftOuterApply` | Try or identity |
| `addV('L').property(...)` | `Create` | Properties collected |
| `addE('L').from().to()` | `Create` (edge) | |
| `drop()` | `Delete { detach: true }` | Cascade |
| `as()` / `select()` | Row key aliasing + `Project` | |
| `match(patterns)` | `Apply` + `Expand` + `Filter` chains | |
| `where(P.gt(30))` | `Filter` | |
| `is(P.eq(42))` | `Filter` | |
| `not(traversal)` | `Filter` (negated) | |
| `and()` / `or()` | `Filter` (combined) | |

### Steps requiring NEW Operators

| Gremlin Step | New Operator | Why existing won't work |
|:-------------|:-------------|:-----------------------|
| `repeat().until().emit()` | `Repeat` | VarLenExpand is BFS-only, can't repeat arbitrary sub-traversals |
| `choose(pred, t, f)` | `Choose` | No conditional branching in current operators |
| `coalesce(t1, t2)` | `Coalesce` | N-way fallback (not just optional/null) |
| `aggregate('x')` / `store('x')` | `SideEffect` | Shared mutable state across steps — ForEach discards results |
| `cap('x')` | `Cap` | Materialize side-effect channel as result |

---

## 6. New Operators

```rust
// Added to gql-core/src/planner/mod.rs Operator enum

/// Gremlin repeat/until/emit loop.
/// For simple repeat(out()).times(N): compiler emits VarLenExpand as fast-path.
/// For complex bodies: uses this general iterator.
Repeat {
    body: Box<Operator>,
    until: Option<Expr>,
    emit: Option<Expr>,        // None = no emit, Some(true) = emit all
    times: Option<u32>,
    child: Box<Operator>,
},

/// Conditional branching (Gremlin choose step).
Choose {
    predicate: Expr,
    if_true: Box<Operator>,
    if_false: Option<Box<Operator>>,
    child: Box<Operator>,
},

/// First-succeeds fallback (Gremlin coalesce step).
Coalesce {
    alternatives: Vec<Operator>,
    child: Box<Operator>,
},

/// Accumulate into named side-effect channel.
SideEffect {
    channel: String,
    operation: SideEffectOp,   // Store, Aggregate, GroupCount
    value_expr: Expr,
    child: Box<Operator>,
},

/// Emit side-effect channel as result.
Cap {
    channels: Vec<String>,
    child: Box<Operator>,
},
```

Total Operator count: 28 existing + 5 new = **33 variants**.

---

## 7. Protocol Design

### Port Separation

| Protocol | Port | Transport | Reason |
|----------|:----:|-----------|--------|
| Bolt | 7687 | TCP + WebSocket | Existing, unchanged |
| Gremlin | 8182 | WebSocket only | TinkerPop convention; cannot share port because both use WS |

### Gremlin Server Messages

```rust
// Request (from driver)
pub struct GremlinRequest {
    pub request_id: Uuid,
    pub op: GremlinOp,          // Eval, Bytecode, Close, Authentication
    pub processor: String,       // "", "session", "traversal"
    pub args: GremlinArgs,       // gremlin (script/bytecode), bindings, aliases, etc.
}

// Response (to driver)
pub struct GremlinResponse {
    pub request_id: Uuid,
    pub status: GremlinStatus,   // code (200/204/206/401/500/597/598), message, attributes
    pub result: GremlinResult,   // data: Vec<GremlinValue>, meta: HashMap
}
```

### Session Model

Gremlin connections are **stateless multiplexers** (not Bolt's 8-state machine):

```rust
pub struct GremlinConnection {
    pub conn_id: u64,
    pub store: Arc<dyn GraphStore>,
    pub codec: Box<dyn GremlinCodec>,
    pub sessions: HashMap<Uuid, GremlinSession>,  // Optional TX sessions
    pub write_semaphore: Arc<Semaphore>,
}
```

### Response Streaming

Large results chunked via status codes:
- **206 PARTIAL_CONTENT** — batch of results, more coming
- **200 SUCCESS** — final batch, done

Server controls batch size (default 64 results per frame).

---

## 8. Value Type Mapping

| Gremlin Type | GremlinValue | gql-core Value | Notes |
|:-------------|:-------------|:---------------|:------|
| `g:Int32` | `Int32(i32)` | `Int(i64)` | Widen |
| `g:Int64` | `Int64(i64)` | `Int(i64)` | Direct |
| `g:Float` | `Float(f32)` | `Float(f64)` | Widen |
| `g:Double` | `Double(f64)` | `Float(f64)` | Direct |
| `g:String` | `String(String)` | `Str(Box<str>)` | |
| `g:UUID` | `Uuid(Uuid)` | `Str(Box<str>)` | Stored as string |
| `g:Date` | `Date(i64)` | `DateTime(Box<Zoned>)` | ms → jiff::Zoned |
| `g:Vertex` | `Vertex(GremlinVertex)` | `Value::Node(NodeId)` | Resolve via store |
| `g:Edge` | `Edge(GremlinEdge)` | `Value::Edge(EdgeId)` | Resolve via store |
| `g:Path` | `Path(GremlinPath)` | `Value::Path { nodes, edges }` | |
| `g:VertexProperty` | `VertexProperty(...)` | Entry in `NodeData.properties` | No meta-properties |
| `g:List` / `g:Set` | `List(Vec<...>)` | `Value::List(Vec<Value>)` | Set → deduped List |
| `g:Map` | `Map(Vec<(K,V)>)` | `Value::Map(IndexMap<...>)` | Non-string keys coerced |
| `g:T` | `T(GremlinToken)` | — | Resolved at compile time |
| `g:P` / `g:TextP` | `P(Predicate)` | — | Compiled to `Expr` filters |
| `g:Bytecode` | `Bytecode(...)` | — | Compiled to `Operator` tree |

**Multi-label handling**: Gremlin vertices have exactly 1 label. SedanDelivery nodes can have multiple. Convention: expose first label as the Gremlin label; all labels accessible via `values('__labels__')`.

---

## 9. Risk Register

| # | Risk | Severity | Likelihood | Mitigation |
|:-:|------|:--------:|:----------:|------------|
| 1 | **Repeat infinite loops** — `repeat(out())` without termination on cyclic graphs | Critical | Medium | Require `until()` OR `times()` at compile time. Runtime: `ExecutionBudget` row limit + timeout. Config: `max_repeat_depth` (default 100). |
| 2 | **Path memory explosion** — `path()` with deep `repeat()` on dense graphs | High | High | Track paths only when `needs_path` flag set. `ExecutionBudget.max_rows` caps output. Document `path()` + deep traversal as expensive. |
| 3 | **Row cloning cost** — HashMap cloned per traverser per step in high fan-out | High | Medium | Phase 1: accept cost (correctness first). Phase 4: bulk optimization, evaluate `Arc<Row>` or structural sharing. |
| 4 | **Multi-label impedance** — Gremlin `label()` returns 1 string; Cypher nodes have N labels | Medium | High | Return first label. Expose all via `__labels__` property. Document. Matches CosmosDB. |
| 5 | **GraphSON performance** — JSON serialization 3-5× slower than binary | Medium | Medium | Phase 1: GraphSON (debuggability). Phase 3: GraphBinary (performance). Drivers negotiate format. |

---

## Appendix: Changes to Existing Code

### Files Modified (minimal)

| File | Change | Scope |
|------|--------|-------|
| `gql-core/src/lib.rs` | Add `pub mod gremlin;` | 1 line |
| `gql-core/src/planner/mod.rs` | Add 5 Operator variants + `SideEffectOp` enum | ~30 lines |
| `gql-core/src/executor/engine/mod.rs` | Add `gremlin_side_effects` to ExecContext; add 5 match arms in `exec_operator()` | ~20 lines in mod.rs |
| `gql-core/src/plan_cache.rs` | Extend `CachedAst` enum for Gremlin plans | ~15 lines |
| `Cargo.toml` (workspace) | Add `sedandelivery-gremlin` to members | 1 line |

### Files Unchanged

- `gql-core/src/parser/` — Cypher parser untouched
- `gql-core/src/ast/` — Cypher AST untouched  
- `gql-core/src/storage/` — GraphStore trait untouched
- `gql-core/src/types/` — Value enum untouched
- `sedandelivery-bolt/` — Bolt server completely untouched
- All existing tests continue to pass
