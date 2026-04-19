# Mutation Testing for Embedded Engine Crates

This document describes the **mutation-testing baseline** for the embedded
engine crates (`sedan-cypher-embedded`, `sedan-gql-embedded`,
`sedan-gremlin-embedded`) and how to run, interpret, and extend it.

## What is mutation testing?

Line coverage tells you which lines were _executed_ during tests. It does
**not** tell you which lines were _verified_. A test that calls a function
and never inspects the result still racks up coverage even though it would
pass if the function returned garbage.

Mutation testing closes that gap. A mutation tool ([cargo-mutants][cm])
mechanically applies small "mutants" to the source — replacing a function
body with `Default::default()`, flipping a comparison, deleting a branch —
and re-runs the test suite for each mutant. The outcomes are:

| Outcome    | Meaning                                                   |
| ---------- | --------------------------------------------------------- |
| `caught`   | A test failed → mutant detected → 👍                      |
| `missed`   | All tests passed → **test gap** → 👎                      |
| `unviable` | Mutated code didn't compile (often type-system caught it) |
| `timeout`  | Tests hung — usually a hint at a real bug or infinite loop|

The headline reliability metric is the **mutation score**:

    mutation_score = caught / (caught + missed)

It is a far stronger signal than line coverage.

[cm]: https://mutants.rs/

## Installing cargo-mutants

```bash
cargo install cargo-mutants --version "^25" --locked
```

The script in [`scripts/mutation_test_embedded.sh`](../scripts/mutation_test_embedded.sh)
auto-installs it on first run if missing.

## Running the script

From the repo root:

```bash
# All three embedded crates
scripts/mutation_test_embedded.sh

# A single crate
scripts/mutation_test_embedded.sh sedan-cypher-embedded

# CI: shard 0 of 4 to parallelize across runners
SHARD="0/4" scripts/mutation_test_embedded.sh
```

Output directories are `mutants.<crate>.out/`. The interesting files are:

- `mutants.<crate>.out/mutants.out/missed.txt` — every test gap, one per line
- `mutants.<crate>.out/mutants.out/caught.txt` — mutants the suite killed
- `mutants.<crate>.out/mutants.out/unviable.txt` — type-system rejections
- `mutants.<crate>.out/mutants.out/diff/*.diff` — exact mutation source

## Interpreting missed mutants

Every line in `missed.txt` is a **test gap** — a place where the engine could
return wrong results and the test suite would not notice. Typical fixes:

1. **Replace expression with constant** missed → add an assertion that pins
   the actual computed value (not just `is_ok()`).
2. **Boundary off-by-one** missed (`<` ↔ `<=`) → add a boundary test at the
   exact edge.
3. **Branch deletion** missed (`if x { … }` → `{}`) → test both arms,
   including the side effect, not just the function's return value.
4. **Function body replaced with `()` or `Ok(default)`** missed → add a test
   that observes the function's _side effect_ (mutated state, cleared cache,
   sent message) rather than only its return value.

When a mutant is genuinely _impossible_ to kill from outside (e.g. the
mutation only affects a private optimization with no observable behavior
change), document it in the test or open a Cargo.toml `[mutants]` exclusion
with rationale.

## Baseline (PR `feat/mutation-testing-baseline`, against `origin/main`)

| Crate                     | Total | Viable | Caught | Missed | Unviable | Score      |
| ------------------------- | ----: | -----: | -----: | -----: | -------: | ---------- |
| `sedan-cypher-embedded`   |     4 |      2 |    2\* |      0 |        2 | 100%\*     |
| `sedan-gql-embedded`      |     2 |      0 |      0 |      0 |        2 | n/a        |
| `sedan-gremlin-embedded`  |     5 |      0 |      0 |      0 |        5 | n/a        |

\* After this PR. The pre-PR baseline was **0 / 2 caught**; the
gap-closing tests in
[`mutation_gap_01_cache_capacity.rs`](../sedan-cypher-embedded/tests/mutation_gap_01_cache_capacity.rs)
and
[`mutation_gap_02_clear_cache.rs`](../sedan-cypher-embedded/tests/mutation_gap_02_clear_cache.rs)
kill the two missed mutants by introducing observability via the new
`CypherEngine::cache_len()` accessor.

The high `unviable` count for the GQL and Gremlin embedded crates reflects
that their public API surface returns concrete `QueryResult` types that do
not implement `Default`, so the only mutation cargo-mutants currently
generates (`Ok(Default::default())`) is rejected by the type system. This
is a desirable property — the type system is doing test work for us — but
it also means the embedded crate suites do not yet carry a meaningful
mutation signal of their own. A follow-up should run cargo-mutants against
the underlying `gql-engine` / `gremlin-core` / `cypher-core` crates to get
end-to-end coverage; that work is sized for a dedicated PR per crate
because those crates have orders of magnitude more mutants and individual
runs take 30 min – several hours.

## Adding a gap-closing test

Convention: one file per closed gap.

    sedan-<crate>-embedded/tests/mutation_gap_<NN>_<short_name>.rs

Each test must:

1. Reference this doc and the original missed mutant in a top-of-file comment.
2. Fail with the mutant in place.
3. Pass with the original code.

You can verify (3) by reverting the mutant locally and running
`cargo test -p <crate> --test mutation_gap_<NN>_<short_name>`, and (2) by
applying the mutant by hand and confirming the test fails. Re-run
`scripts/mutation_test_embedded.sh <crate>` to confirm the mutant moved
from `missed.txt` to `caught.txt`.

## CI integration (suggested)

Mutation testing is too slow for every PR. The recommended cadence:

- **Per-PR**: run only the `tests/mutation_gap_*.rs` files (cheap;
  prevents regressions of already-closed gaps).
- **Nightly / weekly**: full sweep across all embedded crates with
  `--shard` parallelism, posting the mutation-score delta to a dashboard
  or CI summary.

If a nightly run regresses the mutation score, treat it like a coverage
regression: open a tracking issue, add a gap-closing test.
