# Code Coverage

Baseline line-coverage for the core/embedded Rust crates, measured with
[`cargo-llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov) using each
crate's `--lib` unit tests.

## Baseline (2026-04-19)

| Crate                     |   Line% | Function% | Files tested | Target |
| ------------------------- | ------: | --------: | -----------: | -----: |
| `cypher-core`             | **61.14%** |    66.31% |           79 |    70% |
| `gql-engine`              | **78.35%** |    85.71% |            5 |    70% |
| `gremlin-core`            | **44.05%** |    69.68% |           25 |    70% |
| `sedan-cypher-embedded`   | **65.48%** |    60.00% |            1 |    70% |
| `sedan-gql-embedded`      | **72.92%** |    71.43% |            1 |    70% |
| `sedan-gremlin-embedded`  | **96.43%** |    88.24% |            1 |    70% |

Percentages come from `cargo llvm-cov --lib --json`
(`data[0].totals.{lines,functions}.percent`); files-tested is `len(data[0].files)`.
Crates already above the 70% line-coverage target: `gql-engine`,
`sedan-gql-embedded`, `sedan-gremlin-embedded`. Crates below target
(gap work tracked separately): `cypher-core`, `gremlin-core`,
`sedan-cypher-embedded`.

### Top gap modules (highest-LOC files pulling line% down)

The three crates furthest below the 70% line-coverage floor and the
single highest-LOC, lowest-coverage module dragging each one down:

| Crate                   | Module (file)                                       |   LOC | Line%  |
| ----------------------- | --------------------------------------------------- | ----: | -----: |
| `gremlin-core`          | `src/compiler/helpers.rs`                           | 1,414 | 21.57% |
| `cypher-core`           | `src/executor/engine/gremlin.rs`                    | 3,821 | 38.16% |
| `sedan-cypher-embedded` | `src/lib.rs`                                        |    84 | 65.48% |

Other notable low-coverage hot spots (informational, not separately tracked):

- `gremlin-core/src/text_parser.rs` (2,616 LOC, 42.55%)
- `gremlin-core/src/compiler/steps/strings.rs` (464 LOC, 3.45%)
- `gremlin-core/src/compiler/steps/by.rs` (629 LOC, 14.47%)
- `cypher-core/src/executor/filter/mod.rs` (2,563 LOC, 35.93%)
- `cypher-core/src/executor/engine/eval.rs` (1,947 LOC, 41.40%)
- `cypher-core/src/storage/cached.rs` (953 LOC, 41.87%)

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

Not yet wired into CI. This baseline exists so that a future change can
enforce the 70% floor without a flag-day regression.
