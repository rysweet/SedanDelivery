# Gremlin Error Handling

SedanDelivery's Gremlin pipeline surfaces errors explicitly at every stage —
parsing, compilation, and execution — instead of silently dropping them.
This document describes the error guarantees, the error types involved,
and how to handle them in application code.

## Design Principle

> **No silent failures.** Every error propagates to the caller.
> If a Gremlin step cannot be compiled, parsed, or executed, the
> response is an error — never a quietly wrong result.

This applies across both the text parser (`gremlin-core`) and the
Gremlin execution engine (`cypher-core`).

---

## Error Types

### Parse Errors (`TextParseError`)

Returned by `parse_traversal()` and the text parser internals.

| Variant | Meaning |
|---------|---------|
| `UnexpectedEnd` | Input ended before a complete step could be parsed |
| `Expected { expected, found }` | Token not expected at this position |
| `UnknownStep(String)` | Step name not recognized |
| `InvalidArgument(String)` | Argument parsing failed (type, count, or format) |

**Trailing input detection.** After successfully parsing a Gremlin query,
the tokenizer validates that no unparsed input remains. Unbalanced
parentheses or trailing tokens produce an `InvalidArgument` error with a
message describing the position and nature of the leftover input. This
prevents queries like `g.V().count())` from silently ignoring the trailing
`)`.

**Inner traversal errors.** When parsing nested traversals (e.g., the
predicate argument to `has()`, the body of `repeat()`, or a `by()`
modulator), parse failures propagate immediately via `?` rather than
being swallowed. A malformed inner traversal produces the same error the
caller would see for a malformed top-level traversal.

```rust
use gremlin_core::{parse_traversal, TextParseError};
use gremlin_core::types::GremlinValue;

let no_params = |_: &str| -> Option<GremlinValue> { None };

// Trailing input is rejected
let result = parse_traversal("g.V().count())", &no_params);
assert!(result.is_err());

// Inner traversal errors propagate
let result = parse_traversal(
    r#"g.V().has("name", P.containing(42, "extra"))"#,
    &no_params,
);
assert!(result.is_err());
```

### Compile Errors (`CompileError`)

Returned by `GremlinCompiler::compile()`.

| Variant | Meaning |
|---------|---------|
| `UnsupportedStep(String)` | Step has no Cypher/Operator equivalent |
| `MissingChild` | Step requires a preceding traversal but none was provided |
| `InvalidSequence(String)` | Steps appear in an invalid order |
| `ReadOnlyViolation` | Mutating step used in a read-only traversal |
| `ReservedKeyViolation(String)` | Property key is a reserved word |
| `EdgeLabelRequired` | Vertex step used without specifying an edge label |

**Unsupported step detection.** When the compiler encounters a Gremlin
step that has no mapping to a `cypher-core` Operator, it returns
`CompileError::UnsupportedStep` with the step name. This applies
consistently across all compilation paths, including `by()`-modulated
steps in `project()` and `group()`.

Previously, some `by()` modulators in `project()` and `group()` context
used `if let Ok(...)` patterns that silently ignored compilation failures
for individual modulator steps. Now, every modulator compilation failure
propagates as a `CompileError` (typically `UnsupportedStep` or
`InvalidSequence`).

```rust
use gremlin_core::{parse_traversal, GremlinCompiler};
use gremlin_core::compiler::CompileError;
use gremlin_core::types::GremlinValue;

let no_params = |_: &str| -> Option<GremlinValue> { None };
let steps = parse_traversal(
    "g.V().project(\"x\").by(pageRank())",
    &no_params,
).unwrap();

let mut compiler = GremlinCompiler::new();
match compiler.compile(&steps) {
    Err(CompileError::UnsupportedStep(s)) => {
        // "pageRank" — clearly reported, not silently dropped
        assert!(s.contains("pageRank") || s.contains("unsupported"));
    }
    other => panic!("expected UnsupportedStep, got {other:?}"),
}
```

### Execution Errors (`ExecutionError` / `GqlError`)

Returned by the Gremlin execution engine inside `cypher-core`.

| Variant | Meaning |
|---------|---------|
| `TypeMismatch { expected, actual }` | Operation applied to wrong type |
| `IntegerOverflow { context }` | Checked arithmetic exceeded `i64` range |
| `DivisionByZero` | Division or modulus by zero |
| `IndexOutOfBounds { index, len }` | List access out of range |
| `UnsupportedFeature { feature }` | Runtime feature not implemented |
| *(others)* | See `cypher-core::error::ExecutionError` |

All execution errors are wrapped as `GqlError::Execution(...)` and
propagated through the query result.

---

## Specific Error Guarantees

### Integer Overflow in `group().by(sum)`

The `group()` step with `sum` aggregation uses checked arithmetic
(`i64::checked_add`). If the sum exceeds `i64::MAX` or falls below
`i64::MIN`, the query returns `ExecutionError::IntegerOverflow` with
context `"integer overflow in Gremlin group sum"`.

This prevents silent wraparound that could produce negative totals from
all-positive input.

```rust
// Conceptual example — this traversal would overflow i64
// g.inject(9223372036854775800, 100).group().by(constant("k")).by(sum)
// → ExecutionError::IntegerOverflow { context: "integer overflow in Gremlin group sum" }
```

### Type-Safe Map Merging

When the Gremlin engine merges map entries during `group()`, `repeat()`,
or other aggregating steps, values under the same key must have compatible
types. Merging an `Integer` with a `String`, or a `List` with a `Map`,
returns `ExecutionError::TypeMismatch` rather than silently choosing one
value or the other.

The function `merge_gremlin_map_entry` returns `Result<(), GqlError>`,
and all callers propagate errors via `?`.

**Compatible merge operations:**

| Existing Value | New Value | Result |
|---------------|-----------|--------|
| `Integer` | `Integer` | Sum (checked) |
| `Float` | `Float` | Sum |
| `List` | `List` | Concatenation |
| any | any (no special rule) | Replaced |
| any | different type | **Error** (after fix) |

### Storage Error Propagation in Property Resolution

The functions `resolve_in_values_min`, `apply_path_by_key`, and
`resolve_prop_on_value` return `Result<Value, GqlError>`. Storage
backend failures (e.g., Redis connection loss during property lookup)
propagate as `GqlError::Storage(...)` rather than being silently
converted to `Value::Null`.

This means a query that encounters a storage error mid-traversal will
fail with an explicit error rather than returning `null` values that
look like missing properties.

```
// Before (silent failure):
// Storage error → Value::Null → query returns null for "name"
// User thinks the property doesn't exist

// After (explicit failure):
// Storage error → GqlError::Storage(...) → query fails with error message
// User knows something went wrong with storage
```

---

## Error Handling in Applications

### Gremlin Text Server

The `sedandelivery-gremlin-server` binary parses Gremlin text queries
and returns errors as GraphSON error responses. All parse errors,
including trailing unparsed input, are reported in the `message` field
of the error response.

### Gremlin Bytecode Server

The `sedandelivery-gremlin` binary processes bytecode requests.
Compilation and execution errors are returned as GraphSON status code
500 with the error message in the response.

### Embedded Engine

The `sedan-gremlin-embedded` library returns `Result<Vec<Value>, GqlError>`
from its query method. Pattern-match on the error variants to distinguish
parse, compile, and execution failures:

```rust
use sedan_gremlin_embedded::GremlinEngine;
use cypher_core::error::{GqlError, ExecutionError};

let engine = GremlinEngine::new();
match engine.query("g.V().group().by(label).by(sum)") {
    Ok(results) => { /* process results */ }
    Err(GqlError::Execution(ExecutionError::IntegerOverflow { context })) => {
        eprintln!("overflow: {context}");
    }
    Err(GqlError::Execution(ExecutionError::TypeMismatch { expected, actual })) => {
        eprintln!("type error: expected {expected}, got {actual}");
    }
    Err(e) => eprintln!("query failed: {e}"),
}
```

---

## Relationship to the Silent-Failure Audit

These fixes are part of the broader silent-failure audit documented in
[`docs/audit/silent-failure-audit.md`](audit/silent-failure-audit.md).
The audit categorizes every `unwrap_or_else(|_| ...)` and similar pattern
in the codebase. The six bugs addressed here were classified as
**`silent-bug`** — sites where a real error was eaten and a default
returned, masking genuine failures.

| Issue | Category | Fix |
|-------|----------|-----|
| [#1192](https://github.com/cloud-ecosystem-security/SedanDelivery/issues/1192) | silent-bug | `by()` modulators in `project()`/`group()` propagate `CompileError` |
| [#1193](https://github.com/cloud-ecosystem-security/SedanDelivery/issues/1193) | silent-bug | `parse_inner_traversal` propagates `TextParseError` |
| [#1194](https://github.com/cloud-ecosystem-security/SedanDelivery/issues/1194) | silent-bug | `resolve_in_values_min` propagates `GqlError::Storage` |
| [#1199](https://github.com/cloud-ecosystem-security/SedanDelivery/issues/1199) | silent-bug | Tokenizer rejects trailing unparsed input |
| [#1205](https://github.com/cloud-ecosystem-security/SedanDelivery/issues/1205) | silent-bug | `group()` sum uses `checked_add` → `IntegerOverflow` |
| [#1206](https://github.com/cloud-ecosystem-security/SedanDelivery/issues/1206) | silent-bug | `merge_gremlin_map_entry` rejects cross-type merges |

---

## API Reference

### New `ExecutionError` Variant

```rust
#[derive(Debug, Clone, Error)]
pub enum ExecutionError {
    // ... existing variants ...

    #[error("integer overflow: {context}")]
    IntegerOverflow { context: String },
}
```

### Changed Function Signatures

Functions that previously returned bare values or `()` will return `Result`
after the fix. These are internal functions — not part of the public API.

```rust
// cypher-core::executor::engine::gremlin::helpers::value_format
// Visibility: pub(in crate::executor::engine::gremlin)
fn merge_gremlin_map_entry(
    map: &mut IndexMap<Box<str>, Value>,
    key: &str,
    incoming: &Value,
) -> Result<(), GqlError>;  // was: ()

// Methods on QueryExecutor<'a> in select_helpers.rs
// Visibility: pub(super) / fn (private)
impl<'a> QueryExecutor<'a> {
    fn resolve_in_values_min(
        &self,
        cx: &mut ExecContext<'_>,
        rel_type: &str,
        prop: &str,
        val: &Value,
    ) -> Result<Value, GqlError>;  // was: Value

    pub(super) fn apply_path_by_key(
        &self,
        cx: &mut ExecContext<'_>,
        key: &str,
        val: &Value,
    ) -> Result<Value, GqlError>;  // was: Value

    pub(super) fn resolve_prop_on_value(
        &self,
        cx: &mut ExecContext<'_>,
        prop_name: &str,
        val: &Value,
    ) -> Result<Value, GqlError>;  // was: Value
}
```

All callers of these functions use `?` to propagate errors. Test code
uses `.unwrap()` for the expected-success paths.
