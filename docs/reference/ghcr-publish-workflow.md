# ghcr-publish Workflow Reference

## Overview

The `ghcr-publish` workflow builds and publishes four Docker images to GitHub
Container Registry (ghcr.io). It runs on pushes to `main`, semver tags matching
`v*`, and manual `workflow_dispatch` runs.

## Trigger

```yaml
on:
  push:
    branches: [main]
    tags:
      - 'v*'
  workflow_dispatch:
```

That means the workflow covers:

- default-branch image publication on `main`
- release/tag publication for `v*`
- manual reruns for recovery or debugging

## Build Matrix

| Matrix image | Dockerfile | Published image | Purpose |
|--------------|------------|-----------------|---------|
| `sedandelivery` | `Dockerfile` | `ghcr.io/<owner>/sedandelivery` | Valkey + gql-module Redis extension |
| `sedandelivery-bolt` | `Dockerfile.bolt` | `ghcr.io/<owner>/sedandelivery-bolt` | Standalone Bolt protocol server |
| `sedandelivery-fabric-sync` | `Dockerfile.sync` | `ghcr.io/<owner>/sedandelivery-fabric-sync` | Fabric Graph replication sidecar |
| `sedandelivery-sync` | `Dockerfile.sync` | `ghcr.io/<owner>/sedandelivery-sync` | Compatibility alias for the Fabric sync sidecar |

## Build Context

The workflow sets `context: .` for every matrix entry, so Docker starts from the
repository root. The effective build context is still filtered by
`.dockerignore`, which excludes most docs, scripts, and test trees while
re-including the specific test manifests the Dockerfiles need. `Cargo.lock`
remains available because it lives at the repository root and is not excluded.

## Cargo.lock Requirements

Across the repo's four workflow-built Rust Dockerfiles, each builder stage
copies `Cargo.lock` alongside `Cargo.toml` at the start of the builder stage:

```dockerfile
COPY Cargo.toml Cargo.lock ./
```

This is required for reproducible builds. Without `Cargo.lock`, Cargo performs
floating version resolution at build time, which can select incompatible crate
versions and produce a compilation failure (exit code 101).

To verify the current workflow-built Dockerfiles contain the lockfile copy:

```bash
grep -n 'COPY Cargo' Dockerfile Dockerfile.bolt Dockerfile.sync Dockerfile.api
```

Expected output (line numbers may vary):

```
Dockerfile:16:COPY Cargo.toml Cargo.lock ./
Dockerfile.bolt:22:COPY Cargo.toml Cargo.lock ./
Dockerfile.sync:24:COPY Cargo.toml Cargo.lock ./
Dockerfile.api:26:COPY Cargo.toml Cargo.lock ./
```

PR CI, `ghcr-publish`, and `deploy.yml` enforce this contract with:

```bash
bash tests/test_dockerfile_cargo_lock.sh
bash scripts/validate-dockerfiles.sh
```

Those checks verify the lockfile copy, workspace manifest ordering, required
`[[bin]]` stubs, and `--locked` usage both before merge and again before any
publish or deploy workflow begins its image-build path. `ghcr-publish.yml`
builds `Dockerfile`, `Dockerfile.bolt`, and `Dockerfile.sync`; `deploy.yml`
also builds `Dockerfile.api`; and PR CI runs builder stages for all four
Dockerfiles, so a pull request has to survive a real Docker build before it can
merge.

## --locked Flag

The final `cargo build --release` step in each Dockerfile uses `--locked`:

```dockerfile
RUN cargo build --release --locked -p <crate>
```

The `--locked` flag causes Cargo to error rather than silently re-resolve if the
lockfile is absent or stale, making lockfile omissions fail loudly instead of
producing a subtly broken image.

## Package Visibility Step

After each image is pushed, the workflow calls `gh api` to set the package
visibility to public:

```yaml
env:
  GH_TOKEN: ${{ secrets.GH_AW_GITHUB_TOKEN }}
```

If `GH_AW_GITHUB_TOKEN` is unset, the workflow fails intentionally before the
package-visibility PATCH call. That secret is required in addition to the
standard `GITHUB_TOKEN` used for the registry login.

## Not in Scope

The following files are **not** modified by the ghcr-publish workflow and are
referenced here only for clarity:

- `Dockerfile.bolt.prebuilt` — alternative bolt image using a pre-built binary;
  not part of the ghcr-publish matrix.
- `Dockerfile.api` / `Dockerfile.gremlin` — not part of the ghcr-publish matrix.
- `docker-compose*.yml` — local development and testing; not used by CI publish.

## Troubleshooting

| Symptom | Likely Cause | Resolution |
|---------|-------------|------------|
| Exit code 101 in `cargo build` | `Cargo.lock` missing from Dockerfile `COPY` | See [fix-docker-exit-101](../howto/fix-docker-exit-101.md) |
| `--locked` fails with "needs update" | `Cargo.lock` is stale | Run `cargo update` locally and commit the updated lockfile |
| Package push succeeds but package stays private | `GH_AW_GITHUB_TOKEN` missing or lacks package admin access | Add the secret and re-run `ghcr-publish` |
| Image not published after a release/tag push | Tag does not match `v*` pattern | Use semver tags: `git tag v1.2.3 && git push origin v1.2.3` |
| Expected `latest` tag missing | Workflow did not run on `main` | Confirm the push targeted the default branch or run `workflow_dispatch` |
