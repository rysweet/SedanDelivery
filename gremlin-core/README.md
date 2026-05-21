# gremlin-core

Pure-Rust parser, bytecode model, and compiler for Apache TinkerPop Gremlin
traversals. No I/O, no runtime — just `&str` in, `Operator` tree out.

Part of the [SedanDelivery](https://github.com/cloud-ecosystem-security/SedanDelivery)
graph database engine.

## Getting started

Add the dependency:

```toml
[dependencies]
gremlin-core = { path = "../gremlin-core" }
cypher-core  = { path = "../cypher-core" }   # for Operator / Value types
```

Parse a traversal string into typed steps:

```rust
use gremlin_core::{parse_traversal, GremlinStep};
use gremlin_core::types::GremlinValue;

// No parameter bindings needed for literals
let no_params = |_: &str| -> Option<GremlinValue> { None };

let steps = parse_traversal(
    r#"g.V().has("name","Alice").out("knows").values("name")"#,
    &no_params,
).expect("parse succeeds");

assert!(matches!(steps[0], GremlinStep::V(None)));
// steps: [V, Has("name", Some("Alice")), Out(["knows"]), Values(["name"])]
```

Compile steps into an `Operator` tree:

```rust
use gremlin_core::{parse_traversal, GremlinCompiler};
use gremlin_core::types::GremlinValue;

let no_params = |_: &str| -> Option<GremlinValue> { None };
let steps = parse_traversal("g.V().count()", &no_params).unwrap();

let mut compiler = GremlinCompiler::new();
let operator = compiler.compile(&steps).expect("compile succeeds");
// `operator` is a cypher_core::planner::Operator tree ready for execution
```

## Architecture

```text
                  text_parser            compiler
  "g.V().count()"  -------->  [V, Count]  -------->  Operator tree
                                                        |
                  graphson              bytecode        |
  GraphSON JSON  -------->  RequestMsg  -------->  [GremlinStep]
```

### Modules

| Module | Purpose |
|--------|---------|
| `text_parser` | Recursive-descent parser: Gremlin string to `Vec<GremlinStep>` |
| `bytecode` | `GremlinStep` enum (~80 step variants), `Predicate`, `Pop` |
| `compiler` | Transforms step sequences into `cypher_core::planner::Operator` trees |
| `types` | `GremlinValue` / `Traverser` with bidirectional `Value` conversions |
| `graphson` | GraphSON v3 request/response message types and serde support |
| `prelude` | Convenience re-exports: `use gremlin_core::prelude::*` |

### Key types

- **`GremlinStep`** -- Enum with ~80 variants covering source steps (`V`, `E`),
  filters (`Has`, `HasLabel`), traversals (`Out`, `In`, `Both`), mutations
  (`AddV`, `AddE`, `Property`, `Drop`, `MergeV`, `MergeE`), aggregations
  (`Count`, `Sum`, `Group`, `Fold`), branching (`Repeat`, `Choose`,
  `Coalesce`, `Union`), string operations (`ToLower`, `Split`, `Concat`),
  collection operations (`Combine`, `Intersect`, `Difference`), and more.

- **`Predicate`** -- Comparison operators: `Eq`, `Neq`, `Gt`, `Gte`, `Lt`,
  `Lte`, `Within`, `Without`, `Between`, `Inside`, text predicates
  (`Containing`, `StartingWith`, `Regex`), and compound predicates
  (`And`, `Or`, `Not`).

- **`GremlinValue`** -- Gremlin's type system: `Null`, `Bool`, `Integer`,
  `Float`, `String`, `List`, `Map`, `Vertex`, `Edge`, `Path`. Implements
  `From<Value>` and `Into<Value>` for seamless conversion with cypher-core.

- **`GremlinCompiler`** -- Stateful compiler that walks steps left-to-right,
  building a bottom-up `Operator` tree. Handles `as()` label aliasing,
  `ProductiveByStrategy`, `ReadOnlyStrategy`, and `SeedStrategy`.

- **`RequestMessage` / `ResponseMessage`** -- GraphSON v3 wire format types
  with `serde` support for server integration.

## Advanced usage

### Parameter bindings

The text parser supports TinkerPop-style parameter references (`vid1`, `xx1`, etc.):

```rust
use gremlin_core::{parse_traversal, GremlinStep};
use gremlin_core::types::GremlinValue;
use std::collections::HashMap;

let mut params = HashMap::new();
params.insert("vid1".to_string(), GremlinValue::Integer(42));

let resolve = |name: &str| -> Option<GremlinValue> {
    params.get(name).cloned()
};

let steps = parse_traversal("g.V(vid1).values(\"name\")", &resolve).unwrap();
// V(Some([Integer(42)])), Values(["name"])
```

### Predicate compilation

Predicates map to comparison expressions in the operator tree:

```rust
use gremlin_core::{parse_traversal, GremlinCompiler};
use gremlin_core::types::GremlinValue;

let no_params = |_: &str| -> Option<GremlinValue> { None };

// Range predicates
let steps = parse_traversal(
    r#"g.V().has("age", P.between(20, 30))"#,
    &no_params,
).unwrap();

let mut compiler = GremlinCompiler::new();
let op = compiler.compile(&steps).unwrap();
```

### GraphSON deserialization

Deserialize a GraphSON v3 request message:

```rust
use gremlin_core::graphson::RequestMessage;

let json = serde_json::json!({
    "requestId": "cb682578-9d92-4499-9ebc-5c6aa73c5397",
    "op": "bytecode",
    "processor": "traversal",
    "args": {
        "gremlin": {
            "@type": "g:Bytecode",
            "@value": { "step": [["V"], ["count"]] }
        }
    }
});

let msg: RequestMessage = serde_json::from_value(json).unwrap();
assert_eq!(msg.op, "bytecode");
```

### Error handling

Both parsing and compilation return typed errors:

```rust
use gremlin_core::{parse_traversal, TextParseError, GremlinCompiler};
use gremlin_core::compiler::CompileError;
use gremlin_core::types::GremlinValue;

let no_params = |_: &str| -> Option<GremlinValue> { None };

// Parse errors — including trailing unparsed input
match parse_traversal("g.V().count())", &no_params) {
    Err(TextParseError::InvalidArgument(msg)) => {
        eprintln!("trailing input: {msg}");
    }
    Err(e) => eprintln!("parse error: {e}"),
    Ok(_) => {}
}

// Inner traversal errors propagate (never silently swallowed)
match parse_traversal(r#"g.V().has("name", P.containing(42, "extra"))"#, &no_params) {
    Err(e) => eprintln!("inner parse error: {e}"),
    Ok(_) => {}
}

// Compile errors — unsupported steps are always reported
let steps = parse_traversal("g.V().count()", &no_params).unwrap();
let mut compiler = GremlinCompiler::new();
match compiler.compile(&steps) {
    Ok(operator) => { /* use operator */ }
    Err(CompileError::UnsupportedStep(s)) => {
        eprintln!("step not yet supported: {s}");
    }
    Err(e) => eprintln!("compile error: {e}"),
}
```

**Error guarantees:**

- **No silent drops.** Every parse error, compile error, and execution
  error propagates to the caller. Inner traversals, `by()` modulators,
  and property resolution all use `?` propagation.
- **Trailing input rejected.** Unbalanced parentheses or extra tokens
  after a complete parse produce `InvalidArgument`.
- **Checked arithmetic.** `group().by(sum)` uses `checked_add` and
  returns `IntegerOverflow` on `i64` overflow instead of wrapping.
- **Type-safe merges.** Map entry merging during `group()` and `repeat()`
  rejects incompatible value types with `TypeMismatch`.

See [`docs/gremlin-error-handling.md`](../docs/gremlin-error-handling.md)
for the complete error handling reference and
[`docs/gremlin-quote-handling.md`](../docs/gremlin-quote-handling.md)
for string quoting and escape rules in the text parser.

### Value conversions

`GremlinValue` converts to/from `cypher_core::types::Value`:

```rust
use gremlin_core::types::GremlinValue;
use cypher_core::types::Value;

// GremlinValue -> Value
let gv = GremlinValue::Integer(42);
let v: Value = gv.into();
assert_eq!(v, Value::Int(42));

// Value -> GremlinValue
let v = Value::Str("hello".into());
let gv: GremlinValue = v.into();
assert_eq!(gv, GremlinValue::String("hello".to_string()));
```

## Supported Gremlin steps

The parser and compiler cover the following TinkerPop step categories:

| Category | Steps |
|----------|-------|
| Source | `V()`, `E()` |
| Filter | `has()`, `hasLabel()`, `hasId()`, `hasKey()`, `hasValue()`, `where()`, `not()`, `and()`, `or()`, `is()`, `simplePath()`, `cyclicPath()`, `coin()`, `filter()` |
| Traverse | `out()`, `in()`, `both()`, `outE()`, `inE()`, `bothE()`, `inV()`, `outV()`, `bothV()`, `otherV()`, `toE()` |
| Map | `values()`, `valueMap()`, `elementMap()`, `properties()`, `id()`, `label()`, `select()`, `project()`, `path()`, `constant()`, `identity()`, `element()`, `map()`, `flatMap()`, `math()` |
| Aggregate | `count()`, `sum()`, `min()`, `max()`, `mean()`, `group()`, `groupCount()`, `fold()`, `unfold()` |
| Order | `order()`, `by()`, `limit()`, `range()`, `tail()`, `skip()`, `dedup()`, `sample()` |
| Mutate | `addV()`, `addE()`, `property()`, `from()`, `to()`, `drop()`, `mergeV()`, `mergeE()` |
| Branch | `repeat()`, `times()`, `until()`, `emit()`, `choose()`, `coalesce()`, `union()`, `optional()`, `match()` |
| Side-effect | `as()`, `sideEffect()`, `aggregate()`, `cap()`, `inject()` |
| String | `toLower()`, `toUpper()`, `trim()`, `lTrim()`, `rTrim()`, `length()`, `reverse()`, `replace()`, `split()`, `substring()`, `concat()`, `asString()`, `format()`, `conjoin()` |
| Collection | `combine()`, `intersect()`, `difference()`, `disjunct()`, `merge()`, `product()`, `all()`, `any()`, `none()` |
| Type | `asNumber()`, `asBool()`, `typeOf()`, `asDate()` |
| Date | `dateAdd()`, `dateDiff()` |
| Control | `barrier()`, `index()`, `sack()`, `withSack()`, `withSideEffect()`, `withBulk()`, `loops()`, `discard()` |
| Strategy | `ProductiveByStrategy`, `ReadOnlyStrategy`, `SeedStrategy`, `ReservedKeysVerificationStrategy`, `EdgeLabelVerificationStrategy` |

Steps parsed but not yet compiled: `tree()`, `subgraph()`, `call()`,
`shortestPath()`, `pageRank()`, `connectedComponent()`, `peerPressure()`,
`io()`, `dateAdd()`, `dateDiff()`, `toE()`.

## Benchmarks

Run the parse/compile microbenchmarks:

```bash
cargo bench -p gremlin-core
```

This benchmarks parsing and compilation for representative traversal patterns
(simple count, has-filter, traversal chains, mutations, aggregations).

## License

MIT OR Apache-2.0
