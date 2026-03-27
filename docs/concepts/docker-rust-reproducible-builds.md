# Docker + Rust: Reproducible Builds with Cargo.lock

## Overview

Rust workspace builds inside Docker require both `Cargo.toml` and `Cargo.lock` to
be copied into the build context before any `cargo build` invocation. Without
`Cargo.lock`, Cargo resolves dependency versions at build time ("floating
resolution"), which can select incompatible crate versions and produce compilation
failures.

## The Layer-Caching Pattern

SedanDelivery's Dockerfiles use a two-phase dependency pre-warm strategy:

1. **Copy manifests** — `COPY Cargo.toml Cargo.lock ./` plus each workspace member's
   `Cargo.toml`, establishing the full dependency graph.
2. **Stub sources** — Create minimal placeholder source files so Cargo can load the
   workspace without real implementation code.
3. **Pre-warm build** — `cargo build --release -p <target> || true` downloads and
   compiles all dependencies into a Docker layer. The `|| true` absorbs failures from
   stub-only source (expected; only dependencies are cached, not binaries).
4. **Copy real sources** — Overwrite stubs with actual implementation files.
5. **Final build** — `cargo build --release --locked -p <target>` compiles the real
   binary using the now-cached dependency artifacts.

The `--locked` flag on the final build step enforces that Cargo uses only the exact
versions pinned in `Cargo.lock`, making the build reproducible across runs, machines,
and CI agents.

## Why Exit Code 101 {#why-exit-code-101}

Exit code **101** is **Cargo's own diagnostic exit code** for compilation failure —
it is not a Docker error, not a network error, and not a runner-environment problem.
Docker surfaces it because Docker propagates the exit code of the `RUN` instruction's
process unchanged.

When `Cargo.lock` is absent from the Docker build context:

- Cargo performs unconstrained semver resolution for every dependency.
- Transitive dependency version selections can change between runs or compared with
  the developer's local environment.
- An incompatible version selection (e.g. a crate that introduces a breaking API
  change in a patch release) causes a compile error, and Cargo exits 101.

Including `Cargo.lock` in the `COPY` step eliminates this class of failure entirely:
Cargo resolves from the pinned lockfile, using the same versions the developer tested.

## Security Implications

Copying `Cargo.lock` into the Docker build context improves supply-chain integrity:

- **Pinned dependency selection** — Cargo resolves the same versions the
  developer tested instead of re-solving transitive dependencies during the
  Docker build.
- **Deterministic dependency graph** — Every image built from the same commit has
  identical transitive dependencies, making security audits reproducible.
- **No floating resolution** — A crate that is patched with a CVE fix cannot be
  silently upgraded by Cargo in CI while an incompatible version is pulled elsewhere.
- **Consistent audit input** — Tools like `cargo audit` and `cargo deny` evaluate
  the pinned dependency set from `Cargo.lock`, so CI and local review examine the
  same graph.

> **Recommended (CI hardening):** Add `cargo audit` or `cargo deny` as a CI step
> to detect CVEs in the pinned dependency versions before an image is published.
> `cargo audit` scans `Cargo.lock` against the RustSec Advisory Database.
> This is not required for correct builds but strengthens supply-chain posture.

## Rule: `Cargo.lock` Must Remain in Git

For binary crates and Docker-built workspaces, `Cargo.lock` **must** be committed
to the repository and must not appear in `.gitignore`. Removing it causes every
Docker build to re-resolve dependencies, which:

- Breaks reproducibility.
- Reintroduces the exit 101 failure class described above.
- Changes the dependency graph that local review and security tooling are auditing.

The Cargo documentation is explicit: for binary crates (as opposed to libraries),
`Cargo.lock` should always be committed.

## Summary

| Condition | Result |
|-----------|--------|
| `Cargo.lock` copied + `--locked` flag | Reproducible, deterministic build |
| `Cargo.lock` copied, no `--locked` flag | Reproducible by default (Cargo uses lockfile when present) |
| `Cargo.lock` absent from Docker context | Floating resolution → potential exit 101 |
| `Cargo.lock` in `.gitignore` | Lockfile unavailable at build time → same as absent |
