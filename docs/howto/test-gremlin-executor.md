---
title: Test the Gremlin Executor
description: How to write, run, and extend unit tests for cypher-core's Gremlin operator engine.
last_updated: 2026-05-21
review_schedule: as-needed
doc_type: how-to
---

# Test the Gremlin Executor

The Gremlin executor lives at `cypher-core/src/executor/engine/gremlin/` and
implements every Gremlin traversal step as an `Operator` variant executed by
`QueryExecutor`. Its test suite (`tests.rs` in the same directory) exercises
these operators in isolation using `InMemoryGraphStore`, without requiring
Redis, a network stack, or the Gremlin text parser.

This guide covers how to run the tests, how to add new ones, and what
conventions the suite follows.

## Running the tests

```bash
# All gremlin executor unit tests
cargo test -p cypher-core --lib -- executor::engine::gremlin::tests

# A single test by name
cargo test -p cypher-core --lib -- executor::engine::gremlin::tests::barrier_passthrough

# With output (useful for debugging)
cargo test -p cypher-core --lib -- executor::engine::gremlin::tests --nocapture
```

## Measuring coverage

```bash
# Line coverage for cypher-core (includes gremlin executor)
cargo llvm-cov -p cypher-core --lib --summary-only

# HTML report for visual inspection
cargo llvm-cov -p cypher-core --lib --html --output-dir target/llvm-cov/html/cypher-core
open target/llvm-cov/html/cypher-core/html/index.html
```

The CI floor script (`scripts/coverage_floor_check.sh`) prevents regressions.
See [docs/COVERAGE.md](../COVERAGE.md) for targets and methodology.

## Test conventions

### Helpers

Two shared helpers at the top of `tests.rs` create the test fixtures every
test needs:

| Helper        | What it creates                                          |
| ------------- | -------------------------------------------------------- |
| `make_store()`| Empty `InMemoryGraphStore` with one graph named `"g"`    |
| `make_chain()`| Three `:N` nodes A→B→C linked by `:NEXT` edges; returns `InMemoryGraphStore` |

Most tests call one of these, construct an `Operator` tree, and execute it
through `QueryExecutor::new(&store).execute("g", &plan, &mut budget)`.

### Operator construction

All Gremlin `Operator` variants have public fields — construct them directly
as struct literals. Do **not** go through the Gremlin text parser or
`gremlin-core` compiler; that creates coupling to upstream changes and makes
failures harder to diagnose.

```rust
let plan = Operator::GremlinBarrier {
    sack_merge_op: None,
    norm_sack: false,
    with_bulk: false,
    current_alias: Identifier::new("n"),
    out_alias: Identifier::new("n"),
    child: Box::new(Operator::NodeScan {
        label: Some(Label::new("N")),
        alias: Identifier::new("n"),
    }),
};
```

### Section organisation

Tests are grouped by operator family under ASCII banner comments:

```rust
// ── GremlinBarrier tests ────────────────────────────────────────────────
```

When adding tests for a new operator, add a new banner. Keep test function
names descriptive: `barrier_norm_sack_sum`, `sample_deterministic_seed`,
`simple_path_filters_duplicates`.

### Assertions

Assert on `result.rows` (a `Vec<IndexMap<String, Value>>`). Check both the
number of rows and the values in specific columns:

```rust
let result = executor.execute("g", &plan, &mut budget).unwrap();
assert_eq!(result.rows.len(), 3);
assert_eq!(result.rows[0]["name"], Value::Str("A".into()));
```

For operators that produce non-deterministic output (e.g. `GremlinSample`),
use the `seed` field (`seed: Some(42)`) for reproducible results.

## Operator coverage map

The test suite covers the following operator groups. Each section below
describes the operators tested, the key scenarios, and how to extend coverage.

### GremlinBarrier (8 tests)

Barrier steps collect all input rows before emitting them. Tests cover:

- **Passthrough** — no merge op, rows pass unchanged.
- **sack_merge_op variants** — Sum, Min, Max, Mult, Div, Assign — each test
  sets `norm_sack: true` with the corresponding `sack_merge_op` string,
  injects `__sack` values via upstream rows, and verifies the merged result.
- **with_bulk** — sets `with_bulk: true` and validates bulk-count preservation
  through the barrier.

Extend by: adding tests for empty input (zero rows through barrier).

### GremlinSample (4 tests)

Random sampling from the traversal stream. Tests cover:

- **Uniform sampling** — `count` rows sampled from a larger input.
- **Weighted sampling** — `weight: Some(Expr)` biases selection toward specific rows.
- **count > input size** — returns all rows when `count` exceeds input.
- **Deterministic seed** — `seed: Some(42)` produces identical results across
  runs.

Extend by: testing `weight` expression pointing to a missing property
(should treat weight as 0 or skip).

### GremlinIndex (4 tests)

The `GremlinIndex` operator unwraps indexed entries from List and Map
collections. Tests cover:

- **with_map=true, list input** — index a list, emit per-element maps
  `{key: idx, value: el}`.
- **with_map=false, list input** — index a list, emit `[idx, el]` pairs.
- **with_map=true, stream input** — index a stream of scalar values, same as
  list→map but from non-collection rows.
- **with_map=false, stream input** — stream variant producing `[idx, el]`
  pairs.

Extend by: testing with an empty list/map input.

### GremlinProject (3 tests)

Multi-key projection using `by(...)` modulator traversals. Tests cover:

- **Basic multi-key** — two or more projection keys, each with its own
  traversal.
- **productive: false** — keys that produce Null are dropped from the output
  map.
- **productive: true** — Null values are preserved in the output map.

### GremlinMapSlice (3 tests)

Slices into Map or List values. Tests cover:

- **Map slice** — extracts a range of entries from a Map.
- **List slice** — extracts a sub-list by start/end index.
- **Non-collection passthrough** — scalar values pass through unchanged.

### GremlinSimplePath / GremlinCyclicPath (6 tests)

Path-based filters that examine the `__gremlin_path_acc` column.

**SimplePath** (3 tests):
- **Identity** — all path elements unique → row retained.
- **Duplicate filter** — repeated element in path → row dropped.
- **by_keys** — comparison uses projected keys instead of raw path objects.

**CyclicPath** (3 tests):
- **No cycle** — all unique path elements → row dropped (no cycle detected).
- **Has cycle** — repeated element → row retained.
- **by_keys** — same as SimplePath, but keep/drop logic is inverted.

These tests seed `__gremlin_path_acc` by placing a `GremlinPath` step
upstream.

### GremlinTree (2 tests)

Builds a tree structure from parent-child path relationships.

- **Barrier mode** (`side_effect_key: None`) — emits a single row containing
  a nested Map tree.
- **Side-effect mode** (`side_effect_key: Some(...)`) — stores the tree in a
  side-effect register and passes rows through unchanged.

### GremlinAggregate gaps (5 tests)

Supplements the existing Aggregate+Cap tests with previously-untested
branches:

- **by_keys** — aggregate values projected by `by(...)` instead of the
  default element.
- **set_semantics** — duplicates are ignored (BulkSet with unique constraint).
- **assign_semantics** — each new value replaces the previous.
- **by_traversal** — aggregation key comes from a nested traversal.
- **productive** — aggregate step marks itself productive/unproductive.

### GremlinGroup (6 tests)

Grouping with reduce semantics. Tests cover:

- **count** — `agg_fn: AggFunction::Count` — values are group cardinalities.
- **collect** — `agg_fn: AggFunction::Collect` — values are lists of elements.
- **sum** — `agg_fn: AggFunction::Sum` — numeric reduction per group.
- **side_effect** — `side_effect_key: Some(...)` stores the map in a register
  rather than emitting it as a row.
- **null key** — rows with Null group key go into a `Null` bucket
  (`key_is_identity: true`).
- **key_traversal** — group key derived from a `key_traversal` sub-traversal.

### GremlinSelect (4 tests)

Step label lookup. Tests cover:

- **Single key** — `keys` contains one pair, extracts one label.
- **Multi-key** — `keys` contains multiple pairs, produces a Map.
- **Pop first/last** — `pop: Some(GremlinPop::First)` /
  `pop: Some(GremlinPop::Last)` pick the first or last bound value for a
  label via `label_stacks`.
- **productive: true** — missing labels produce Null instead of filtering the
  row.

### Branch / Choose gaps (4 tests)

Control-flow operators. Supplements existing Choose/Coalesce tests:

- **ChooseMap default branch** — the `None` match-value option fires when no
  other predicate matches.
- **Unproductive predicate** — a Choose branch that produces zero rows
  does not contribute to output.
- **Choose output_alias** — verifies the output column name binding.
- **Coalesce all-empty** — when every branch produces zero rows, the
  overall result is zero rows.

### Repeat gaps (4 tests)

Loop operators. Supplements existing Repeat tests:

- **until predicate** — `until: Some(...)` terminates the loop when predicate
  is satisfied.
- **emit_before: true** — emit rows at the start of each iteration (not just
  the end).
- **loop_label** — `loop_label: Some(...)` stores a named loop counter
  accessible via `loops("name")`.
- **times_before: true** — acts as a while-loop (check count before first
  iteration).

## Adding a new operator test

1. Find (or create) the section banner for the operator family.
2. Write the test using the `make_store()` or `make_chain()` helpers.
3. Construct the `Operator` directly — no parser round-trip.
4. Execute via `QueryExecutor::new(&store).execute("g", &plan, &mut budget)`.
5. Assert on `result.rows`.
6. Run `cargo test -p cypher-core --lib` and `cargo clippy -- -D warnings`.
7. Check coverage impact:
   `cargo llvm-cov -p cypher-core --lib --summary-only`.

## Related documentation

- [docs/COVERAGE.md](../COVERAGE.md) — coverage targets, CI enforcement, and
  baseline numbers.
- [AGENTS.md](../../AGENTS.md) — project-wide contributor conventions.
- [Gremlin implementation plan](../gremlin-implementation-plan.md) — which
  Gremlin steps map to which `Operator` variants.
