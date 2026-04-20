# embedded-bench

Reliability micro-benchmark suite for the SedanDelivery embedded query
engines. Locks in steady-state performance characteristics on the hot path
so future PRs can detect regressions via the existing `perf-regression`
workflow.

## What it measures

Three bench files — one per embedded engine — each running the same five
scenarios against the shared 1000-node `:Person` fixture defined in
`src/lib.rs`:

| Bench | Workload |
| ----- | -------- |
| `simple_match`      | `MATCH (n:Person) WHERE n.age > 21 RETURN n LIMIT 10` |
| `traversal_2hop`    | `MATCH (a)-[*2]->(c) RETURN c LIMIT 100` |
| `aggregate`         | Count grouped by label |
| `mutation`          | Insert 100 nodes (fresh store per iteration) |
| `parameter_binding` | Same query × 100 with different parameter values |

Results are reported as `cypher/<bench>`, `gql/<bench>`, `gremlin/<bench>`
so the three engines can be diffed apples-to-apples.

### Engine notes

* **Cypher / GQL** — `parameter_binding` uses `execute_with_parameters` /
  `execute_with_context` with a deterministic 100-element schedule of
  `$age` values.
* **Gremlin** — has no `$param` API; `parameter_binding` measures the
  closest semantic equivalent (compile once, run the cached plan 100×).
  `mutation` uses direct `GraphStore` writes because the embedded Gremlin
  surface is read-only (mutations go through `cypher-core`'s storage layer).

All seed data is generated from a fixed integer schedule (no RNG); results
are reproducible across runs.

## Running

Build only (used by CI's pre-merge `--no-run` gate):

```bash
cargo bench -p embedded-bench --no-run
```

Quick smoke run (low sample size, ~6s per bench):

```bash
cargo bench -p embedded-bench -- --sample-size 10 --warm-up-time 1 --measurement-time 2
```

A single bench, full sampling:

```bash
cargo bench -p embedded-bench --bench cypher_bench -- simple_match
```

Save a baseline (e.g. before a refactor) and compare a candidate run:

```bash
cargo bench -p embedded-bench -- --save-baseline pre
# … apply changes …
cargo bench -p embedded-bench -- --baseline pre
```

## Perf-regression integration

This crate's three benches (`cypher_bench`, `gql_bench`, `gremlin_bench`)
are registered as additional bench targets in
`scripts/perf_regression_check.sh`, so they run automatically on every PR
and fail the gate if any bench regresses by more than
`PERF_REGRESSION_THRESHOLD %` (default 10%).
