# How to Fix Docker Rust Build Exit Code 101

## Symptoms

A Docker build for a Rust workspace fails with a message similar to:

```
#12 [builder 6/9] RUN cargo build --release -p sedandelivery-bolt 2>/dev/null || true
...
error[E0433]: failed to resolve: use of undeclared crate or module `redis`
...
error: could not compile `sedandelivery-bolt`
The command '/bin/sh -c cargo build ...' returned a non-zero code: 101
```

Exit code **101 is Cargo's own exit code** for compilation failure. It is not a
Docker error or a network problem — Docker surfaces it because the `RUN cargo build`
process returned 101 and Docker propagates that exit code unchanged.

See [Why Exit Code 101](../concepts/docker-rust-reproducible-builds.md#why-exit-code-101)
for a full explanation of how floating dependency resolution produces this failure.

## Root Cause

The Dockerfile copies `Cargo.toml` but not `Cargo.lock` before the dependency
pre-warm step. Without the lockfile, Cargo performs unconstrained semver resolution
and may select a crate version that is incompatible with the workspace.

## Fix

Add `COPY Cargo.lock ./` immediately after `COPY Cargo.toml ./` in the builder stage
of every affected Dockerfile, **before** any `RUN cargo build` instruction.

**Before (broken):**

```dockerfile
COPY Cargo.toml ./
COPY cypher-core/Cargo.toml ./cypher-core/
# ... more COPY instructions ...
RUN ... cargo build --release -p my-crate || true
```

**After (fixed):**

```dockerfile
COPY Cargo.toml Cargo.lock ./
COPY cypher-core/Cargo.toml ./cypher-core/
# ... more COPY instructions ...
RUN ... cargo build --release -p my-crate || true
```

Alternatively, use two separate `COPY` instructions if you prefer explicit lines:

```dockerfile
COPY Cargo.toml ./
COPY Cargo.lock ./
COPY cypher-core/Cargo.toml ./cypher-core/
```

Both forms are equivalent. The single-line form (`COPY Cargo.toml Cargo.lock ./`)
is preferred because it creates one Docker layer instead of two.

## Verification

After applying the fix, run both verification checks:

```bash
bash tests/test_dockerfile_cargo_lock.sh
bash scripts/validate-dockerfiles.sh
```

Expected output includes:

```text
Results: 12 passed, 0 failed
OK
All checks passed.
```

You can also verify manually:

```bash
grep -n 'COPY Cargo' Dockerfile Dockerfile.bolt Dockerfile.sync Dockerfile.api
```

Each file should show a single line containing both `Cargo.toml` and `Cargo.lock`,
or two consecutive lines — one for each file.

## Checklist

- [ ] `Cargo.lock` is tracked in git (`git ls-files Cargo.lock` returns the file)
- [ ] `Cargo.lock` is not listed in `.gitignore`
- [ ] All workflow-built Rust Dockerfiles contain `COPY Cargo.lock ./` before any `cargo build`
- [ ] `bash tests/test_dockerfile_cargo_lock.sh` exits 0
- [ ] `bash scripts/validate-dockerfiles.sh` exits 0

## Prevention

To prevent regression:

1. Run both `tests/test_dockerfile_cargo_lock.sh` and
   `scripts/validate-dockerfiles.sh` in PR CI, and run real builder-stage Docker
   builds before merging changes to workflow-built Rust images.
2. Never add `Cargo.lock` to `.gitignore` for binary crates — Cargo documentation
   recommends committing the lockfile for all executable packages.
3. Use `cargo build --release --locked` for the final (non-stub) build step. The
   `--locked` flag causes Cargo to error rather than silently re-resolve if the
   lockfile is stale or absent.
