# Differential Testing

A cross-engine equivalence harness that asserts SedanDelivery's three
query engines — `cypher-core` (openCypher), `gql-engine` (GQL), and
`gremlin-core` (Apache TinkerPop Gremlin) — return **semantically
equivalent results** for a curated set of queries on a shared in-memory
graph fixture.

- **Crate**: [`tests/differential/`](../tests/differential/)
- **Runner script**: [`scripts/run_differential_tests.sh`](../scripts/run_differential_tests.sh)
- **CI**: [`.github/workflows/differential.yml`](../.github/workflows/differential.yml)
- **Local invocation**: `cargo test -p differential -- --nocapture`

## Why this gate exists

The three engines share the same `cypher_core::storage::GraphStore`
abstraction and emit results as `cypher_core::types::Value`, but each
engine has its own parser, planner, and executor. That means **any of
them can drift from the others** without breaking its own tests:

- A WHERE-clause optimization in `cypher-core` may incorrectly drop rows.
- A GQL keyword rewrite may change comparator semantics for one operator.
- A Gremlin step may emit traversers in a different shape than expected.

Recent incidents showed how easy this kind of drift is to miss:

- **PR #1680** — UTF-8 panics in the GQL rewrite tokenizer and the
  Gremlin literal parser. Each engine's own tests passed; the bug only
  manifested when comparing engines against the same input.
- **PR #1685** — silent test failures in `sd2-redis-store` after a file
  decomposition. Audits passed, but the actual storage path regressed.

Differential testing closes the gap by treating the three engines as
*independent oracles* for one another. If two engines agree and one
disagrees, the disagreeing engine is almost certainly wrong (or the
queries are not actually equivalent — also worth knowing).

## How the harness works

1. **Fixture** ([`fixture.rs`](../tests/differential/src/fixture.rs))
   builds a 50-node, ~95-edge graph in an `InMemoryGraphStore` with a
   mix of labels (`Person`, `City`, `Company`, `Project`, `Tag`),
   relationship types (`KNOWS`, `LIVES_IN`, `WORKS_AT`, `OWNS`, `TAGGED`,
   `COLLAB`), and property datatypes (strings, ints, floats, booleans,
   lists, dates, nulls / missing). The shape is locked by the
   `fixture_shape` test so accidental edits are caught.

2. **Cases** ([`cases.rs`](../tests/differential/src/cases.rs)) is a
   `&[ParityCase]` catalog. Each case is a triple:

   ```rust
   ParityCase {
       name: "alice_friends_via_knows",
       cypher:  "MATCH (p:Person {name:'Alice'})-[:KNOWS]->(f) RETURN f.name AS name",
       gql:     "MATCH (p:Person {name:'Alice'})-[:KNOWS]->(f) RETURN f.name AS name",
       gremlin: "g.V().has('Person','name','Alice').out('KNOWS').values('name')",
       mode:    DivergenceMode::Strict,
   }
   ```

3. **Runner** ([`runner.rs`](../tests/differential/src/runner.rs))
   executes each query through its engine and *canonicalizes* the result
   into a sorted `Vec<Vec<Value>>`. Order is normalized; column names
   are not (Gremlin doesn't always emit named columns), so positional
   value ordering is what's compared.

4. **Test entry point** ([`tests/parity.rs`](../tests/differential/tests/parity.rs))
   iterates the catalog, prints a per-case verdict, and panics with a
   detailed three-engine diff if any **strict** case diverges. Cases
   marked `DivergenceMode::Documented` are reported but never fail the
   run.

## How to add a new parity test

1. Pick an aspect of query semantics you want to lock in (a new
   aggregation, a temporal predicate, a path operator, etc.).

2. Write the same logical query in all three languages. **Important**:
   the queries must produce columns in the same logical order — the
   runner compares positionally, not by name. If you can't get the
   shapes to match (e.g. Gremlin `.project()` always returns a Map),
   either restructure the query or mark the case
   `DivergenceMode::Documented`.

3. Append to the `CASES` slice in
   [`tests/differential/src/cases.rs`](../tests/differential/src/cases.rs).

4. Run `bash scripts/run_differential_tests.sh` locally. If your case
   passes, you're done — CI will run it on every PR.

5. If your case fails:
   - **Engines actually diverge on a query you thought was equivalent.**
     Either you found a bug (file a GitHub issue, see "Found a real
     divergence?" below) or your queries weren't actually equivalent —
     refine the queries.
   - **Engines all agree but on a value the runner can't compare**
     (e.g. a Gremlin Map vs a Cypher row). Mark the case
     `DivergenceMode::Documented` with a one-paragraph rationale.

## Documented divergences (current catalog)

These are *not bugs*. They are accepted, well-understood semantic
differences between the three query languages and their engines. The
runner records them but does not fail the run.

| Case name                                          | Difference                                                                                                                                                              |
|----------------------------------------------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `documented_divergence_optional_property_null`     | Cypher/GQL surface missing properties as `NULL`; Gremlin `.values()` silently drops the traverser. Even `.by(values(...).fold())` produces an empty list, not `NULL`.   |
| `documented_divergence_float_avg_precision`        | Floating-point aggregation order is implementation-defined; engines may differ by ULPs. The runner currently treats them as equal when the value coincides exactly.     |
| `documented_divergence_count_distinct_label`       | `MATCH (n)` vs `g.V()` cover the same set in our implementation, but row-shape bookkeeping inside aggregations can differ in subtle ways.                                |
| `documented_divergence_list_property_return`       | Gremlin `.values()` on a list-typed property *unfolds* into multiple traversers; Cypher/GQL return the list as a single value. Use `fold()` to bridge — but shapes differ. |
| `documented_divergence_temporal_date_return`       | TinkerPop Gremlin has no native Date type; `.values()` returns a raw representation while Cypher/GQL return `Value::Date`. Cross-engine equality on temporals is not yet supported. |
| `documented_divergence_project_returns_map`        | Cypher/GQL multi-column `RETURN` produces N positional columns; Gremlin `.project()` produces a single column whose value is a Map. The values are equivalent but the row shape differs by design. |

When the runner observes that a documented case has *unexpectedly
converged* (the engines now agree), it prints a hint to consider
promoting the case to `DivergenceMode::Strict`. That's a good signal
that an underlying engine has improved and parity should be enforced
going forward.

## Found a real divergence?

If `cargo test -p differential` fails on a strict case after one of
your changes, that is a **bug**. The expected workflow:

1. Reproduce locally and capture the three-engine diff (the runner
   prints it).
2. File a GitHub issue describing the divergence (which case, which
   engine, the diff). Tag the engine team that owns the regressing
   crate.
3. Either:
   - **(a)** Fix the regressing engine in the same PR if the fix is
     small and clearly localized to one engine.
   - **(b)** Mark the failing case `#[ignore]` (or leave it failing
     behind a `should_panic`) with a TODO referencing the issue, and
     fix it in a follow-up PR.

Never relax a strict case to `Documented` just to make the test pass —
that defeats the whole purpose of the gate. Documented cases are for
known *language-level* differences, not for hiding engine bugs.
