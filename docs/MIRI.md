# Miri — Undefined Behavior Detection for SedanDelivery

[Miri] is an experimental interpreter for Rust's mid-level intermediate
representation. It can detect a variety of undefined behavior (UB) that the
compiler does not catch at compile time:

- Out-of-bounds memory access and use-after-free
- Invalid use of uninitialized memory
- Violation of pointer aliasing rules (Stacked Borrows / Tree Borrows)
- Data races
- Memory leaks (when `-Zmiri-leak-check` is enabled)
- Improper integer / pointer casts that violate strict provenance

[Miri]: https://github.com/rust-lang/miri

This document tracks every `unsafe` block in the SedanDelivery workspace,
classifies whether it is amenable to checking under Miri, and describes the
Miri job that runs in CI as a defense-in-depth measure against UB introduced
by future changes (either in our own code or in transitive dependencies).

## Audit of `unsafe` blocks

The audit is reproducible with:

```bash
grep -rEn "\bunsafe\s*(\{|fn|impl|trait)" --include="*.rs" \
  cypher-core gremlin-core gql-engine \
  sedan-cypher-embedded sedan-gql-embedded sedan-gremlin-embedded \
  sd2-redis-store gql-fabric-store sd2-redis-module
```

| # | Crate | File:line | Kind | Justification | Miri-compatible? |
|---|-------|-----------|------|---------------|------------------|
| 1 | `sd2-redis-module` | `src/module_store/common.rs:35` | Pointer dereference (`unsafe { &*ctx_ptr }`) | The Valkey/Redis module API hands us a raw `*mut RedisModuleCtx` per command callback. We stash it in a thread-local at callback entry and clear it at callback exit. Module commands are dispatched on Redis's single main thread, so the borrow is serialized with the pointer's lifetime. | **No** — this code only links inside the `cdylib` Redis module, which depends on `redis_module` bindgen FFI to the live Valkey server. There is no test harness that exercises this path without a running Valkey. Miri cannot model the foreign code that produces the `*mut RedisModuleCtx`. |

**Total `unsafe` blocks in the workspace:** 1.

All of the workspace's pure-logic crates — `cypher-core`, `gremlin-core`,
`gql-engine`, `sedan-cypher-embedded`, `sedan-gql-embedded`,
`sedan-gremlin-embedded`, `sd2-redis-store`, and `gql-fabric-store` — contain
**zero** `unsafe` blocks. The matches reported by `grep -rn "unsafe "` in
those crates are all string literals or comments referencing
"cluster-unsafe SCAN", not actual `unsafe` code.

### Why we still run Miri

Although the only first-party `unsafe` block is FFI-bound, Miri remains
useful as a regression net:

1. **Future drift.** Any new first-party `unsafe` block will be caught by
   the audit grep above (run as part of the Miri script) and should be
   added to this table together with a Miri verdict.
2. **Transitive UB.** Our pure-logic crates pull in dependencies that do
   contain `unsafe` (e.g. `indexmap`, `hashbrown`, `serde_json`,
   `rustc-hash`). Running our existing unit tests under Miri exercises
   those code paths against real input shapes from our domain types and
   would surface UB caused by mis-using a dependency's API.
3. **Aliasing assumptions.** Miri's Stacked / Tree Borrows checking
   catches aliasing-rule violations that compile cleanly but are UB.

## What the Miri job runs

The Miri smoke job runs a curated subset of `cypher-core` unit tests that
exercise the value model, canonical hashing, conversions, comparisons, and
serde round-trips. These were chosen because they:

- Are pure-logic (no I/O, no time, no env, no syscalls — Miri compatible).
- Cover hashing/equality semantics where transitive `unsafe` in
  `hashbrown` / `indexmap` is most likely to be exercised.
- Run in well under one minute end-to-end under Miri.

The exact filter list is defined in `scripts/run_miri.sh` under the
`MIRI_TEST_FILTERS` array; update it when adding new pure-logic test
modules.

### Tests excluded from Miri

The following test surfaces are intentionally **not** run under Miri because
they hit Miri-incompatible code paths:

| Surface | Reason |
|---------|--------|
| `cypher-core` integration tests under `tests/` (TCK, end-to-end, persistence, stress) | File I/O, large fixture loading, time-dependent assertions, multi-threaded executor paths. Run under `cargo test --release` instead. |
| `sd2-redis-store`, `sd2-redis-module` | Require a live Valkey server and FFI into the Redis module API. |
| `gql-fabric-store` | Requires Microsoft Fabric / OneLake credentials. |
| `sedandelivery-bolt`, `sedandelivery-gremlin*`, `sd2-api`, `sedandelivery-fabric-sync` | Network servers — exercised by integration suites, not Miri. |
| `gremlin-tck` | Long-running scenario suite (1672 tests); too slow under Miri's interpreter. |

## `MIRIFLAGS`

The default `MIRIFLAGS` used by `scripts/run_miri.sh` are:

```
MIRIFLAGS="-Zmiri-strict-provenance"
```

We do **not** enable `-Zmiri-disable-isolation` by default. The curated
test subset is fully hermetic — no clock, no env, no filesystem — so
isolation can stay on, which is the strongest UB-detection mode. If a
future test legitimately needs `getrandom`, `SystemTime::now`, or env
access under Miri, document the reason in this file and only relax
isolation for that specific test invocation.

## CI integration

`.github/workflows/miri.yml` runs the script on:

- Pull requests labelled `run-miri` (opt-in to keep the default PR path fast)
- A nightly cron at 06:45 UTC

The job is marked `continue-on-error: true` while we establish a stable
baseline. Once the job has been green for two consecutive weeks it should
be promoted to a required check by removing `continue-on-error`.

## Triaging Miri failures

If Miri reports UB:

1. **Do not** silence the failure with `#[cfg(not(miri))]` or by removing
   the test from the curated set without filing an issue.
2. File a GitHub issue per distinct UB diagnostic (one bug = one issue) so
   each can be prioritized and fixed independently. Attach the full Miri
   trace (`MIRIFLAGS="-Zmiri-backtrace=full" bash scripts/run_miri.sh`).
3. If the UB is in a transitive dependency, link the upstream issue / PR
   on top of our tracking issue and pin or patch the dependency in
   `Cargo.toml` until upstream is fixed.

## Local usage

```bash
# One-time
rustup toolchain install nightly
rustup +nightly component add miri rust-src

# Run the curated Miri suite
bash scripts/run_miri.sh

# Run a single test under Miri (fastest feedback loop)
CARGO_TARGET_DIR=target-miri \
  cargo +nightly miri test -p cypher-core --lib \
  types::value::canonical::tests::canonical_hash_float_nan
```

The script uses a separate `target-miri/` directory so that Miri's
instrumented artifacts do not invalidate your normal stable-toolchain
build cache.
