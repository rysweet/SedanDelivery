# Performance Benchmarking

SedanDelivery ships a high-signal [criterion](https://bheisler.github.io/criterion.rs/book/)
benchmark suite for each embedded engine crate so users — and CI — can track
top-line query performance over time.

## Crates with embedded benches

| Crate                     | Bench file                                    |
| ------------------------- | --------------------------------------------- |
| `sedan-cypher-embedded`   | `benches/embedded_bench.rs` + `benches/engine.rs` |
| `sedan-gql-embedded`      | `benches/embedded_bench.rs` + `benches/engine.rs` |
| `sedan-gremlin-embedded`  | `benches/embedded_bench.rs` + `benches/engine.rs` |

`embedded_bench.rs` is the **headline suite**: a tight set of four scenarios
designed to be representative, low-noise, and fast enough to run in CI
(~30 s per crate on a modern laptop).

## Running

```bash
# One crate at a time (recommended — each suite runs to convergence)
cargo bench -p sedan-cypher-embedded --bench embedded_bench
cargo bench -p sedan-gql-embedded     --bench embedded_bench
cargo bench -p sedan-gremlin-embedded --bench embedded_bench

# Just compile (sanity check, no measurement)
cargo bench -p sedan-cypher-embedded --no-run

# Smoke run with reduced sampling (CI-friendly)
cargo bench -p sedan-cypher-embedded -- --quick --warm-up-time 1 --measurement-time 1
```

Criterion writes HTML reports to `target/criterion/<group>/<bench>/report/index.html`
when the `html_reports` feature is enabled (it is, by default in this workspace).

## What each bench measures

### `sedan-cypher-embedded::embedded_bench`

| Bench                              | Measures                                                |
| ---------------------------------- | ------------------------------------------------------- |
| `cypher/param_roundtrip`           | Parser/planner overhead floor for `RETURN $x`           |
| `cypher/count_all_100_nodes`       | Full-scan + aggregate over 100 nodes                    |
| `cypher/traversal_knows_limit_50`  | Label + rel-type filtered traversal with `LIMIT 50`     |
| `cypher/plan_cache/{hit,miss}`     | Plan-cache effectiveness: same query, fresh vs. reused engine |

### `sedan-gql-embedded::embedded_bench`

GQL (ISO/IEC 39075) parity benches mirroring the Cypher set so regressions
and parity gaps are detectable side-by-side.

| Bench                            | Measures                                          |
| -------------------------------- | ------------------------------------------------- |
| `gql/param_roundtrip`            | Parameter round-trip via `execute_with_context`   |
| `gql/count_all_100_nodes`        | Full-scan + aggregate                             |
| `gql/traversal_knows_limit_50`   | Filtered traversal with `LIMIT`                   |
| `gql/repeat_same_query`          | Steady-state cost of repeated execution           |

### `sedan-gremlin-embedded::embedded_bench`

| Bench                                      | Measures                                       |
| ------------------------------------------ | ---------------------------------------------- |
| `gremlin/v_count`                          | `g.V().count()` over 100 nodes                 |
| `gremlin/v_out_limit_50`                   | `g.V().out().limit(50)` early-termination path |
| `gremlin/compile_reuse/compile_each_call`  | Full parse+compile+execute every call          |
| `gremlin/compile_reuse/compile_once`       | Compile once, execute many — amortised cost    |

## Interpreting criterion output

Each bench prints a line like:

```
cypher/count_all_100_nodes
                        time:   [12.345 µs 12.456 µs 12.578 µs]
                        change: [-2.3% -1.1% +0.4%] (p = 0.07 > 0.05)
                        No change in performance detected.
```

* **time** — `[lower_bound estimate upper_bound]` 95 % confidence interval.
  The middle number is the best point estimate.
* **change** — delta vs. the previous run (or baseline). Criterion only
  declares a change when `p < 0.05`.
* **outliers** — criterion will note the count and severity; investigate
  if "high severe" outliers exceed ~5 %.

## Comparing runs (baselines)

```bash
# Save a named baseline before your change
cargo bench -p sedan-cypher-embedded --bench embedded_bench -- --save-baseline before

# Make your change, then compare
cargo bench -p sedan-cypher-embedded --bench embedded_bench -- --baseline before
```

For CI regression detection, save a baseline on `main` and compare PR runs
against it. Treat `>5 %` regressions on the headline benches as actionable.

## CI perf-regression gate

Two CI jobs guard against silent perf regressions:

| Workflow | Job | When | Format | Scope |
| -------- | --- | ---- | ------ | ----- |
| `.github/workflows/perf-regression.yml` | `perf-regression` | every PR | criterion `estimates.json` | curated workspace benches (no Redis/Bolt services) |
| `.github/workflows/perf-regression.yml` | `save-baseline`   | push to `main` | criterion `--save-baseline main` | same curated set, archived as artifact (90 d) |
| `.github/workflows/ci.yml`              | `benchmarks`      | `perf`-labeled PRs + `main` pushes | bencher text format | service-bound benches (`sedandelivery-bolt`, `sd2-redis-store`) |

### What the gate runs

`scripts/perf_regression_check.sh` is the single entry point. It:

1. Checks out `BASELINE_REF` (default: `origin/main`) and runs each curated
   bench with `cargo bench --message-format=json -- --save-baseline perfgate-baseline`.
2. Checks out `HEAD_REF` (default: `HEAD`) and re-runs them with
   `--save-baseline perfgate-head`.
3. Walks `target/criterion/<group>/<bench>/<label>/estimates.json` and compares
   the `mean.point_estimate` of each bench between the two labels.
4. Prints a markdown-friendly table (bench | baseline ms | head ms | delta % |
   status) and exits non-zero if any benchmark regressed by more than
   `PERF_REGRESSION_THRESHOLD` percent (default **10**).

### Default bench set

| Crate                     | Bench                |
| ------------------------- | -------------------- |
| `cypher-core`             | `parser`             |
| `cypher-core`             | `executor`           |
| `cypher-core`             | `property_index`     |
| `gremlin-core`            | `parse_compile`      |
| `gql-engine`              | `gql_parse`          |
| `sedan-cypher-embedded`   | `embedded_bench`     |
| `sedan-gql-embedded`      | `embedded_bench`     |
| `sedan-gremlin-embedded`  | `embedded_bench`     |

`cypher-core::scale` and the bolt / redis benches are intentionally excluded
from the always-on gate (too slow for a per-PR run, or require external
services). They remain in the existing `benchmarks` job in `ci.yml`, which
is opt-in via the `perf` PR label.

### Running the gate locally

```bash
# Compare HEAD against origin/main with default 10 % threshold
bash scripts/perf_regression_check.sh

# Stricter threshold
PERF_REGRESSION_THRESHOLD=5 bash scripts/perf_regression_check.sh

# Compare against a specific ref
BASELINE_REF=origin/release-1.4 bash scripts/perf_regression_check.sh

# Smoke test — compares HEAD against itself (expect ~0 % delta)
DRY_RUN=1 bash scripts/perf_regression_check.sh

# Override the bench set
BENCH_SPECS="cypher-core::executor sedan-cypher-embedded::embedded_bench" \
  bash scripts/perf_regression_check.sh
```

The script requires a clean working tree (it uses `git checkout --detach` to
swap between baseline and head); commit or stash local changes first.

### Limitations

* **GitHub-hosted ubuntu-latest runners** have noisy, shared CPUs. The default
  threshold is set at **10 %** to avoid false positives — tighten via the
  `PERF_REGRESSION_THRESHOLD` repo variable once a self-hosted dedicated
  runner is available.
* The CI job runs benches with `--warm-up-time 1 --measurement-time 2` to
  fit in a 60-minute timeout. Local runs use criterion defaults, so local
  measurements are more accurate than CI ones.
* Benches that fail to compile or run on the baseline ref are skipped with a
  warning rather than failing the gate (so the gate stays green when a new
  bench is added in the PR itself).
* `cargo bench --message-format=json` emits *cargo's* compile messages as
  JSON, not criterion's measurement data. Measurement numbers are read from
  the `target/criterion/**/estimates.json` files criterion writes natively.

## Notes

* Graphs are seeded **once** outside the measured closure; the hot path only
  exercises the engine.
* `criterion::black_box` is used on inputs and results so the optimiser can't
  hoist work out of the loop.
* The 100-node / ~200-edge sizing keeps one full sweep under 30 s while
  remaining large enough to surface real planner/executor cost differences.
