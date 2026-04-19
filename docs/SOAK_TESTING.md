# Soak / memory-leak detection harness

The `soak` test crate is a long-running stability gate for the three
SedanDelivery embedded engines:

| Crate                     | Engine             |
| ------------------------- | ------------------ |
| `sedan-cypher-embedded`   | openCypher         |
| `sedan-gql-embedded`      | GQL (ISO/IEC 39075)|
| `sedan-gremlin-embedded`  | Apache TinkerPop Gremlin |

Each scenario drives an embedded engine for tens-of-thousands to millions
of operations and asserts that resource usage and latency stay inside an
explicit budget. Soak failures surface bugs that unit and TCK tests miss:
slow leaks, unbounded growth in caches/arenas, latency drift, deadlocks,
and resource cleanup regressions when budgets fire mid-execution.

## Why we need this gate

Unit tests, the openCypher TCK, fuzz smoke (PR #1675), and Miri (PR #1687)
all exercise *correctness*. None of them exercise *steady-state behaviour
under load*. The most painful production regressions in graph databases
are typically:

* Plan caches that grow without bound under high-cardinality query streams.
* Iterator state that is forgotten on early termination (timeouts, row caps).
* Locks held across error paths, surfacing as latency tail blow-outs.
* Allocation patterns that fragment the heap during long-running churn.

The soak harness is the cheapest place to catch all four — before they hit
a production deployment.

## Running soak tests locally

```bash
# Quick smoke (10% of spec iterations — finishes in a couple of minutes):
SOAK_ITER_SCALE=0.1 bash scripts/run_soak_tests.sh

# Full spec iteration counts (matches CI; ~20-30 minutes on a laptop):
bash scripts/run_soak_tests.sh

# Extended overnight soak (10x spec):
SOAK_ITER_SCALE=10 bash scripts/run_soak_tests.sh
```

Run a single scenario directly:

```bash
cargo test -p soak --release --test scenario1_query_soak \
  -- --ignored --nocapture
```

Every scenario is marked `#[ignore]` so it never runs in the default
`cargo test` invocation; opt-in via `--ignored` (or via the script,
which sets the appropriate flags).

## Scenarios and budgets

| # | Name                     | What it does                                                                  | Leak / latency budget                          |
|---|--------------------------|-------------------------------------------------------------------------------|------------------------------------------------|
| 1 | Steady-state query soak  | 10K-node graph; 100K iterations of a 4-query read mix                         | RSS at end < **1.5x** RSS at 10% in            |
| 2 | Insert/delete churn      | 50K iterations of `CREATE` / `DELETE` matched pairs                           | Final node count == initial; RSS < **1.5x**    |
| 3 | Long-running session     | 1M queries through a single `CypherEngine`                                    | p99 latency at end < **3x** p99 at start; 0 errors |
| 4 | Concurrent stress        | 8 OS threads × 50K reads against a shared `InMemoryGraphStore`                | No deadlocks (5-min watchdog); no graph drift  |
| 5 | Budget enforcement       | 10K iterations of a heavy Gremlin scan under `ExecutionBudget(10ms, 100 rows)` | Every iter must Err or cap rows; RSS < **1.5x** |

Each scenario emits a one-line summary like:

```
[soak/scenario1] iter=100000 rss_baseline_kb=39492 rss_final_kb=41200 rss_growth=4.3% ratio=1.04x budget=1.5x pass
```

The script (`scripts/run_soak_tests.sh`) extracts these lines into a
final summary block.

## Interpreting RSS samples

RSS (resident-set size) is sampled by reading `/proc/self/statm`
(Linux only; falls back to `0` and skips the budget check on other
platforms). Soak budgets are intentionally expressed as **ratios**, not
absolute byte counts, because:

* Different runners have different baselines (CI vs. laptop vs. dev VM).
* The Rust allocator's first-touch behaviour inflates absolute numbers.
* Ratios are stable across machine generations.

The "baseline" sample is taken **after** warm-up — typically 10% into the
run — so first-touch allocations don't poison the measurement. The
"final" sample is taken on the last sampling tick.

A scenario passes when `RSS_final / RSS_baseline < budget`. The default
budget is **1.5x** for memory and **3x** for p99 latency. These are
intentionally generous so the gate is robust against allocator noise but
catches genuine leaks (which typically grow 5-100x).

If you see a failure, look at the per-tick samples printed during the
run — a clean leak shows monotonic growth; a one-shot allocation shows a
single jump and then steady-state.

## Adding a new soak scenario

1. Add a new `tests/scenarioN_*.rs` file under `tests/soak/tests/`. Use
   the existing scenarios as templates.
2. Register it in `tests/soak/Cargo.toml` as a `[[test]]` entry.
3. Mark the test `#[ignore = "soak — opt-in via cargo test -p soak -- --ignored"]`.
4. Arm a `TimeoutGuard` to bound the worst case — the watchdog calls
   `std::process::exit(2)` if the deadline elapses, surfacing deadlocks
   that would otherwise hang CI.
5. Use `soak::scaled_iters(BASE)` for any iteration count so the
   `SOAK_ITER_SCALE` env var works uniformly.
6. Print a `[soak/scenarioN] ...` summary line at the end so the
   script's grep picks it up.
7. If the scenario depends on RSS, treat `rss_kb() == 0` as
   "unavailable" and skip the budget check (Linux-only).

## CI integration

The `.github/workflows/soak.yml` workflow is **opt-in and observability
only**:

* Triggered on `workflow_dispatch`, nightly cron (06:00 UTC), and pushes
  to `main` whose commit message contains `[soak]`.
* Uses `Swatinem/rust-cache` pinned by SHA.
* Uploads the full `soak-output.log` as an artifact.
* Marked `continue-on-error: true` — failures **never** block other
  required checks. Promote to required only after two consecutive green
  weeks.

## When a soak test finds a real bug

Treat it the same way you'd treat a fuzz finding:

1. File a GitHub issue with the failing scenario, the printed budget
   line, and the per-tick RSS samples.
2. Either fix the bug in the same PR (preferred for small, obvious
   leaks) or mark the scenario `#[ignore = "fails-bug-#XXXX"]` with the
   issue number so the gate stays green and the regression is tracked.
3. Never silently weaken a budget to make a failing scenario pass —
   that defeats the purpose of the gate.
