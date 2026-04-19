# Code Coverage

Baseline line-coverage for the core/embedded Rust crates, measured with
[`cargo-llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov) using each
crate's `--lib` unit tests.

## Baseline (refreshed post Gremlin-bridge integration push past 80%)

Refreshed against `main` after adding `cypher-core/tests/gremlin_integration.rs`,
a dev-dep integration suite that compiles Gremlin traversals via
`gremlin-core` and executes them through `cypher-core`'s `QueryExecutor` in
Gremlin mode. The suite (~50 tests covering addV/addE, project/by, group,
repeat/until/emit, choose/coalesce, sample, where(traversal), order().by(),
select, unfold/fold, aggregate/sideEffect, and more) exercises the
previously-unreachable Gremlin-only executor modules under
`cypher-core/src/executor/engine/gremlin/*`. For `cypher-core` the floor
script now measures `--lib --tests` (instead of `--lib` alone) to include
integration coverage; other crates remain on `--lib`.

cypher-core moved from **78.12% → 82.50%** (+4.38pt, `--lib --tests`).

| Crate                     |   Line% | Function% | Files tested | Target |
| ------------------------- | ------: | --------: | -----------: | -----: |
| `cypher-core`             | **82.50%** |        ~  |          ~110 |    70% |
| `gql-engine`              | **99.36%** |   100.00% |            5 |    70% |
| `gremlin-core`            | **79.20%** |    88.36% |           40 |    70% |
| `sedan-cypher-embedded`   | **100.00%** |   100.00% |            1 |    70% |
| `sedan-gql-embedded`      | **100.00%** |   100.00% |            1 |    70% |
| `sedan-gremlin-embedded`  | **99.47%** |    95.65% |            1 |    70% |

Percentages come from `cargo llvm-cov --json`
(`data[0].totals.{lines,functions}.percent`); files-tested is `len(data[0].files)`.
For `cypher-core` the command is `cargo llvm-cov -p cypher-core --lib --tests --json`;
for all other crates it remains `cargo llvm-cov -p <crate> --lib --json`.
Crates already above the 70% line-coverage target: `cypher-core`,
`gql-engine`, `sedan-gql-embedded`, `sedan-gremlin-embedded`,
`sedan-cypher-embedded`. Crates still below target: `gremlin-core` (gap
work tracked separately).

## Baseline (refreshed post cypher-core push to 78%)

Refreshed against `main` after the cypher-core coverage push past 78%:
the current PR added five `coverage_targeted_tests{5,6,7,8,9}.rs` modules
(~390 new tests) targeting the lowest-covered cypher-core modules
exercising the parse → plan → execute pipeline plus direct `Operator`
construction. cypher-core moved from **75.34% → 78.12%** (+2.78pt).
A residual gap remains in Gremlin-only executor modules
(`executor/engine/gremlin/*`, `executor/engine/eval/gremlin_*_funcs.rs`,
~3300 lines at 0% from cypher tests) which require Gremlin-side
integration tests to exercise.

| Crate                     |   Line% | Function% | Files tested | Target |
| ------------------------- | ------: | --------: | -----------: | -----: |
| `cypher-core`             | **78.12%** |    78.12% |          ~110 |    70% |
| `gql-engine`              | **99.36%** |   100.00% |            5 |    70% |
| `gremlin-core`            | **79.20%** |    88.36% |           40 |    70% |
| `sedan-cypher-embedded`   | **100.00%** |   100.00% |            1 |    70% |
| `sedan-gql-embedded`      | **100.00%** |   100.00% |            1 |    70% |
| `sedan-gremlin-embedded`  | **99.47%** |    95.65% |            1 |    70% |

Percentages come from `cargo llvm-cov --lib --json`
(`data[0].totals.{lines,functions}.percent`); files-tested is `len(data[0].files)`.
Crates already above the 70% line-coverage target: `gql-engine`,
`sedan-gql-embedded`, `sedan-gremlin-embedded`, `sedan-cypher-embedded`.
Crates still below target (gap work tracked separately): `cypher-core`,
`gremlin-core`. The most dramatic delta from #1336/#1345 is
`gremlin-core` (44.05% → 58.97% → 56.65%, +12.60pt net) driven by #1353 + #1355,
with a small artifactual dip from the helpers/ submodule split (file-boundary
instrumentation, no behavior change);
`cypher-core` recovered substantially in this PR (58.73% → 69.70%, +10.97pt)
by adding inline `#[cfg(test)]` tests to the lowest-coverage modules exposed
by the #1357 / #1376 / #1378 decompositions: `storage/cached.rs`,
`changelog.rs`, `executor/filter/builtins.rs`, `executor/filter/operators.rs`,
`executor/filter/geo.rs`, `executor/filter/temporal/properties.rs`, and the
entire `executor/filter/temporal/{constructors,truncation,arithmetic}` tree.
All additions are pure additive tests; no production code changed.

### Top gap modules (highest-LOC files pulling line% down)

The two crates still below the 70% line-coverage floor and the
single highest-LOC, lowest-coverage module dragging each one down:

| Crate                   | Module (file)                                                |   LOC | Line%  |
| ----------------------- | ------------------------------------------------------------ | ----: | -----: |
| `gremlin-core`          | `src/compiler/helpers/predicate_label.rs`                    |   189 | 19.58% |
| `cypher-core`           | `src/storage/cached.rs`                                      |   953 | 41.87% |

Other notable low-coverage hot spots (informational, not separately tracked):

- `gremlin-core/src/text_parser/steps.rs` (1,111 LOC, 48.87%) — refactored from monolithic `text_parser.rs` by #1353
- `gremlin-core/src/compiler/steps/by.rs` (629 LOC, 36.41%) — improved from 14.47% by #1355
- `gremlin-core/src/compiler/steps/strings.rs` (464 LOC, 57.76%) — improved from 3.45% by #1355
- `gremlin-core/src/compiler/steps/filter.rs` (506 LOC, 63.83%) — improved from 31.82% by #1355
- `gremlin-core/src/compiler/steps/branch.rs` (501 LOC, 52.50%)
- `gremlin-core/src/compiler/compile_from.rs` (379 LOC, 52.24%)
- `cypher-core/src/executor/filter/builtins.rs` (891 LOC, 54.43%)
- `cypher-core/src/storage/persistent.rs` (885 LOC, 59.44%)
- `cypher-core/src/parser/parser/expression.rs` (717 LOC, 55.09%)
- `cypher-core/src/planner/planner/result_planner.rs` (651 LOC, 39.17%)
- `cypher-core/src/executor/engine/eval/gremlin_value_funcs.rs` (350 LOC, 0.00%) — exposed by #1357 decomposition
- `cypher-core/src/executor/engine/eval/gremlin_misc_funcs.rs` (197 LOC, 0.00%) — exposed by #1357 decomposition

## Running coverage locally

One-liner:

```bash
./scripts/coverage.sh
```

This will:

1. Install `cargo-llvm-cov` if missing (and the `llvm-tools-preview` component).
2. Run `--lib` coverage for the six target crates.
3. Emit `target/llvm-cov-reports/<crate>.json` (machine-readable summary).
4. Emit `target/llvm-cov/html/<crate>/html/index.html` (browsable report).
5. Print a per-crate `LINES%` table to stdout.

To run manually for a single crate:

```bash
cargo llvm-cov -p cypher-core --lib --summary-only
cargo llvm-cov -p cypher-core --lib --html --output-dir target/llvm-cov/html/cypher-core
```

## Target: 70% minimum per crate

The 70% per-crate line-coverage target balances three concerns:

- **Signal vs. noise.** Below ~60%, coverage gaps are dominated by
  easy-to-cover paths (error formatting, trivial getters) and do not yet
  reflect real gaps in query semantics. Above ~80%, marginal gains tend to
  come from defensive branches that are expensive to exercise and offer
  diminishing bug-catching value.
- **Achievability from the current baseline.** Four of the six crates are
  already within ~10 points of 70% on `--lib` alone; the remaining two
  (`gremlin-core` text parser, `cypher-core` display code) have
  well-understood, enumerable gaps.
- **Alignment with embedded-surface guarantees.** The embedded crates are
  public API surfaces with parity contracts across cypher/GQL/gremlin; a
  uniform floor makes regressions in any single engine immediately visible
  rather than masked by a workspace-wide average.

Reaching 70% on the three under-target crates is explicitly **follow-up
work** and is out of scope for this baseline-tooling change.

## CI integration

CI runs `scripts/coverage_floor_check.sh` in the `coverage` job to guard
against regressions below the **currently-measured** baseline (not the
70% target — see below). The script:

1. Runs `cargo llvm-cov -p <crate> --lib --json` for each tracked crate.
2. Parses `data[0].totals.lines.percent` from each report.
3. Compares against a hardcoded floor table (the values above, with
   `cypher-core` rounded down to the nearest 0.5pt).
4. Allows a 1.0pt tolerance for measurement noise.
5. Exits non-zero (and reports which crates regressed and by how much)
   if any crate drops below `floor − tolerance`.

The floor enforced today is the **current measured value**, not 70%.
This avoids a flag-day regression while still preventing any future
change from silently moving coverage backwards. A separate ratchet
PR is expected to raise individual floors as crates reach 70%.

## Branch coverage

Line coverage answers "did this line execute at least once?" but is silent on
whether every conditional branch was taken. A line such as
`if a && b.is_ok() { ... } else { ... }` shows as covered as soon as it runs
once, even if the `else` arm or the short-circuit-on-`false` case never
fires. Branch coverage closes that blind spot by counting each LLVM-emitted
branch edge separately.

### Toolchain

`cargo-llvm-cov`'s `--branch` flag is **unstable** and requires:

- The **nightly** Rust toolchain (`rustup toolchain install nightly`).
- The nightly `llvm-tools-preview` component
  (`rustup component add llvm-tools-preview --toolchain nightly`).
- `cargo-llvm-cov` itself (same binary as line coverage; reused).

Invocation:

```bash
rustup run nightly cargo llvm-cov -p <crate> --lib --branch \
  --json --output-path target/llvm-cov-branch/<crate>.json
```

For `cypher-core` use `--lib --tests` (matches `coverage_floor_check.sh`)
to include the Gremlin-bridge integration suite.

The `branches.percent` / `branches.count` / `branches.covered` fields appear
in the JSON `data[0].totals` block alongside the existing `lines` block.

### Branch-coverage baseline

Measured against `main` at the introduction of
`scripts/branch_coverage_floor_check.sh`. These are the **current truth**,
not aspirational targets — they exist to prevent silent regression.

| Crate                     |  Branch% |  Covered/Total | Floor |
| ------------------------- | -------: | -------------: | ----: |
| `cypher-core`             |  60.98%  |   3,405 / 5,584 |  60.0 |
| `gremlin-core`            |  57.83%  |   1,300 / 2,248 |  57.0 |
| `gql-engine`              |  81.67%  |       147 / 180 |  81.0 |
| `sedan-cypher-embedded`   | 100.00%  |          2 / 2  | 100.0 |
| `sedan-gql-embedded`      |    n/a   |          0 / 0  |   0.0 |
| `sedan-gremlin-embedded`  |    n/a   |          0 / 0  |   0.0 |

The two embedded shells (`sedan-gql-embedded`, `sedan-gremlin-embedded`) are
thin re-export wrappers whose `--lib` test suites currently exercise no
branching code paths — LLVM emits zero branch regions. The floor script
treats `count == 0` as PASS but keeps the row so we notice if branches
later start being instrumented (at which point we should ratchet the floor
up to whatever is measured).

The line/branch gap on `cypher-core` (82% vs 61%) and `gremlin-core`
(80% vs 58%) is the expected magnitude: both crates contain large
match-on-AST executors and option/result short-circuit chains that line
coverage flatters. The gap is a useful map of where targeted tests will
move the needle most.

### Running branch coverage locally

```bash
# Single crate
rustup run nightly cargo llvm-cov -p cypher-core --lib --tests --branch \
  --summary-only

# All tracked crates + floor enforcement
bash scripts/branch_coverage_floor_check.sh
```

### CI integration

`scripts/branch_coverage_floor_check.sh` runs in the
`branch-coverage` job (`.github/workflows/branch-coverage.yml`) on every PR.
The script is structurally identical to `coverage_floor_check.sh` but uses
`--branch` on the nightly toolchain and parses `branches.percent` instead
of `lines.percent`. A 1.0pt tolerance for measurement noise mirrors the
line-coverage gate.
