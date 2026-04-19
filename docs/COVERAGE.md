# Code Coverage

Baseline line-coverage for the core/embedded Rust crates, measured with
[`cargo-llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov) using each
crate's `--lib` unit tests.

## Baseline (as of 2026-04-18)

| Crate                     | Lines covered | Target |
| ------------------------- | ------------: | -----: |
| `cypher-core`             |     **61.14%** |   70% |
| `gql-engine`              |     **78.35%** |   70% |
| `gremlin-core`            |     **42.35%** |   70% |
| `sedan-cypher-embedded`   |     **65.48%** |   70% |
| `sedan-gql-embedded`      |     **72.92%** |   70% |
| `sedan-gremlin-embedded`  |     **96.43%** |   70% |

Percentages come from `cargo llvm-cov --lib --json` (`data[0].totals.lines.percent`).
Crates already above the 70% target: `gql-engine`, `sedan-gql-embedded`,
`sedan-gremlin-embedded`. Crates below target (gap work tracked separately):
`cypher-core`, `gremlin-core`, `sedan-cypher-embedded`.

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
