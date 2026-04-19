# Code Coverage

Baseline line-coverage for the core/embedded Rust crates, measured with
[`cargo-llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov) using each
crate's `--lib` unit tests.

## Baseline (2026-04-19, refreshed post-#1357)

Refreshed against `main` after the recent test-coverage waves landed:
PR #1336 established the initial baseline; #1345 refreshed it after the
`cypher-core/src/executor/filter/mod.rs` decomposition; #1353 added a
large `gremlin-core::text_parser` test suite; #1355 added compiler-step
tests (`strings.rs`, `by.rs`, `filter.rs`, etc.); and #1357 decomposed
`cypher-core/src/executor/engine/eval.rs` into the `eval/` submodule and
added accompanying eval tests.

| Crate                     |   Line% | Function% | Files tested | Target |
| ------------------------- | ------: | --------: | -----------: | -----: |
| `cypher-core`             | **58.73%** |    62.54% |          106 |    70% |
| `gql-engine`              | **78.35%** |    85.71% |            5 |    70% |
| `gremlin-core`            | **58.97%** |    75.49% |           33 |    70% |
| `sedan-cypher-embedded`   | **65.48%** |    60.00% |            1 |    70% |
| `sedan-gql-embedded`      | **72.92%** |    71.43% |            1 |    70% |
| `sedan-gremlin-embedded`  | **96.43%** |    88.24% |            1 |    70% |

Percentages come from `cargo llvm-cov --lib --json`
(`data[0].totals.{lines,functions}.percent`); files-tested is `len(data[0].files)`.
Crates already above the 70% line-coverage target: `gql-engine`,
`sedan-gql-embedded`, `sedan-gremlin-embedded`. Crates still below target
(gap work tracked separately): `cypher-core`, `gremlin-core`,
`sedan-cypher-embedded`. The most dramatic delta from #1336/#1345 is
`gremlin-core` (44.05% → 58.97%, +14.92pt) driven by #1353 + #1355;
`cypher-core` ticked down very slightly (59.13% → 58.73%) because the
#1357 `eval.rs` decomposition exposed previously-aggregated low-coverage
gremlin-funcs files (see hot-spot table) without changing test behaviour.

### Top gap modules (highest-LOC files pulling line% down)

The three crates still below the 70% line-coverage floor and the
single highest-LOC, lowest-coverage module dragging each one down:

| Crate                   | Module (file)                                                |   LOC | Line%  |
| ----------------------- | ------------------------------------------------------------ | ----: | -----: |
| `gremlin-core`          | `src/compiler/helpers.rs`                                    | 1,414 | 44.55% |
| `cypher-core`           | `src/storage/cached.rs`                                      |   953 | 41.87% |
| `sedan-cypher-embedded` | `src/lib.rs`                                                 |    84 | 65.48% |

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
