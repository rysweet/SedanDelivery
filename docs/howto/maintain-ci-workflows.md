# How to Maintain CI Workflows

This guide documents the CI maintenance patterns established by
[issue #1772](https://github.com/cloud-ecosystem-security/SedanDelivery/issues/1772):
Docker validation whitelisting, GitHub Actions arithmetic safety, and Gremlin
text parser fuzz hardening.

---

## Docker Validation Whitelist

### Problem

`scripts/validate-dockerfiles.sh` checks that every workspace member's
`Cargo.toml` is copied into each Dockerfile. However, not all workspace members
are intended for Docker images — test harnesses, fuzz targets, benchmarks, and
examples are development-only crates that should never appear in a production
container.

When these crates are added to the workspace, the validator fails with:

```
FAIL: Dockerfile: missing 'COPY tests/embedded-fuzz/Cargo.toml'
FAIL: Dockerfile: missing 'COPY tests/bolt-fuzz/Cargo.toml'
```

### Solution: `DOCKER_WHITELIST`

The validator maintains a `DOCKER_WHITELIST` array of workspace members that are
intentionally excluded from Docker builds. Whitelisted members are skipped in
three validation checks:

| Check | What it validates | Whitelist effect |
|-------|-------------------|------------------|
| Check 3 | Every member's `Cargo.toml` is copied before `cargo build` | Skips whitelisted members |
| Check 5 | Every `[[bin]]` member has a `src/main.rs` stub | Skips whitelisted members |
| Check 6 | `.dockerignore` re-includes `tests/` members | Skips whitelisted members |

### Current whitelist

```bash
DOCKER_WHITELIST=(
    "tests/embedded-fuzz"
    "tests/bolt-fuzz"
    "tests/concurrent-stress"
    "tests/scalability"
    "tests/soak"
    "tests/differential"
    "examples/embedded_app"
    "examples/embedded_quickstart"
)
```

### Adding a new crate to the whitelist

When you add a new workspace member that is **not** meant to ship in a Docker
image (e.g., a new test harness or benchmark crate):

1. Add the member path to the `DOCKER_WHITELIST` array in
   `scripts/validate-dockerfiles.sh`.
2. Run the validator to confirm it passes:
   ```bash
   bash scripts/validate-dockerfiles.sh
   ```
3. Commit the change alongside the new crate.

**When NOT to whitelist:** If the crate produces a binary that will be deployed
(a new server, API, or sidecar), it must be in the Dockerfile — do not add it
to the whitelist. Instead, add the appropriate `COPY` instructions to every
Dockerfile that builds it.

### Running the validator

```bash
# Full validation
bash scripts/validate-dockerfiles.sh

# Expected output when all checks pass:
# Workspace members (17): cypher-core gremlin-core gql-engine ...
# PASS: Dockerfile copies Cargo.lock
# ...
# All checks passed.
```

---

## GitHub Actions Arithmetic: `((var++))` vs `$((var + 1))`

### Problem

Bash `((var++))` returns exit code 1 when `var` is 0 before the increment
(because the pre-increment value is falsy). Under `set -e` (the default for
GitHub Actions `run:` blocks), this kills the step:

```bash
skipped_count=0
# ... later ...
((skipped_count++))  # Exit code 1! Step aborts.
```

### Solution

All counter increments use the assignment form, which always exits 0:

```bash
skipped_count=$((skipped_count + 1))
```

### Affected workflow

`.github/workflows/update-pr-branches.yml` — the Update PR Branches workflow
uses three counters (`skipped_count`, `updated_count`, `failed_count`) to track
how many PRs were updated, skipped, or failed. All increment operations use the
safe `var=$((var + 1))` form.

### Rule for new workflows

Never use `((var++))` or `((var--))` in any GitHub Actions `run:` block.
Always use the assignment form:

```bash
# ✗ Breaks under set -e when var is 0
((counter++))

# ✓ Always exits 0
counter=$((counter + 1))
```

---

## Gremlin Text Parser: Fuzz-Safe Numeric Suffix Parsing

### Problem

The Gremlin text parser's collection literal handler
(`gremlin-core/src/text_parser/literal/collection.rs`) parses numeric literals
with type suffixes (e.g., `42L`, `3.14f`). The original code used direct byte
slicing to strip the suffix:

```rust
let num_part = &s[..s.len() - 1];  // panics on multi-byte UTF-8 last char
```

If the last character is a multi-byte UTF-8 code point (e.g., `42é`), the slice
index falls inside the character boundary and Rust panics with
`byte index N is not a char boundary`. This was discovered by fuzz testing.

### Solution

The parser uses `s.get(..s.len() - 1)` instead of direct indexing. The `get()`
method returns `None` if the range boundary is not a valid char boundary,
and the code falls through to the sub-traversal parser — the correct behavior
for non-numeric input:

```rust
if let Some(num_part) = s.get(..s.len() - 1) {
    // parse numeric suffix...
}
```

### Contract

All three parsers (openCypher, GQL, Gremlin) satisfy the fuzz contract:

> For any UTF-8 input, `parse(...)` returns `Result<_, ParseError>` — never
> panics, aborts, or runs indefinitely on bounded input.

A panic discovered by fuzzing is a real bug. See [Fuzzing](../FUZZING.md) for
the full fuzz testing guide.

### Running the fuzz smoke test

```bash
bash scripts/fuzz_smoke.sh
```

To target only the Gremlin parser:

```bash
FUZZ_TARGETS="gremlin_text_parser" bash scripts/fuzz_smoke.sh
```

---

## Gremlin TCK CI Concurrency

### Background

The Gremlin TCK (`tests/gremlin-tck/`) uses the `cucumber` crate to run 1672
Gherkin scenarios. The cucumber harness controls its own concurrency via the
`--concurrency` flag and does not support `cargo test`'s `--test-threads` flag.

### Current CI invocation

The CI workflow runs the Gremlin TCK with:

```bash
cargo test -p gremlin-tck --test tck -- --concurrency 1
```

The `--concurrency 1` flag is passed to the cucumber runner (not to the Rust
test harness) and ensures deterministic, serial scenario execution. Do not add
`--test-threads` — it will cause a "unrecognized option" error from the
cucumber binary.

### Running locally

```bash
# Serial execution (matches CI)
cargo test -p gremlin-tck --test tck -- --concurrency 1

# Parallel execution (faster locally, may reorder output)
cargo test -p gremlin-tck --test tck
```

---

## See also

- [Validation Guide](../validation) — Full test suite, CI tiers, regression policy
- [Fuzzing](../FUZZING) — Fuzz harness setup, smoke campaigns, crash triage
- [Fix Docker Exit Code 101](fix-docker-exit-101) — Cargo.lock in Docker builds
- [Docker + Rust Reproducible Builds](../concepts/docker-rust-reproducible-builds) — Layer-caching pattern, `--locked` flag
- [GitHub Actions Workflows](../../.github/workflows/README.md) — Workflow inventory
